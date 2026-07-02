<#
.SYNOPSIS
    Build Legwork.msi, a Windows installer for Legwork.

.DESCRIPTION
    Builds an optimized x86_64 release binary (with the app icon embedded by
    build.rs) and wraps it in an MSI via cargo-wix + the WiX 3 Toolset. The MSI
    installs Legwork to Program Files, adds a Start Menu shortcut, and registers
    it in Add/Remove Programs with the app icon.

    Output: target\wix\legwork-<version>-x86_64.msi

.NOTES
    Requires the Rust toolchain (x86_64-pc-windows-msvc). cargo-wix and the WiX 3
    binaries are installed automatically on first run if missing.

.EXAMPLE
    scripts\windows\bundle-windows.ps1
#>
[CmdletBinding()]
param(
    # Skip the cargo build (reuse an existing target\release\legwork.exe).
    [switch]$NoBuild
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $root

function Info($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }

# --- Ensure cargo-wix is installed --------------------------------------------
if (-not (cargo install --list 2>$null | Select-String -SimpleMatch 'cargo-wix')) {
    Info 'Installing cargo-wix (one-time, compiles from source)'
    cargo install cargo-wix
}

# --- Ensure the WiX 3 Toolset is available ------------------------------------
# cargo-wix looks for candle.exe/light.exe under "$env:WIX\bin". Use an existing
# install if WIX is already set; otherwise bootstrap the standalone binaries into
# LOCALAPPDATA (no admin / .NET 3.5 installer needed — they run on .NET 4.x).
function Test-Wix($base) { $base -and (Test-Path (Join-Path $base 'bin\candle.exe')) }

if (-not (Test-Wix $env:WIX)) {
    $wixHome = Join-Path $env:LOCALAPPDATA 'WiX314'
    if (-not (Test-Wix $wixHome)) {
        Info 'WiX 3 Toolset not found; downloading standalone binaries'
        $ver = 'wix3141rtm'
        $url = "https://github.com/wixtoolset/wix3/releases/download/$ver/wix314-binaries.zip"
        $zip = Join-Path $env:TEMP 'wix314-binaries.zip'
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
        $bin = Join-Path $wixHome 'bin'
        New-Item -ItemType Directory -Force -Path $bin | Out-Null
        Expand-Archive -Path $zip -DestinationPath $bin -Force
        Remove-Item $zip -Force
    }
    $env:WIX = $wixHome
}
$env:Path = "$(Join-Path $env:WIX 'bin');$env:Path"
Info "Using WiX at $env:WIX"

# --- Build the release binary -------------------------------------------------
if (-not $NoBuild) {
    Info 'Building release binary (x86_64-pc-windows-msvc)'
    cargo build --release
}
if (-not (Test-Path 'target\release\legwork.exe')) {
    throw 'target\release\legwork.exe not found — run without -NoBuild first.'
}

# --- Build the MSI ------------------------------------------------------------
Info 'Building MSI installer'
cargo wix --no-build --nocapture

$msi = Get-ChildItem 'target\wix\*.msi' | Sort-Object LastWriteTime | Select-Object -Last 1
Info "Done: $($msi.FullName)"
Write-Host ("     {0:N1} MB" -f ($msi.Length / 1MB))
