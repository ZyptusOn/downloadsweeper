param()
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$toolchainBin = Join-Path $env:USERPROFILE '.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin'
if (Test-Path -LiteralPath (Join-Path $toolchainBin 'cargo.exe')) { $env:PATH = $toolchainBin + ';' + $env:PATH }
$previousEncodedFlags = $env:CARGO_ENCODED_RUSTFLAGS
$demoFlags = if ($previousEncodedFlags) { $previousEncodedFlags } elseif ($env:RUSTFLAGS) { ($env:RUSTFLAGS -split '\s+') -join [char]31 } else { '' }
# Embed the MSVC runtime so a presentation machine needs no Visual C++ redistributable.
$env:CARGO_ENCODED_RUSTFLAGS = (@($demoFlags, '-Ctarget-feature=+crt-static') | Where-Object { $_ }) -join [char]31
$demoTarget = 'x86_64-pc-windows-msvc'
Push-Location -LiteralPath $projectRoot
try {
    & cargo build --locked --release --target $demoTarget -p ds-demo -p ds-dev
    if ($LASTEXITCODE -ne 0) { throw 'Demo build failed.' }
    $metadata = (& cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json)
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    $release = Join-Path $metadata.target_directory ($demoTarget + '\release')
    $name = 'DownloadSweeper-Fixed-Desktop-Demo-windows-x86_64-' + (Get-Date -Format 'yyyyMMdd-HHmmss')
    $destination = Join-Path $projectRoot ('dist\' + $name)
    New-Item -ItemType Directory -Path $destination | Out-Null
    Copy-Item -LiteralPath (Join-Path $release 'ds-demo.exe') -Destination $destination
    Copy-Item -LiteralPath 'docs/OFFLINE_DEMO.md' -Destination (Join-Path $destination 'README.md')
    Copy-Item -LiteralPath 'LICENSE' -Destination $destination
    Copy-Item -LiteralPath 'frontend/vendor/LICENSES.txt' -Destination (Join-Path $destination 'THIRD_PARTY_LICENSES.txt')
    $launcher = @'
@echo off
setlocal
pushd "%~dp0"
ds-demo.exe
set "DS_EXIT_CODE=%ERRORLEVEL%"
popd
if not "%DS_EXIT_CODE%"=="0" pause
exit /b %DS_EXIT_CODE%
'@
    [System.IO.File]::WriteAllText((Join-Path $destination 'Start-Demo.cmd'), $launcher.Replace("`r`n", "`n").Replace("`n", "`r`n"), [System.Text.Encoding]::ASCII)
    [System.IO.File]::WriteAllText((Join-Path $destination 'Reset-Demo.cmd'), $launcher.Replace('ds-demo.exe', 'ds-demo.exe --reset').Replace("`r`n", "`n").Replace("`n", "`r`n"), [System.Text.Encoding]::ASCII)
    & (Join-Path $release 'ds-dev.exe') verify-share --root $projectRoot --package $destination
    if ($LASTEXITCODE -ne 0) { throw 'Demo credential verification failed.' }
    $archive = "$destination.zip"
    Compress-Archive -LiteralPath $destination -DestinationPath $archive -CompressionLevel Optimal
    & (Join-Path $release 'ds-dev.exe') verify-share --root $projectRoot --package $archive
    if ($LASTEXITCODE -ne 0) { throw 'ZIP credential verification failed.' }
    [ordered]@{package=$name;model='offline-scripted';real_api_cost=0;sha256=(Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash;bytes=(Get-Item -LiteralPath $archive).Length} | ConvertTo-Json | Set-Content -LiteralPath "$destination.manifest.json" -Encoding UTF8
    Write-Output "Package: $archive"
} finally { $env:CARGO_ENCODED_RUSTFLAGS = $previousEncodedFlags; Pop-Location }
