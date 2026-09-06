param([int]$Port = 3187, [switch]$Demo, [switch]$DesktopDemo, [switch]$NoOpen)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $projectRoot
$runningBinary = Join-Path $projectRoot 'target\debug\ds-web.exe'
$runningServer = Get-Process -Name 'ds-web' -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $runningBinary }
if ($runningServer) {
    throw 'This project is already running. Stop its server before rebuilding, or open its existing browser URL.'
}
# Some migrated Windows installations have broken rustup proxy symlinks.
$toolchainBin = Join-Path $env:USERPROFILE '.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin'
if (Test-Path -LiteralPath (Join-Path $toolchainBin 'cargo.exe')) {
    $env:PATH = $toolchainBin + ';' + $env:PATH
}
$arguments = @('run', '--locked', '-p', 'ds-web', '--', '--port', "$Port")
if (-not $env:DS_CONFIG) { $env:DS_CONFIG = Join-Path $projectRoot 'config.toml' }
if (-not $env:DS_DATA_DIR) { $env:DS_DATA_DIR = Join-Path $projectRoot '.ds-data' }
if ($Demo -or $DesktopDemo) {
    $demoArgs = @('-B', 'scripts/create_demo.py')
    if ($DesktopDemo) { $demoArgs += '--desktop' }
    python @demoArgs
    if ($LASTEXITCODE -ne 0) { throw '演示目录创建失败' }
    $arguments += @('--data-dir', 'artifacts/demo-data', '--config', 'artifacts/test-config.toml')
}
if (-not $NoOpen) { $arguments += '--open' }
& cargo @arguments
exit $LASTEXITCODE
