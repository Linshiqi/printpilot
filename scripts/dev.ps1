# Dev launcher: `cargo tauri dev` on ports that do not collide with other local Tauri projects.
#
#   .\scripts\dev.ps1                  # dev server on the port from Trunk.toml, WebView debugging on 17439
#   .\scripts\dev.ps1 -Port 18000      # start probing for a free dev-server port at 18000 instead
#   .\scripts\dev.ps1 -CdpPort 18100   # same for the WebView remote-debugging port
#   .\scripts\dev.ps1 -NoCdp           # do not open a debugging port at all
#
# If a port is busy the script walks upwards to the next free one. When the dev-server port ends up
# different from the one in Trunk.toml / tauri.conf.json, a small override config is written to
# target\dev-override.conf.json and passed to the Tauri CLI with --config, so the repo files never
# need editing. The ports actually used are printed and saved to target\dev-ports.json, which
# scripts\cdp.py reads to find the debugging port.
#
# NOTE: keep this file ASCII-only. Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI (GBK on a
# Chinese system); multi-byte UTF-8 comments can swallow the following line break, silently
# commenting out the next statement.
param(
    [switch]$NoCdp,
    [int]$Port = 0,
    [int]$CdpPort = 0
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

# Some terminals export NO_COLOR=1; trunk maps that env var onto its --no-color flag, which only
# accepts true/false, and then refuses to start.
if (Test-Path Env:\NO_COLOR) { $env:NO_COLOR = $null }

function Test-PortFree([int]$p) {
    try {
        $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $p)
        $listener.Start()
        $listener.Stop()
        return $true
    } catch {
        return $false
    }
}

function Find-FreePort([int]$start) {
    for ($p = $start; $p -lt $start + 100; $p++) {
        if (Test-PortFree $p) { return $p }
    }
    throw "no free TCP port between $start and $($start + 99)"
}

# The port the repo is configured for (Trunk.toml and tauri.conf.json agree; a unit test checks that).
$configured = 0
$match = Select-String -Path (Join-Path $root 'Trunk.toml') -Pattern '^\s*port\s*=\s*(\d+)'
if ($match) { $configured = [int]$match.Matches[0].Groups[1].Value }
if ($configured -eq 0) { throw 'could not read [serve] port from Trunk.toml' }

if ($Port -eq 0) { $Port = $configured }
$devPort = Find-FreePort $Port

$targetDir = Join-Path $root 'target'
New-Item -ItemType Directory -Force $targetDir | Out-Null

$tauriArgs = @('tauri', 'dev')
if ($devPort -ne $configured) {
    $override = Join-Path $targetDir 'dev-override.conf.json'
    $json = '{"build":{"devUrl":"http://localhost:' + $devPort + '","beforeDevCommand":"trunk serve --port ' + $devPort + '"}}'
    [System.IO.File]::WriteAllText($override, $json)
    $tauriArgs += @('--config', $override)
    Write-Host "port $configured is busy -> dev server moves to $devPort for this run"
}

$cdp = 0
if (-not $NoCdp) {
    if ($CdpPort -eq 0) { $CdpPort = 17439 }
    $cdp = Find-FreePort $CdpPort
    # The dev server has not started yet, so its port still looks free: never hand out the same one twice.
    if ($cdp -eq $devPort) { $cdp = Find-FreePort ($devPort + 1) }
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$cdp --remote-allow-origins=*"
}

[System.IO.File]::WriteAllText((Join-Path $targetDir 'dev-ports.json'), ('{"dev":' + $devPort + ',"cdp":' + $cdp + '}'))
Write-Host "dev server: http://localhost:$devPort    webview debugging port: $cdp (0 = off)"

& cargo @tauriArgs
