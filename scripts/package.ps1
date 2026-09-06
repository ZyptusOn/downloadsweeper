param([switch]$IncludeCli)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot

# Match run.ps1: migrated Windows installations may have broken rustup proxies.
$toolchainBin = Join-Path $env:USERPROFILE '.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin'
if (Test-Path -LiteralPath (Join-Path $toolchainBin 'cargo.exe')) {
    $env:PATH = $toolchainBin + ';' + $env:PATH
}

Push-Location -LiteralPath $projectRoot
try {
    & cargo run --locked -p ds-dev -- verify-share --project-only
    if ($LASTEXITCODE -ne 0) { throw 'Share verification failed; no package was created.' }
    $metadataText = & cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read Cargo metadata.' }
    $metadata = $metadataText | ConvertFrom-Json
    $version = ($metadata.packages | Where-Object { $_.name -eq 'ds-web' }).version
    $rustInfo = & rustc -vV
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read the Rust target.' }
    $hostTarget = ($rustInfo | Where-Object { $_ -like 'host: *' }) -replace '^host: ', ''
    if ($hostTarget -notmatch '^(x86_64|aarch64)-pc-windows-msvc$') {
        throw 'This script packages native Windows builds. See README.md for macOS.'
    }
    $architecture = $Matches[1]

    $arguments = @('build', '--release', '--locked', '-p', 'ds-web', '--message-format=json-render-diagnostics')
    if ($IncludeCli) { $arguments += @('-p', 'ds-cli') }
    $executables = @{}
    & cargo @arguments | ForEach-Object {
        $message = $_ | ConvertFrom-Json
        if ($message.reason -eq 'compiler-artifact' -and $message.executable) {
            $executables[$message.target.name] = $message.executable
        }
    }
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed; no package was created.' }
    $names = @('ds-web')
    if ($IncludeCli) { $names += 'ds' }
    foreach ($name in $names) {
        if (-not $executables.ContainsKey($name) -or -not (Test-Path -LiteralPath $executables[$name])) {
            throw "Missing release executable: $name"
        }
    }

    # Fresh directories and an explicit allowlist keep user config, tasks and caches out.
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss-fff'
    $packageName = "DownloadSweeper-$version-windows-$architecture-$stamp"
    $destination = Join-Path $projectRoot "dist\$packageName"
    New-Item -ItemType Directory -Path $destination | Out-Null
    foreach ($name in $names) {
        Copy-Item -LiteralPath $executables[$name] -Destination (Join-Path $destination "$name.exe")
    }
    Copy-Item -LiteralPath 'config.example.toml' -Destination $destination
    Copy-Item -LiteralPath '.env.example' -Destination $destination
    Copy-Item -LiteralPath 'docs/PORTABLE.md' -Destination (Join-Path $destination 'README.md')
    Copy-Item -LiteralPath 'docs/MULTIMODAL.md' -Destination $destination
    Copy-Item -LiteralPath 'docs/MACOS.md' -Destination $destination
    Copy-Item -LiteralPath 'docs/MODEL_CONNECTIONS.md' -Destination $destination
    Copy-Item -LiteralPath 'docs/AGENT_RUNTIME.md' -Destination $destination
    Copy-Item -LiteralPath 'docs/PRICING.md' -Destination $destination
    Copy-Item -LiteralPath 'docs/ARCHIVES_AND_CLEANUP.md' -Destination $destination
    Copy-Item -LiteralPath 'docs/CHECKPOINTS.md' -Destination $destination
    Copy-Item -LiteralPath 'LICENSE' -Destination $destination
    Copy-Item -LiteralPath 'frontend/vendor/LICENSES.txt' -Destination (Join-Path $destination 'THIRD_PARTY_LICENSES.txt')
    $launcher = @'
@echo off
setlocal
pushd "%~dp0"
if not exist "config.toml" copy /Y "config.example.toml" "config.toml" >nul
ds-web.exe --open %*
set "DS_EXIT_CODE=%ERRORLEVEL%"
popd
if not "%DS_EXIT_CODE%"=="0" pause
exit /b %DS_EXIT_CODE%
'@
    [System.IO.File]::WriteAllText((Join-Path $destination 'Start.cmd'), $launcher.Replace("`r`n", "`n").Replace("`n", "`r`n"), [System.Text.Encoding]::ASCII)

    $archive = "$destination.zip"
    & cargo run --locked -p ds-dev -- verify-share --package $destination
    if ($LASTEXITCODE -ne 0) { throw 'Portable directory failed credential verification; no ZIP was created.' }
    Compress-Archive -LiteralPath $destination -DestinationPath $archive -CompressionLevel Optimal
    & cargo run --locked -p ds-dev -- verify-share --package $archive
    if ($LASTEXITCODE -ne 0) { throw 'Portable ZIP failed credential verification; do not share it.' }
    $files = Get-ChildItem -LiteralPath $destination -File
    $manifest = [ordered]@{
        package = $packageName
        version = $version
        target = $hostTarget
        profile = 'release'
        includes_cli = [bool]$IncludeCli
        unpacked_bytes = [long]($files | Measure-Object Length -Sum).Sum
        archive_bytes = (Get-Item -LiteralPath $archive).Length
        archive_sha256 = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
        files = @($files | ForEach-Object {
            [ordered]@{ name = $_.Name; bytes = $_.Length; sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }
        })
    }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath "$destination.manifest.json" -Encoding UTF8
    Write-Output "Package: $archive"
    Write-Output ('Unpacked: {0:N2} MiB; ZIP: {1:N2} MiB' -f ($manifest.unpacked_bytes / 1MB), ($manifest.archive_bytes / 1MB))
}
finally {
    Pop-Location
}
