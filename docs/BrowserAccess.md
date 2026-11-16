# Private phone browser access

## Ownership and trust boundary

This is an optional transport in the **existing TauriTavern desktop process**.
It does not start a SillyTavern Node backend or a second writer against TT data.
The phone runs the existing TT frontend, `/api` interceptors and route adapters.
The loopback HTTP listener forwards an explicit command allowlist to a hidden,
local `browser-relay` webview. That webview calls the normal Tauri `invoke` API:
Tauri's invoke key, capability checks, command handlers and existing AppState
remain in the path. Its native resource table is separate from the desktop UI.

`tt-adapter-browser` owns HTTP/authentication only. Tauri-bound dispatch and
resource delivery live in `tauritavern/src/app/host/browser_access`.
Images, thumbnails, extension assets, range requests and user CSS reuse
`HostResourceService`; frontend assets use Tauri's asset resolver. There is no
filesystem plugin, arbitrary path, arbitrary JavaScript evaluation or debug port
exposed to the phone.

## Persistent configuration

Browser access can be enabled once in the native app's configuration directory.
On Windows, save `%APPDATA%\com.tauritavern.client\browser-access.json`:

```json
{
  "bind": "127.0.0.1:1432",
  "origin": "http://100.122.192.100:1432"
}
```

Keep the existing private `browser-access-token` file beside this configuration.
The JSON contains no token. Token authentication is the default. For an entry
protected by the existing private Tailscale network, set `"require_token": false`
in this JSON to enter without typing a code. This mode requires a loopback or
literal Tailscale IPv4 or full MagicDNS node origin (`node.tailnet.ts.net`)
and still requires the loopback listener. Only the exact configured origin is
accepted; this does not allow arbitrary `*.ts.net` request origins. It creates
the existing HttpOnly, SameSite session automatically when opening `/`; an expired
session or a desktop restart no longer requires manual login. Host/Origin checks,
native permissions and write protections remain in place. Keep the old token file
for returning to token mode; token-free mode does not read or rotate it.

Normal desktop launches, `pnpm dev`, and native
development restarts all load this configuration without a special launcher.
The browser listener still belongs to the same desktop process and data owner.
The app identifier determines the configuration directory; isolated preview
builds do not read the normal desktop identifier's saved settings.

The existing `start-browser-access.ps1 -Origin <origin>` launcher also saves this
configuration for subsequent normal launches. Explicit `TAURITAVERN_BROWSER_*`
process settings retain precedence for existing launchers and isolated previews.
The launcher preserves the saved `require_token` choice. Explicit environment
configuration can use `TAURITAVERN_BROWSER_REQUIRE_TOKEN=false`; omission keeps
token authentication enabled.
Invalid explicit settings fail rather than falling back to the saved file.
Without either a saved file or an explicit bind setting, browser access remains
disabled. To disable saved access, remove `browser-access.json` and restart TT
without browser environment overrides. No user or system environment changes
are required.

Phone and desktop browsers use the same authenticated `1432` entry. Port `1430`
is the native desktop's development frontend server; it does not provide browser
authentication or the native backend transport.

The current shared browser URL is
`http://agent-workspace.tail63abeb.ts.net:1432/`. Tailscale clients resolve it
through MagicDNS. On the Hub Windows computer, its exact hosts entry points to
`127.0.0.1`, where the existing SSH forward reaches the same TT instance.
This provides one browser origin with userspace Tailscale; it does not expose
another listener or change Tailscale routes. The Windows DNS Client service must
be able to read its hosts file. Hub configuration uses this URL with an empty
`tauritavern_local_url`, so the button no longer substitutes a loopback URL.

## Supported, bounded workflow

- Startup, character/group lists and existing chat reads, including cold swipes.
- Chat UI settings, presets, world information and model connection profiles.
- Chat-completion connection/status, normal and streaming generation, stop/cancel.
- Atomic chat commits with raw byte frames, metadata and settings saves.
- Browser-loadable resources, backend events and Channel/binary transport.

