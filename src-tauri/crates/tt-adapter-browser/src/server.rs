use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    hash::{DefaultHasher, Hash, Hasher},
    io,
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, Response, StatusCode, header},
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures_util::stream;
use headers::{ETag, HeaderMapExt, IfNoneMatch};
use serde_json::{Value, json};
use tokio::sync::{Semaphore, broadcast, mpsc, oneshot};

use tokio::sync::OwnedSemaphorePermit;

const SESSION_LIFETIME: Duration = Duration::from_secs(8 * 60 * 60);
const SESSION_IDLE_GRACE: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_BODY: usize = 8 * 1024 * 1024;
const COOKIE: &str = "tt_browser_session";

pub struct Config {
    pub bind: SocketAddr,
    pub origin: String,
    pub token: String,
    pub require_token: bool,
}

#[derive(serde::Deserialize)]
struct StoredConfig {
    bind: SocketAddr,
    origin: String,
    #[serde(default = "default_require_token")]
    require_token: bool,
}

fn default_require_token() -> bool {
    true
}

impl Config {
    pub fn load(path: &Path) -> io::Result<Option<Self>> {
        if let Some(config) = Self::from_env()? {
            return Ok(Some(config));
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let stored: StoredConfig = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let config = Self {
            bind: stored.bind,
            origin: stored.origin,
            require_token: stored.require_token,
            token: if stored.require_token {
                std::fs::read_to_string(path.with_file_name("browser-access-token"))?
                    .trim()
                    .to_owned()
            } else {
                String::new()
            },
        };
        config.validate()?;
        Ok(Some(config))
    }

    /// Missing bind means completely disabled. Configuration errors never fall back
    /// to a wildcard listener, a different authentication mode, or a different data owner.
    pub fn from_env() -> io::Result<Option<Self>> {
        let Some(bind) = std::env::var_os("TAURITAVERN_BROWSER_BIND") else {
            return Ok(None);
        };
        let origin = std::env::var("TAURITAVERN_BROWSER_ORIGIN")
            .map_err(|_| invalid("TAURITAVERN_BROWSER_ORIGIN is required"))?;
        let require_token = match std::env::var("TAURITAVERN_BROWSER_REQUIRE_TOKEN") {
            Ok(value) => value
                .parse::<bool>()
                .map_err(|_| invalid("TAURITAVERN_BROWSER_REQUIRE_TOKEN must be true or false"))?,
            Err(std::env::VarError::NotPresent) => true,
            Err(_) => return Err(invalid("Invalid TAURITAVERN_BROWSER_REQUIRE_TOKEN")),
        };
        let token = if require_token {
            let token_path = std::env::var_os("TAURITAVERN_BROWSER_TOKEN_FILE")
                .ok_or_else(|| invalid("TAURITAVERN_BROWSER_TOKEN_FILE is required"))?;
            std::fs::read_to_string(token_path)?.trim().to_owned()
        } else {
            String::new()
        };
        let config = Self {
            bind: bind
                .to_string_lossy()
                .parse()
                .map_err(|_| invalid("Invalid browser bind address"))?,
            origin,
            token,
            require_token,
        };
        config.validate()?;
        Ok(Some(config))
    }

    fn validate(&self) -> io::Result<()> {
        if !self.bind.ip().is_loopback() {
            return Err(invalid(
                "Browser access must bind to loopback; use Tailscale Serve for HTTPS",
            ));
        }
        let url = url::Url::parse(&self.origin).map_err(|_| invalid("Invalid browser origin"))?;
        let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        let tailnet_ip = url
            .host_str()
            .and_then(|host| host.parse::<std::net::Ipv4Addr>().ok())
            .is_some_and(|ip| ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]));
        let tailnet_dns = url.host_str().is_some_and(|host| {
            let labels: Vec<_> = host.split('.').collect();
            matches!(labels.as_slice(), [device, tailnet, "ts", "net"]
                if !device.is_empty() && !tailnet.is_empty())
        });
        let private_origin = local || tailnet_ip || tailnet_dns;
        if url.origin().ascii_serialization() != self.origin
            || !url.username().is_empty()
            || url.password().is_some()
            || !(url.scheme() == "https" || (url.scheme() == "http" && private_origin))
        {
            return Err(invalid(
                "Browser origin must be exact HTTPS, loopback HTTP, or private Tailscale IPv4/MagicDNS HTTP",
            ));
        }
        if !(self.require_token || private_origin) {
            return Err(invalid(
                "Token-free access requires a loopback or Tailscale IPv4/MagicDNS origin",
            ));
        }
        if self.require_token && (self.token.len() < 64
            || self.token.len() > 128
            || !self.token.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(invalid(
                "Browser token must contain 64–128 random hexadecimal characters",
            ));
        }
        Ok(())
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Revalidate already-resolved representations against their current bytes and
/// MIME. Callers must resolve current data before calling this; cached reads
/// never bypass the host. User files keep HostResourceService's metadata policy.
pub fn cached_response(
    request: &axum::http::Request<Vec<u8>>,
    bytes: Vec<u8>,
    mime_type: &str,
) -> Response<Vec<u8>> {
    let mut hash = DefaultHasher::new();
    bytes.hash(&mut hash);
    mime_type.hash(&mut hash);
    let etag: ETag = format!("W/\"browser-{:016x}-{}\"", hash.finish(), bytes.len())
        .parse()
        .expect("generated asset validator");
    let not_modified = request
        .headers()
        .typed_get::<IfNoneMatch>()
        .is_some_and(|condition| !condition.precondition_passes(&etag));
    let mut builder = Response::builder()
        .status(if not_modified { StatusCode::NOT_MODIFIED } else { StatusCode::OK })
        .header(header::CACHE_CONTROL, "private, no-cache");
    if !not_modified {
        builder = builder
            .header(header::CONTENT_TYPE, mime_type)
            .header(header::CONTENT_LENGTH, bytes.len());
    }
    let mut response = builder
        .body(if not_modified || request.method() == Method::HEAD { Vec::new() } else { bytes })
        .expect("asset response");
    response.headers_mut().typed_insert(etag);
    response
}

pub enum HostRequest {
    Invoke {
        payload: Value,
        reply: oneshot::Sender<Value>,
        permit: OwnedSemaphorePermit,
    },
    Resource {
        request: Box<axum::http::Request<Vec<u8>>>,
        reply: oneshot::Sender<Response<Vec<u8>>>,
        permit: OwnedSemaphorePermit,
    },
}

pub struct Server {
    pub requests: mpsc::Receiver<HostRequest>,
    pub events: broadcast::Sender<Value>,
    listener: tokio::net::TcpListener,
    router: Router,
}

impl Server {
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub fn into_parts(
        self,
    ) -> (
        mpsc::Receiver<HostRequest>,
        broadcast::Sender<Value>,
        impl Future<Output = io::Result<()>> + Send,
    ) {
        (self.requests, self.events, async move {
            axum::serve(self.listener, self.router).await
        })
    }
}

struct Session {
    expires_at: Instant,
    last_used: Instant,
    in_use: Arc<()>,
}

struct Shared {
    config: Config,
    authorities: [String; 3],
    origins: [String; 3],
    sessions: Mutex<HashMap<String, Session>>,
    login_attempts: Mutex<VecDeque<Instant>>,
    requests: mpsc::Sender<HostRequest>,
    events: broadcast::Sender<Value>,
    invokes: Arc<Semaphore>,
    resources: Arc<Semaphore>,
    event_connections: Arc<Semaphore>,
}

/// Binding is synchronous so an occupied port fails host setup immediately.
pub fn bind(config: Config) -> io::Result<Server> {
    config.validate()?;
    let listener = std::net::TcpListener::bind(config.bind)?;
    listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(listener)?;
    let (requests, receiver) = mpsc::channel(64);
    let (events, _) = broadcast::channel(256);
    let authority = config
        .origin
        .split_once("://")
        .expect("validated origin")
        .1
        .to_owned();
    let local_addr = listener.local_addr()?;
    let authorities = [
        authority,
        local_addr.to_string(),
        format!("localhost:{}", local_addr.port()),
    ];
    let origins = [
        config.origin.clone(),
        format!("http://{}", authorities[1]),
        format!("http://{}", authorities[2]),
    ];
    let shared = Arc::new(Shared {
        authorities,
        origins,
        config,
        sessions: Mutex::new(HashMap::new()),
        login_attempts: Mutex::new(VecDeque::new()),
        requests,
        events: events.clone(),
        invokes: Arc::new(Semaphore::new(32)),
        resources: Arc::new(Semaphore::new(16)),
        event_connections: Arc::new(Semaphore::new(8)),
    });
    Ok(Server {
        requests: receiver,
        events,
        listener,
        router: Router::new().fallback(handle).with_state(shared),
    })
}

fn text(status: StatusCode, message: &'static str) -> Response<Body> {
    (status, message).into_response()
}

fn json_response(value: Value) -> Response<Body> {
    axum::Json(value).into_response()
}

fn session_id(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(name, value)| (name == COOKIE).then_some(value))
}

