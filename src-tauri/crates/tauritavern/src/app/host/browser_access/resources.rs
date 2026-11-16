use std::sync::Arc;
use tauri::{Manager, http::{Request, Response, StatusCode}};
use tt_application::services::host_resource_service::{HostResourceService, HostResourceDeliveryCapabilities};
use crate::infrastructure::paths::RuntimePaths;

fn error(status: StatusCode) -> Response<Vec<u8>> {
    Response::builder().status(status).body(Vec::new()).expect("resource response")
}

pub(super) fn serve(app: &tauri::AppHandle, mut request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    if request.uri().path() == "/__remote/file" {
        let query: Vec<_> = url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes()).collect();
        let Some((_, path)) = query.iter().find(|(key, _)| key == "path") else {
            return error(StatusCode::BAD_REQUEST);
        };
        let root = app.state::<RuntimePaths>().data_root.join("default-user");
        // convertFileSrc is not arbitrary filesystem access. Translate only known
        // browser asset roots back through the existing HostResourceService.
        let path = path.replace('\\', "/");
        let root = root.to_string_lossy().replace('\\', "/");
        let Some(relative) = path.strip_prefix(&format!("{root}/")) else {
            return error(StatusCode::FORBIDDEN);
        };
        if !["characters/", "backgrounds/", "User Avatars/", "assets/", "user/images/", "user/files/"]
            .iter().any(|prefix| relative.starts_with(prefix))
        {
            return error(StatusCode::FORBIDDEN);
        }
        let encoded = relative.split('/').map(|part|
            percent_encoding::utf8_percent_encode(part, percent_encoding::NON_ALPHANUMERIC).to_string()
        ).collect::<Vec<_>>().join("/");
        let Ok(uri) = format!("/{encoded}").parse() else { return error(StatusCode::BAD_REQUEST) };
        *request.uri_mut() = uri;
    }
    let service = app.state::<Arc<HostResourceService>>();
    if let Some(response) = service.try_serve(&request, HostResourceDeliveryCapabilities::new(true, false)) {
        return response;
    }
    if request.method() != "GET" && request.method() != "HEAD" {
        return error(StatusCode::METHOD_NOT_ALLOWED);
    }
    let decoded = match percent_encoding::percent_decode_str(request.uri().path()).decode_utf8() {
        Ok(path) => path,
        Err(_) => return error(StatusCode::BAD_REQUEST),
    };
    // AssetResolver's development filesystem fallback does not reject '..'.
    // Reject it here before crossing that boundary; never serve source outside src.
    if decoded.contains('\\') || decoded.contains(':') || decoded.contains('\0')
        || decoded.split('/').any(|part| part == ".." || part.starts_with('.'))
    {
        return error(StatusCode::FORBIDDEN);
    }
    let path = if decoded == "/" { "index.html" } else { decoded.trim_start_matches('/') };
    // The relay page is local-only, never a remote browser document.
    if path == "browser-relay.html" || path == "browser-relay.js" {
        return error(StatusCode::NOT_FOUND);
    }
    let resolver = app.asset_resolver();
    // Tauri's production resolver falls back to index.html for missing assets.
    // HTTP subresources must be exact matches, including the local-only relay.
    let asset = if cfg!(feature = "custom-protocol") || app.config().build.dev_url.is_none() {
        resolver
            .iter()
            .find(|(key, _)| key.as_ref() == format!("/{path}"))
            .map(|(_, bytes)| (
                bytes.into_owned(),
                mime_guess::from_path(path).first_or_octet_stream().to_string(),
            ))
    } else {
        resolver.get(path.to_owned()).map(|asset| (asset.bytes, asset.mime_type))
    };
    let Some((bytes, mime_type)) = asset else {
        return error(StatusCode::NOT_FOUND);
    };
    let bytes = if path == "index.html" {
        let Ok(html) = String::from_utf8(bytes) else {
            return error(StatusCode::INTERNAL_SERVER_ERROR);
        };
        if !html.contains("<head>") {
            return error(StatusCode::INTERNAL_SERVER_ERROR);
        }
        html.replacen("<head>", "<head><meta name=\"tauritavern-browser-host\" content=\"1\">", 1).into_bytes()
    } else {
        bytes
    };
    tt_adapter_browser::cached_response(&request, bytes, &mime_type)
}