Native administration remains desktop-only: changing data directories, app
updates, sync/pairing, native pickers, filesystem access, extension installation,
dev logs, Agent/MCP execution and archive operations are not network capabilities.
An unsupported command fails explicitly; it is not routed to another backend.
Both authentication modes grant the same *data owner*, not a separate user account.
Do not concurrently edit the same chat/settings from phone and desktop: this is
remote access, not a collaborative editor with conflict resolution.

### Browser caching

First-party frontend assets use `Cache-Control: private, no-cache` and a validator
of the delivered bytes and MIME type. The browser stores these responses locally
and revalidates them; unchanged responses return bodyless `304`, while updated
files return their new bytes. User files and extensions keep the existing
`HostResourceService` cache and conditional-request contract.

The browser transport also uses argument-free, authenticated GET routes for
`get_bootstrap_snapshot` and `get_sillytavern_settings`. These large startup reads
still run through the same native host and command policy on every request. Only
successful replies are cacheable, and the validator reflects the current reply.
Unchanged settings can therefore reuse the local response without retransmitting
the entire snapshot. Calls with arguments or invoke headers retain normal POST.
Writes, authentication, errors and other commands remain uncached.

This is HTTP browser caching, not an offline backend or a Service Worker. Changed
data, a cleared browser cache, or a browser that does not retain a response requires
a new download. External CDN resources retain their own server cache policies.

### Script and extension preferences

Legacy card/extension preferences written to `localStorage` are shared through
the existing account settings and settings-save queue. The native window seeds
`accountStorage.__ttSharedLocalStorageV1` from its current origin once; a browser
cannot seed that record from phone-local defaults. Reload the desktop first when
upgrading an existing installation, then reload the browser entry.

Account settings hydrate the real local storage before extensions and character
scripts start. Normal Storage writes and same-origin iframe storage events feed
the existing settings save and lifecycle flush. Changes are read from the shared
settings on the next page load; scripts which cache configuration in JavaScript
memory also require a reload.

Device geometry, native bootstrap caches, chat drafts, execution locks, caches,
rollback journals and session snapshots retain their original local scope.
`sessionStorage` is not replicated. The browser connection token/cookie is not
part of this migration. Script settings can contain independent API credentials;
never print/export the shared record into logs or public diagnostic artifacts.

New custom/private model endpoints still require TT's existing native endpoint
authorization. Approve the normal prompt on the computer; browser access does
not bypass that gate or silently grant network access.

## Isolated preview (Windows, no real data)

Use only the snapshot on branch `feat/remote-browser`. Never point it at the live
repository's data or reuse its identifier.

```powershell
powershell -NoProfile -File E:\Projects\TauriTavern-WebAccess\scripts\browser-preview.ps1
```

The script requires `.git/agent-source-snapshot.json` and sets:

| Item | Isolated value |
| --- | --- |
| Identifier | `com.tauritavern.browser-preview` |
| App/data/log root | `E:\Projects\TauriTavern-WebAccess\.browser-preview` |
| User data | `.browser-preview\data` |
| Upload/webview cache | `.browser-preview\cache` |
| Native build output | `.remote-target` |
| Frontend dev port | `1431` |
| Real browser transport | `127.0.0.1:1432` |

Open `http://127.0.0.1:1432`, then paste the token from
`.browser-preview\access-token`. **Port 1431 is only the local desktop frontend,
not a functioning browser backend.** The script does not print the token.
The normal desktop identifier cannot use `TAURITAVERN_ISOLATED_ROOT`; startup
fails rather than accidentally reading its runtime migration configuration.

After `-NativeOnly build` finishes successfully, `-LaunchBuilt` starts that
isolated executable with the existing frontend bundles. It refuses occupied
preview ports. Use `-Origin http://100.122.192.100:1432` for the already configured
private Tailscale forwarding entry; this option does not change Tailscale itself.
The native smoke test uses the same origin when `TT_BROWSER_SMOKE_ORIGIN` is set.
It checks the isolated data root before creating any synthetic test content.
With userspace Tailscale, the host may not be able to connect to its own tailnet
IP. Set `TT_BROWSER_SMOKE_URL=http://127.0.0.1:1432` for native protocol probes
while retaining the configured Origin header. That mode does not run `--ui`;
test the private origin in the phone browser instead.