fn authenticate(shared: &Shared, headers: &HeaderMap) -> Option<Arc<()>> {
    let id = session_id(headers)?;
    let now = Instant::now();
    let mut sessions = shared.sessions.lock().expect("session mutex");
    sessions.retain(|_, session| session.expires_at > now);
    let session = sessions.get_mut(id)?;
    session.last_used = now;
    Some(session.in_use.clone())
}

fn same_origin(shared: &Shared, headers: &HeaderMap) -> bool {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| shared.origins.iter().any(|allowed| allowed == origin))
}

fn same_token(expected: &str, actual: &str) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    expected
        .bytes()
        .zip(actual.bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

async fn handle(State(shared): State<Arc<Shared>>, request: Request) -> Response<Body> {
    let mut response = dispatch(shared, request).await;
    response
        .headers_mut()
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("private, no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("same-origin"));
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static("frame-ancestors 'self'"),
    );
    response
}

async fn dispatch(shared: Arc<Shared>, request: Request) -> Response<Body> {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    // Tailscale Serve may preserve Host or rewrite it to its loopback upstream.
    // Neither forwarded headers nor arbitrary DNS names are trusted.
    if !host.is_some_and(|host| shared.authorities.iter().any(|allowed| allowed == host)) {
        return text(StatusCode::FORBIDDEN, "Unexpected host");
    }
    if headers.contains_key(header::ORIGIN) && !same_origin(&shared, headers) {
        return text(StatusCode::FORBIDDEN, "Cross-origin access denied");
    }
    if headers
        .get("sec-fetch-site")
        .is_some_and(|value| value == "cross-site")
    {
        return text(StatusCode::FORBIDDEN, "Cross-site access denied");
    }
    let path = request.uri().path();
    if path == "/__remote/login" {
        if request.method() == Method::GET {
            if !shared.config.require_token {
                return (StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response();
            }
            return (
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                LOGIN_HTML,
            )
                .into_response();
        }
        if request.method() != Method::POST || !same_origin(&shared, headers) {
            return text(StatusCode::FORBIDDEN, "Login requires a same-origin POST");
        }
        return login(shared, request).await;
    }
    let Some(session_use) = authenticate(&shared, headers) else {
        if request.method() == Method::GET && (path == "/" || path == "/index.html") {
            if !shared.config.require_token {
                let mut response = create_session(
                    &shared,
                    shared.config.origin.starts_with("https://"),
                );
                if response.status().is_success() {
                    *response.status_mut() = StatusCode::SEE_OTHER;
                    response.headers_mut().insert(header::LOCATION, HeaderValue::from_static("/"));
                }
                return response;
            }
            return (
                StatusCode::SEE_OTHER,
                [(header::LOCATION, "/__remote/login")],
            )
                .into_response();
        }
        return text(
            StatusCode::UNAUTHORIZED,
            "Browser session expired; sign in again",
        );
    };
    if request.method() == Method::POST
        && (!same_origin(&shared, headers)
            || headers.get("x-tt-remote").is_none_or(|value| value != "1"))
    {
        return text(
            StatusCode::FORBIDDEN,
            "Same-origin transport header required",
        );
    }
    match (request.method(), path) {
        (&Method::GET, "/__remote/session") => json_response(json!({"transport": 1})),
        (&Method::POST, "/__remote/logout") => {
            if let Some(id) = session_id(headers) {
                shared.sessions.lock().expect("session mutex").remove(id);
            }
            let mut response = json_response(json!({}));
            response.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_static(
                    "tt_browser_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
                ),
            );
            response
        }
        (&Method::GET, "/__remote/events") => events(
            shared,
            session_id(headers).expect("authenticated").to_owned(),
            session_use,
        ),
        (
            &Method::GET,
            "/__remote/read/get_bootstrap_snapshot" | "/__remote/read/get_sillytavern_settings",
        ) => invoke(shared, request).await,
        (&Method::POST, "/__remote/invoke") => invoke(shared, request).await,
        (&Method::GET | &Method::HEAD | &Method::OPTIONS, _)
            if !path.starts_with("/__remote/") || path == "/__remote/file" =>
        {
            let Ok(permit) = shared.resources.clone().try_acquire_owned() else {
                return text(
                    StatusCode::TOO_MANY_REQUESTS,
                    "Too many active resource requests",
                );
            };
            let (parts, _) = request.into_parts();
            let (reply, result) = oneshot::channel();
            if shared
                .requests
                .try_send(HostRequest::Resource {
                    request: Box::new(axum::http::Request::from_parts(parts, Vec::new())),
                    reply,
                    permit,
                })
                .is_err()
            {
                return text(StatusCode::SERVICE_UNAVAILABLE, "Desktop host is busy");
            }
            match tokio::time::timeout(Duration::from_secs(30), result).await {
                Ok(Ok(response)) => response.map(Body::from),
                _ => text(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Desktop resource host unavailable",
                ),
            }
        }
        _ => text(StatusCode::NOT_FOUND, "Not found"),
    }
}

async fn login(shared: Arc<Shared>, request: Request) -> Response<Body> {
    // Both the configured private origin and this listener's loopback origins
    // use the same authenticated owner. Cookie security follows the actual origin.
    let https = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| origin.starts_with("https://"));
    {
        let now = Instant::now();
        let mut attempts = shared.login_attempts.lock().expect("login mutex");
        while attempts
            .front()
            .is_some_and(|time| now.duration_since(*time) >= Duration::from_secs(60))
        {
            attempts.pop_front();
        }
        if attempts.len() >= 10 {
            return text(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many login attempts; wait one minute",
            );
        }
        attempts.push_back(now);
    }
    let Ok(Ok(bytes)) =
        tokio::time::timeout(Duration::from_secs(15), to_bytes(request.into_body(), 1024)).await
    else {
        return text(StatusCode::BAD_REQUEST, "Invalid login");
    };
    let Ok(body) = serde_json::from_slice::<Value>(&bytes) else {
        return text(StatusCode::BAD_REQUEST, "Invalid login");
    };
    if shared.config.require_token
        && !same_token(&shared.config.token, body["token"].as_str().unwrap_or(""))
    {
        return text(StatusCode::UNAUTHORIZED, "Invalid access token");
    }
    create_session(&shared, https)
}

