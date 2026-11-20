param(
    [switch]$Test,
    [string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath (Split-Path $PSScriptRoot -Parent)
if (-not $env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = Join-Path $env:LOCALAPPDATA 'SDIR-build\target'
}
if ($Test) {
    & cargo test --locked --features desktop --all-targets -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'Tests failed; no artifact was copied.' }
}
. (Join-Path $PSScriptRoot 'windows-release-build.ps1')
Invoke-SdirWindowsReleaseBuild -Cargo 'cargo'
if ($LASTEXITCODE -ne 0) { throw 'Build failed; no artifact was copied.' }
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $output = Join-Path (Get-Location) 'dist'
} else {
    $output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
}
New-Item -ItemType Directory -Force -Path $output | Out-Null
$releaseExecutable = Join-Path $env:CARGO_TARGET_DIR (Join-Path $sdirWindowsReleaseTarget 'release\sdir.exe')
Copy-Item -LiteralPath $releaseExecutable -Destination (Join-Path $output 'SDIR.exe')
Copy-Item -LiteralPath 'README.md','THIRD_PARTY_NOTICES.md','THIRD_PARTY_LICENSES.txt','Cargo.lock' -Destination $output
New-Item -ItemType Directory -Force -Path (Join-Path $output 'docs') | Out-Null
Copy-Item -LiteralPath 'docs\user-journeys.md','docs\verification.md' -Destination (Join-Path $output 'docs')
Write-Host ('Ready: ' + (Join-Path $output 'SDIR.exe'))