Existing installed dependencies may be linked into the snapshot for read-only
use. Do not install/update through that junction. The preview script disables
pnpm 11's implicit pre-run install. It limits Cargo to one build job because
parallel native compilation can exhaust Windows commit/pagefile capacity.

```powershell
powershell -NoProfile -File E:\Projects\TauriTavern-WebAccess\scripts\browser-preview.ps1 -CheckOnly
```

`-CheckOnly` uses single-threaded Rust tests, omits test debug symbols and
disables incremental native compilation inside `.remote-target`. This avoids
the Windows linker memory pressure and mixed incremental artifacts encountered
during isolated validation; it does not change assertions or production builds.

## Deployment instructions (manual, not performed)

1. Finish native compilation and the isolated smoke test first. Review the
   allowlist and changes. Back up the real TT data with TT closed before replacing
   a real desktop build. Record the existing runtime mode, app root and data root
   from the owner's desktop settings first; do not guess these from the executable
   path. Do not run a second process against that directory.
   The snapshot includes pre-existing uncommitted user work and is not a clean
   upstream checkout. Reconcile the browser-only delta against
   `.git/agent-source-snapshot.json` and the then-current owner source before
   cutover. Do not overwrite newer owner edits with the entire snapshot.
2. For the real desktop build, retain its normal app identifier and normal
   data-root configuration. **Do not set `TAURITAVERN_ISOLATED_ROOT`.**
   Build without the preview config; for the current snapshot:

   ```powershell
   Set-Location E:\Projects\TauriTavern-WebAccess
   $env:npm_config_verify_deps_before_run = 'false'
   $env:CARGO_TARGET_DIR = 'E:\Projects\TauriTavern-WebAccess\.remote-target'
   $env:CARGO_BUILD_JOBS = '1'
   Remove-Item Env:TAURI_CONFIG -ErrorAction SilentlyContinue
   Remove-Item Env:TAURITAVERN_ISOLATED_ROOT -ErrorAction SilentlyContinue
   node scripts/tauri-app.mjs --prepare-frontend build
   ```

   Do not install/update dependencies through a junction. If dependencies need
   changes, replace the junction with a separately installed dependency tree first.
   The resulting normal-identifier release/bundle is under `.remote-target`.
   Do not launch it against real data during verification. At the later approved
   cutover, close the old TT process cleanly and replace its installation, keeping
   the old build for rollback. A portable installation must keep its original
   executable directory/portable marker and runtime configuration; moving a
   portable executable to a new directory changes its app root.
3. Generate a new random 32-byte token, hex encode it, and save it in a private
   file outside served frontend/data directories. Restrict access to your OS
   account. Do not put the token in a URL, command-line argument, screenshot,
   git file or application logs.
4. Save the persistent configuration described above, or launch that single
   desktop process with explicit process-scoped environment settings:

   ```powershell
   $env:TAURITAVERN_BROWSER_BIND = '127.0.0.1:1432'
   $env:TAURITAVERN_BROWSER_ORIGIN = 'http://100.122.192.100:1432'
   $env:TAURITAVERN_BROWSER_TOKEN_FILE = 'C:\private\tt-browser-token'
   & 'C:\path\to\tauritavern.exe'
   ```

    The configured origin must match what the phone opens (including non-default
    port, with no trailing slash). The same listener also accepts its exact HTTP
    loopback address and `http://localhost:<port>` for the computer's browser, so
    the computer does not need to route through its own Tailscale address. These
    local origins require the same token and session checks; other Host/Origin
    values are rejected. Cookies use Secure when the actual login origin is HTTPS.
    Binding to LAN, wildcard or Tailscale interfaces is rejected.