fn create_session(shared: &Shared, https: bool) -> Response<Body> {
    let id = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let now = Instant::now();
    let mut sessions = shared.sessions.lock().expect("session mutex");
    sessions.retain(|_, session| session.expires_at > now);
    if sessions.len() >= 8 {
        let idle = sessions
            .iter()
            .filter(|(_, session)| {
                Arc::strong_count(&session.in_use) == 1
                    && now.duration_since(session.last_used) >= SESSION_IDLE_GRACE
            })
            .min_by_key(|(_, session)| session.last_used)
            .map(|(id, _)| id.clone());
        if let Some(id) = idle {
            sessions.remove(&id);
        } else {
            return text(
                StatusCode::TOO_MANY_REQUESTS,
                "Browser sessions are busy; retry shortly or sign out an unused browser",
            );
        }
    }
    sessions.insert(
        id.clone(),
        Session {
            expires_at: now + SESSION_LIFETIME,
            last_used: now,
            in_use: Arc::new(()),
        },
    );
    let secure = if https { "; Secure" } else { "" };
    let mut response = json_response(json!({}));
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{COOKIE}={id}; HttpOnly; SameSite=Strict; Path=/; Max-Age=28800{secure}"
        ))
        .expect("generated cookie"),
    );
    response
}

async fn invoke(shared: Arc<Shared>, request: Request) -> Response<Body> {
    let Ok(permit) = shared.invokes.clone().try_acquire_owned() else {
        return text(StatusCode::TOO_MANY_REQUESTS, "Too many active commands");
    };
    // Only the two argument-free read routes above reach this branch. All other
    // commands keep POST, the transport header and the existing native policy.
    let mut cache_request = None;
    let payload = if request.method() == Method::GET {
        let command = request.uri().path().trim_start_matches("/__remote/read/");
        let payload = json!({"command": command, "args": {}, "headers": {}});
        let (parts, _) = request.into_parts();
        cache_request = Some(axum::http::Request::from_parts(parts, Vec::new()));
        payload
    } else {
        let Ok(Ok(bytes)) = tokio::time::timeout(
            Duration::from_secs(30),
            to_bytes(request.into_body(), MAX_BODY),
        )
        .await
        else {
            return text(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Command payload exceeds 8 MiB",
            );
        };
        let Ok(payload) = serde_json::from_slice::<Value>(&bytes) else {
            return text(StatusCode::BAD_REQUEST, "Invalid command payload");
        };
        payload
    };
    if payload["command"]
        .as_str()
        .is_none_or(|name| name.len() > 100)
    {
        return text(StatusCode::BAD_REQUEST, "Invalid command name");
    }
    let (reply, result) = oneshot::channel();
    if shared
        .requests
        .try_send(HostRequest::Invoke {
            payload,
            reply,
            permit,
        })
        .is_err()
    {
        return text(StatusCode::SERVICE_UNAVAILABLE, "Desktop host is busy");
    }
    match tokio::time::timeout(REQUEST_TIMEOUT, result).await {
        Ok(Ok(value)) if value["ok"] == true && cache_request.is_some() => {
            cached_response(
                cache_request.as_ref().expect("cacheable read"),
                serde_json::to_vec(&value).expect("JSON value"),
                "application/json",
            )
            .map(Body::from)
        }
        Ok(Ok(value)) => json_response(value),
        _ => text(
            StatusCode::GATEWAY_TIMEOUT,
            "Desktop command timed out; outcome unknown, do not retry a write blindly",
        ),
    }
}

