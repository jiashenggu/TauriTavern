param(
    [switch]$CheckOnly,
    [ValidateSet('check', 'build')][string]$NativeOnly,
    [switch]$LaunchBuilt,
    [string]$Origin = 'http://127.0.0.1:1432'
)
$ErrorActionPreference = 'Stop'
if (([int]$CheckOnly.IsPresent + [int]$LaunchBuilt.IsPresent + [int][bool]$NativeOnly) -gt 1) {
    throw 'Choose only one of -CheckOnly, -NativeOnly, or -LaunchBuilt.'
}
$repo = Split-Path $PSScriptRoot -Parent
if (-not (Test-Path (Join-Path $repo '.git/agent-source-snapshot.json'))) {
    throw 'Run this only in the isolated source snapshot.'
}
$root = Join-Path $repo '.browser-preview'
$null = New-Item -ItemType Directory -Force -Path $root
$null = New-Item -ItemType Directory -Force -Path (Join-Path $root 'cache')
$tokenFile = Join-Path $root 'access-token'
if (-not (Test-Path $tokenFile)) {
    $bytes = New-Object byte[] 32
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    $rng.GetBytes($bytes)
    $rng.Dispose()
    $token = [BitConverter]::ToString($bytes).Replace('-', '').ToLowerInvariant()
    [IO.File]::WriteAllText($tokenFile, $token)
}
$env:TAURITAVERN_ISOLATED_ROOT = $root
$env:TAURITAVERN_BROWSER_BIND = '127.0.0.1:1432'
$env:TAURITAVERN_BROWSER_ORIGIN = $Origin
$env:TAURITAVERN_BROWSER_TOKEN_FILE = $tokenFile
$env:TAURITAVERN_DEV_PORT = '1431'
$env:TAURITAVERN_DEV_BIND = '127.0.0.1'
$env:CARGO_TARGET_DIR = Join-Path $repo '.remote-target'
$env:CARGO_BUILD_JOBS = '1'
# pnpm 11 otherwise tries an implicit install before running scripts. The
# dependency junction is for reads only; synchronization must never run here.
$env:npm_config_verify_deps_before_run = 'false'
$env:HTTP_PROXY = ''
$env:HTTPS_PROXY = ''
$env:ALL_PROXY = ''
$env:TAURITAVERN_RUNTIME_MODE = 'standard'
$env:PATH = "C:\Program Files\Git\usr\bin;$env:PATH"
$env:RUST_TEST_THREADS = '1'
Set-Location $repo
Write-Host "Preview only. Data/logs: $root ; cache: $root\cache ; frontend:1431 ; browser:1432"
Write-Host "Token file (do not share in logs): $tokenFile"
if ($LaunchBuilt) {
    $binary = Join-Path $repo '.remote-target/debug/tauritavern.exe'
    if (-not (Test-Path $binary)) { throw 'Build the isolated native app first with -NativeOnly build.' }
    foreach ($port in @(1431, 1432)) {
        if (Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue) {
            throw "Preview port $port is already in use. No processes were started."
        }
    }
    $env:TAURITAVERN_DEV_STATIC_ONLY = '1'
    $front = Start-Process -FilePath 'node.exe' -ArgumentList @('scripts/tauri-dev-server.mjs') -WorkingDirectory $repo -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $root 'frontend.out.log') -RedirectStandardError (Join-Path $root 'frontend.err.log')
    try {
        Start-Sleep -Seconds 2
        if ($front.HasExited) { throw 'Preview frontend failed; inspect .browser-preview/frontend.err.log.' }
        $app = Start-Process -FilePath $binary -WorkingDirectory $repo -PassThru -RedirectStandardOutput (Join-Path $root 'app.out.log') -RedirectStandardError (Join-Path $root 'app.err.log')
        Start-Sleep -Seconds 2
        if ($app.HasExited) { throw 'Preview app failed; inspect .browser-preview/app.err.log.' }
    } catch {
        if (-not $front.HasExited) { Stop-Process -Id $front.Id }
        throw
    }
    @{ frontend = $front.Id; app = $app.Id; origin = $Origin } | ConvertTo-Json | Set-Content (Join-Path $root 'pids.json')
    Write-Host "Isolated preview running: $Origin (frontend PID $($front.Id), app PID $($app.Id))"
    exit 0
} elseif ($NativeOnly) {
    # Detached build/check survives CLI session interruption; logs and outputs
    # remain inside the isolated snapshot. It never launches an application.
    $env:TAURI_CONFIG = [IO.File]::ReadAllText((Join-Path $repo 'src-tauri/crates/tauritavern/tauri.browser-preview.conf.json'))
    $cargo = Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe'
    $arguments = @($NativeOnly, '--manifest-path', (Join-Path $repo 'src-tauri/Cargo.toml'), '-p', 'tauritavern', '--no-default-features', '--offline', '-j', '1')
    if ($NativeOnly -eq 'build') {
        # The host's large Windows PDB can exceed commit limits even at -j 1.
        # Keep debug assertions and the preview identity; omit only host symbols.
        $arguments += @('--config', 'profile.dev.package.tauritavern.debug=0')
    }
    $process = Start-Process -FilePath $cargo -ArgumentList $arguments -WorkingDirectory $repo -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $root "native-$NativeOnly.out.log") -RedirectStandardError (Join-Path $root "native-$NativeOnly.err.log")
    Write-Host "Started isolated native $NativeOnly PID $($process.Id). Logs: $root\native-$NativeOnly.*.log"
    exit 0
} elseif ($CheckOnly) {
    $env:TAURI_CONFIG = [IO.File]::ReadAllText((Join-Path $repo 'src-tauri/crates/tauritavern/tauri.browser-preview.conf.json'))
    $env:CARGO_PROFILE_TEST_DEBUG = '0'
    $env:CARGO_INCREMENTAL = '0'
    pnpm --config.verify-deps-before-run=false run check
} else {
    node scripts/tauri-app.mjs --prepare-frontend dev --no-watch --config src-tauri/crates/tauritavern/tauri.browser-preview.conf.json
}
exit $LASTEXITCODE
