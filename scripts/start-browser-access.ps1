param([Parameter(Mandatory = $true)][string]$Origin)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$frontend = $null
try {
    if (Get-Process tauritavern -ErrorAction SilentlyContinue) {
        throw 'TauriTavern is already running. Close that instance before starting browser access.'
    }
    if (Get-NetTCPConnection -LocalPort 1432 -State Listen -ErrorAction SilentlyContinue) {
        throw 'Browser access port 1432 is occupied.'
    }
    $binary = Join-Path $repo 'src-tauri\target\debug\tauritavern.exe'
    if (-not (Test-Path $binary)) { throw 'Build the desktop host before starting browser access.' }
    # Non-embedded development builds read these templates beside the executable.
    # Keep the same top-level HTML layout as build.rs's extension resource collector.
    foreach ($extension in Get-ChildItem (Join-Path $repo 'src\scripts\extensions') -Directory) {
        $templates = @(Get-ChildItem $extension.FullName -File -Filter '*.html')
        if (-not $templates.Count) { continue }
        $destination = Join-Path (Split-Path $binary -Parent) ('frontend-extensions\' + $extension.Name)
        $null = New-Item -ItemType Directory -Force -Path $destination
        foreach ($template in $templates) { Copy-Item $template.FullName (Join-Path $destination $template.Name) -Force }
    }
    $tokenFile = Join-Path $env:APPDATA 'com.tauritavern.client\browser-access-token'
    if (-not (Test-Path $tokenFile)) {
        $bytes = New-Object byte[] 32
        $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
        try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }
        [IO.File]::WriteAllText($tokenFile, [BitConverter]::ToString($bytes).Replace('-', '').ToLowerInvariant())
    }
    $configFile = Join-Path (Split-Path $tokenFile -Parent) 'browser-access.json'
    $requireToken = $true
    if (Test-Path $configFile) {
        $savedConfig = [IO.File]::ReadAllText($configFile) | ConvertFrom-Json
        if ($savedConfig.PSObject.Properties.Name -contains 'require_token') {
            if ($savedConfig.require_token -isnot [bool]) { throw 'require_token must be a boolean.' }
            $requireToken = $savedConfig.require_token
        }
    }
    $browserConfig = @{ bind = '127.0.0.1:1432'; origin = $Origin; require_token = $requireToken }
    [IO.File]::WriteAllText($configFile, ($browserConfig | ConvertTo-Json), [Text.UTF8Encoding]::new($false))
    # Child-process settings only; no user/system environment is modified.
    $env:TAURITAVERN_BROWSER_BIND = $browserConfig.bind
    $env:TAURITAVERN_BROWSER_ORIGIN = $browserConfig.origin
    $env:TAURITAVERN_BROWSER_TOKEN_FILE = $tokenFile
    $env:TAURITAVERN_BROWSER_REQUIRE_TOKEN = $requireToken.ToString().ToLowerInvariant()
    Remove-Item Env:TAURITAVERN_ISOLATED_ROOT -ErrorAction SilentlyContinue
    Remove-Item Env:TAURI_CONFIG -ErrorAction SilentlyContinue
    $env:TAURITAVERN_DEV_PORT = '1430'
    $env:TAURITAVERN_DEV_BIND = '127.0.0.1'
    if (-not (Get-NetTCPConnection -LocalPort 1430 -State Listen -ErrorAction SilentlyContinue)) {
        $frontend = Start-Process node.exe -ArgumentList 'scripts/tauri-dev-server.mjs' -WorkingDirectory $repo -WindowStyle Hidden -PassThru
        $deadline = (Get-Date).AddSeconds(60)
        while (-not (Get-NetTCPConnection -LocalPort 1430 -State Listen -ErrorAction SilentlyContinue)) {
            if ($frontend.HasExited -or (Get-Date) -ge $deadline) { throw 'The desktop frontend did not start.' }
            Start-Sleep -Milliseconds 200
        }
    }
    $expectedRelay = [IO.File]::ReadAllText((Join-Path $repo 'src/browser-relay.js'))
    $servedRelay = (Invoke-WebRequest -UseBasicParsing -Uri 'http://127.0.0.1:1430/browser-relay.js').Content
    if ($servedRelay -ne $expectedRelay) { throw 'Port 1430 is not serving this TauriTavern source.' }
    $app = Start-Process $binary -WorkingDirectory $repo -PassThru
    $app.WaitForExit()
    if ($app.ExitCode -ne 0) { throw "TauriTavern exited with code $($app.ExitCode)." }
} catch {
    Add-Type -AssemblyName System.Windows.Forms
    [Windows.Forms.MessageBox]::Show($_.Exception.Message, 'TauriTavern startup failed') | Out-Null
    throw
} finally {
    if ($frontend -and -not $frontend.HasExited) {
        & taskkill.exe /PID $frontend.Id /T /F | Out-Null
    }
}