fn events(shared: Arc<Shared>, session: String, session_use: Arc<()>) -> Response<Body> {
    let Ok(permit) = shared.event_connections.clone().try_acquire_owned() else {
        return text(StatusCode::TOO_MANY_REQUESTS, "Too many event connections");
    };
    let receiver = shared.events.subscribe();
    let stream = stream::unfold(
        (receiver, shared, session, session_use, permit),
        |(mut receiver, shared, session, session_use, permit)| async move {
            let expiry = {
                let mut sessions = shared.sessions.lock().expect("session mutex");
                let session = sessions.get_mut(&session)?;
                session.last_used = Instant::now();
                session.expires_at
            };
            if expiry <= Instant::now() {
                return None;
            }
            let message = match tokio::time::timeout(Duration::from_secs(15), receiver.recv()).await
            {
                Ok(Ok(value)) => value,
                Ok(Err(_)) => return None, // Never silently drop channel events on overflow.
                Err(_) => json!({"kind": "heartbeat"}),
            };
            Some((
                Ok::<_, Infallible>(Event::default().json_data(message).expect("JSON event")),
                (receiver, shared, session, session_use, permit),
            ))
        },
    );
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

const LOGIN_HTML: &str = r#"<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>TauriTavern browser access</title>
<style>body{font:18px system-ui;background:#181818;color:#eee;max-width:30rem;margin:12vh auto;padding:1rem}input,button{box-sizing:border-box;width:100%;padding:1rem;margin:.5rem 0;font:inherit}#error{color:#ffb0b0}</style>
<h1>TauriTavern</h1><p>Connect to your computer’s running TauriTavern. Paste its browser access token.</p>
<form><input type="password" aria-label="Access token" autocomplete="off" required><button>Connect</button></form>
<p id="error" role="alert"></p><p>Private, single-owner access. Do not edit the same chat from two devices at once.</p>
<script>document.querySelector('form').onsubmit=async e=>{e.preventDefault();const input=document.querySelector('input');try{const response=await fetch('/__remote/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({token:input.value.trim()})});input.value='';if(!response.ok)throw new Error(await response.text());location.replace('/');}catch(error){document.querySelector('#error').textContent=error.message;}};</script></html>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    fn server() -> Server {
        bind(Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            origin: "http://127.0.0.1:1432".into(),
            token: "a".repeat(64),
            require_token: true,
        })
        .unwrap()
    }

    fn request(method: Method, path: &str, cookie: Option<&str>, body: Value) -> Request {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "127.0.0.1:1432")
            .header(header::ORIGIN, "http://127.0.0.1:1432")
            .header("x-tt-remote", "1");
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        builder.body(Body::from(body.to_string())).unwrap()
    }

    async fn login_cookie(router: &Router) -> String {
        let response = router
            .clone()
            .oneshot(request(
                Method::POST,
                "/__remote/login",
                None,
                json!({"token": "a".repeat(64)}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(cookie.contains("HttpOnly; SameSite=Strict"));
        cookie.split(';').next().unwrap().to_owned()
    }

    #[tokio::test]
    async fn unauthenticated_and_cross_origin_requests_never_reach_the_host() {
        let mut server = server();
        for path in [
            "/characters/private.png",
            "/__remote/session",
            "/__remote/events",
        ] {
            let response = server
                .router
                .clone()
                .oneshot(request(Method::GET, path, None, Value::Null))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "private, no-store"
            );
        }
        let cookie = login_cookie(&server.router).await;
        let mut bad = request(
            Method::POST,
            "/__remote/invoke",
            Some(&cookie),
            json!({"command": "save_user_settings"}),
        );
        bad.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_static("http://evil.example"),
        );
        assert_eq!(
            server.router.clone().oneshot(bad).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let mut bad = request(Method::GET, "/characters/a.png", Some(&cookie), Value::Null);
        bad.headers_mut()
            .insert(header::HOST, HeaderValue::from_static("rebind.example"));
        assert_eq!(
            server.router.clone().oneshot(bad).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        assert!(server.requests.try_recv().is_err());
    }

    #[tokio::test]
    async fn authenticated_invokes_preserve_result_errors_and_logout_revokes_session() {
        let mut server = server();
        let cookie = login_cookie(&server.router).await;
        let payload = json!({"command":"append_chat_commit_chunk","args":{"__ttBinary":"AP8="},
            "headers":{"session-id":"synthetic","offset":"0"}});
        let response_future = server.router.clone().oneshot(request(
            Method::POST,
            "/__remote/invoke",
            Some(&cookie),
            payload.clone(),
        ));
        let host = async {
            match server.requests.recv().await.unwrap() {
                HostRequest::Invoke {
                    payload: actual,
                    reply,
                    ..
                } => {
                    assert_eq!(actual, payload);
                    reply
                        .send(json!({"ok":false,"error":"synthetic conflict"}))
                        .unwrap();
                }
                _ => panic!("wrong request kind"),
            }
        };
        let (response, ()) = tokio::join!(response_future, host);
        let bytes = to_bytes(response.unwrap().into_body(), MAX_BODY)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["error"],
            "synthetic conflict"
        );
        let logout = request(Method::POST, "/__remote/logout", Some(&cookie), Value::Null);
        assert_eq!(
            server
                .router
                .clone()
                .oneshot(logout)
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let again = request(Method::GET, "/__remote/session", Some(&cookie), Value::Null);
        assert_eq!(
            server.router.clone().oneshot(again).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn login_is_rate_limited_and_body_limits_reject_large_commands() {
        let server = server();
        let cookie = login_cookie(&server.router).await;
        let mut large = request(Method::POST, "/__remote/invoke", Some(&cookie), Value::Null);
        *large.body_mut() = Body::from(vec![b'x'; MAX_BODY + 1]);
        assert_eq!(
            server.router.clone().oneshot(large).await.unwrap().status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        for _ in 0..9 {
            let bad = request(
                Method::POST,
                "/__remote/login",
                None,
                json!({"token": "wrong"}),
            );
            assert_eq!(
                server.router.clone().oneshot(bad).await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
        }
        let bad = request(
            Method::POST,
            "/__remote/login",
            None,
            json!({"token": "wrong"}),
        );
        assert_eq!(
            server.router.clone().oneshot(bad).await.unwrap().status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[test]
    fn public_http_and_wildcard_listeners_are_rejected_but_tailnet_http_is_explicitly_supported() {
        let mut config = Config {
            bind: "127.0.0.1:1432".parse().unwrap(),
            origin: "http://100.122.192.100:1432".into(),
            token: "a".repeat(64),
            require_token: true,
        };
        assert!(config.validate().is_ok());
        config.origin = "http://192.168.1.2:1432".into();
        assert!(config.validate().is_err());
        config.origin = "https://host.example".into();
        config.bind = "0.0.0.0:1432".parse().unwrap();
        assert!(config.validate().is_err());
    }

    #[tokio::test]
    async fn disconnected_requests_cannot_bypass_native_work_limits() {
        for (method, path, limit) in [
            (Method::POST, "/__remote/invoke", 32),
            (Method::GET, "/characters/test.png", 16),
        ] {
            let mut server = server();
            let cookie = login_cookie(&server.router).await;
            let mut in_flight = Vec::new();
            for _ in 0..limit {
                let http = tokio::spawn(server.router.clone().oneshot(request(
                    method.clone(),
                    path,
                    Some(&cookie),
                    json!({"command": "get_version"}),
                )));
                in_flight.push(server.requests.recv().await.unwrap());
                // HTTP goes away, but the native operation still owns its slot.
                http.abort();
                assert!(http.await.unwrap_err().is_cancelled());
            }
            let response = server
                .router
                .clone()
                .oneshot(request(
                    method.clone(),
                    path,
                    Some(&cookie),
                    json!({"command": "get_version"}),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert!(server.requests.try_recv().is_err());

            drop(in_flight.pop()); // Native completion releases a slot.
            let http = tokio::spawn(server.router.clone().oneshot(request(
                method,
                path,
                Some(&cookie),
                json!({"command": "get_version"}),
            )));
            match server.requests.recv().await.unwrap() {
                HostRequest::Invoke { reply, .. } => {
                    reply.send(json!({"ok": true, "result": "test"})).unwrap();
                }
                HostRequest::Resource { reply, .. } => {
                    reply
                        .send(
                            Response::builder()
                                .header(header::CACHE_CONTROL, "private, no-cache")
                                .body(vec![1, 2])
                                .unwrap(),
                        )
                        .unwrap();
                }
            }
            let response = http.await.unwrap().unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            if path.ends_with(".png") {
                assert_eq!(
                    response.headers()[header::CACHE_CONTROL],
                    "private, no-cache"
                );
            }
        }
    }
}
