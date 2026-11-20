param([string]$Executable = "$PSScriptRoot\SDIR.exe", [switch]$CheckOnly)
$ErrorActionPreference = 'Stop'
$runtime = $null
# Microsoft documents the product GUID and pv value; display names can be localized.
$product = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
foreach ($root in @('HKCU:\Software\Microsoft\EdgeUpdate\Clients','HKLM:\Software\Microsoft\EdgeUpdate\Clients','HKLM:\Software\WOW6432Node\Microsoft\EdgeUpdate\Clients')) {
    $key = Join-Path $root $product
    if (Test-Path -LiteralPath $key) {
        $version = (Get-ItemProperty -LiteralPath $key -Name pv -ErrorAction SilentlyContinue).pv
        if ($version -and $version -match '^\d+\.\d+\.\d+\.\d+$' -and [version]$version -gt [version]'0.0.0.0') { $runtime = $version }
    }
}
if (-not $runtime) {
    Write-Host 'Microsoft Edge WebView2 Runtime is required. Install Evergreen Runtime from:'
    Write-Host 'https://developer.microsoft.com/microsoft-edge/webview2/'
    exit 1
}
if ($CheckOnly) { Write-Host "WebView2 Runtime: $runtime"; exit 0 }
if (-not (Test-Path -LiteralPath $Executable)) { throw 'SDIR.exe was not found beside this launcher.' }
Start-Process -FilePath (Resolve-Path -LiteralPath $Executable).Path