5. **Only after reviewing existing Tailscale Serve configuration**, the owner may
   forward a free, tailnet-only HTTP port to the loopback listener. For example,
   where the existing Tailscale version/policy permits it:

   ```powershell
   tailscale serve status
   tailscale serve --bg --http=1432 http://127.0.0.1:1432
   ```

   Do not run this blindly over an existing Serve entry. Do not enable Funnel,
   router forwarding, a public listener, or change DNS/VPN/security policy.
   Tailscale ACLs must limit access to the intended devices. The phone must use
   the existing Tailscale VPN and open `http://100.122.192.100:1432`.
6. If a working HTTPS hostname/certificate is already available, prefer it:
   set `TAURITAVERN_BROWSER_ORIGIN` to that exact `https://...` origin and use
   the corresponding Tailscale HTTPS Serve entry. This implementation does not
   repair the currently incorrect hostname resolution or assume MagicDNS works.
7. Sign in from the phone using the token. Sessions are HttpOnly, SameSite=Strict,
   expire after eight hours and are in-memory only. HTTPS sessions also use
   Secure cookies. Restarting TT revokes every session. Token rotation requires
   replacing its private file and restarting the one desktop process.
8. To disable access, remove the saved `browser-access.json` if present and
   remove `TAURITAVERN_BROWSER_BIND` from the desktop launch environment, then
   restart that process when safe. Remove only the dedicated
   Serve entry you added (for the HTTP example, `tailscale serve --http=1432 off`;
    confirm the installed version's syntax with `tailscale serve --help`).

For the existing Windows development installation, `scripts/start-browser-access.ps1
-Origin http://100.122.192.100:1432` starts the original development host and its
frontend together and saves the browser settings for later normal launches. It
preserves the normal app identity, runtime configuration and desktop
`localhost:1430` origin. Its environment settings are limited to the child
processes; it does not edit the user or system environment. The token is stored in
`%APPDATA%\com.tauritavern.client\browser-access-token`. The launcher refuses a
second TT process and stops only the frontend it created when TT exits.

HTTP on a literal Tailscale IPv4 address is explicitly supported, but only over
the existing private WireGuard path; it is not general public HTTP support.
The bridge does not require service workers or `crypto.randomUUID`.
It uses `crypto.getRandomValues` to supply UUIDs on insecure browser contexts.
Browser/extension features that inherently require HTTPS (for example some
clipboard APIs or third-party crypto APIs) remain subject to browser policy.
This distinction is intentional: MDN documents
[`getRandomValues`](https://developer.mozilla.org/en-US/docs/Web/API/Crypto/getRandomValues)
as available in insecure contexts, while
[`randomUUID`](https://developer.mozilla.org/en-US/docs/Web/API/Crypto/randomUUID)
requires a secure context. Loopback Chrome smoke tests do not constitute an
on-phone test of the literal tailnet HTTP origin.

## Failure and verification

- Wrong/missing token, expired cookie, wrong Host or Origin and cross-site
  requests fail before reaching the desktop.
- Login attempts, sessions, active invocations, body sizes and event queues are
  bounded. The transport never retries invocations automatically; the existing
  route-level read recovery remains unchanged.
- A command timeout means **outcome unknown**, particularly for a save: inspect
  the chat before retrying. A disconnected event stream blocks further invokes
  and shows a reconnect/reload notice instead of pretending state is current.
  Native work retains its capacity slot after an HTTP disconnect or timeout until
  it actually settles. A stuck relay can therefore exhaust the bounded command
  slots and require a safe desktop restart; reconnecting does not bypass limits.
  Returning through browser history after `pagehide` also requires a reload.
- The desktop and its local relay must remain running. This is not a headless
  replacement backend. Phone suspension/reload requires reconnecting.
- Focused tests: `node --test tests/browser-transport.test.mjs` and
  `cargo test --manifest-path src-tauri/Cargo.toml -p tt-adapter-browser`.
- Native acceptance must additionally verify login, bootstrap, a synthetic
  character/chat, save/read-back, resource bytes, model status and stream/stop
  against a controlled test provider. Adapter/fake-host tests alone are not
  evidence of end-to-end native functionality.
- The executable isolated smoke test is
  `node tests/browser/remote-native-smoke.mjs`; `--ui` additionally uses the
  preinstalled sandboxed Chrome through a local debugging pipe (no debug port).
  `--with-provider` exercises status, streaming and cancel against a synthetic
  localhost provider on port 1433. Its **isolated fixture only** must contain
  `.browser-preview/security/user-endpoint-grants.json` with
  `["http://127.0.0.1:1433/v1"]` before starting that isolated native process.
  No real endpoint, key, or production grant file is used or changed.
- For memory-constrained verification, `scripts/browser-preview.ps1 -NativeOnly check`
  or `-NativeOnly build` starts a detached, single-job native operation with logs
  in `.browser-preview/native-{check,build}.*.log`. These modes do not launch TT.
  The build mode omits host debug symbols (not debug assertions or the isolated
  identifier) to reduce Windows linker memory use. No production profile changes.
  The frontend server can serve previously built bundles without a compiler
  watcher using `TAURITAVERN_DEV_STATIC_ONLY=1`; it is still only port 1431's
  static dev host, never a replacement for port 1432's native transport.

## Isolated validation record

- Resumed verification on 2026-10-20: the current source passes the complete
  `pnpm run check` harness (`.browser-preview/check-resume-final-20261020.log`):
  19 startup/navigation tests, 667 contract tests with one skipped, 175 UI tests,
  and 1,398 Rust tests, followed by workspace Clippy with warnings denied.
  The rebuilt native app also passes the synthetic provider's status/stream/stop
  test and the mobile Chromium UI test. The UI test clicks a recent-chat card,
  verifies rendered history and both swipes, and reads back the remembered chat
  from the native character repository. The browser entry contains one host marker.
  The latest physical-phone repeat is blocked by the phone's locked/off screen;
  these results do not claim a fresh phone UI test or a live-data deployment.
  `.git/browser-access-review.patch` contains only the browser delta against the
  current original source; its manifest records the expected original hashes.
  The original desktop process remains the sole owner of the real data.
- Focused browser/binary/channel/lifecycle tests, native Android IME regression
  tests and browser clipboard preservation: 11 passed. HTTP authentication,
  revoked event readers and native work bounds: 6 Rust tests passed.
- Full `pnpm run check` passed with `CARGO_PROFILE_TEST_DEBUG=0`,
  `CARGO_INCREMENTAL=0`, `RUST_TEST_THREADS=1`, one Cargo build job, offline Cargo
  and pnpm's implicit dependency installation disabled: 19 startup/navigation
  tests, 665 contract tests (one skipped), 175 UI tests and 1,399 Rust tests;
  all guardrails, types, lint and workspace Clippy also passed.
- Earlier normal-debug attempts hit Windows compiler/linker memory exhaustion,
  one transient dependency `lstat` failure, and an incremental artifact-format
  failure. These are recorded in `.browser-preview`; they were not fixed by
  modifying the original repository or its dependency/cache trees.
- A real isolated native process passed `remote-native-smoke.mjs --with-provider`:
  authentication, bootstrap, synthetic character/chat save/read-back, resource
  delivery, model status, streamed token delivery and provider disconnection on
  session close. No external model or real credentials were used.
- `remote-native-smoke.mjs --ui` passed in the preinstalled sandboxed Chrome with
  a mobile viewport/user agent and synthetic data, including first-run onboarding.
  It waits for the actual `APP_READY` event and removal of the startup overlay,
  not just early character availability. Read-only extension settings discovery
  is allowed so bundled panels cannot interrupt that startup event.
  Native Android insets routing is not installed in the phone browser.
- This is not a physical-phone/Tailscale test or production deployment. Use the
  configured direct origin; generic preview proxies that change it are rejected.

## `/goal`
/goal Deliver approved private phone access from the isolated TT source snapshot to its existing native data owner via a bounded authenticated transport, source/tests and isolated preview; preserve desktop IPC/ACL and user edits, never blindly retry uncertain writes, and do not deploy production/Tailscale, use real test data, replace the backend or weaken permissions.
