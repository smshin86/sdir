param([switch]$Release,[switch]$Tests)
$ErrorActionPreference='Stop'
Set-Location -LiteralPath (Split-Path $PSScriptRoot -Parent)
. (Join-Path $PSScriptRoot 'windows-release-build.ps1')
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR=Join-Path $env:LOCALAPPDATA 'SDIR-build\target' }
$cargo=Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
& $cargo check --locked --features desktop
if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
if($Tests){& $cargo test --locked --features desktop --all-targets -- --test-threads=1;if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}}
if($Release){
    Invoke-SdirWindowsReleaseBuild -Cargo $cargo
    if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
}
