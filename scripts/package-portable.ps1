param([string]$Executable = "$PSScriptRoot\..\dist\SDIR.exe",[string]$OutputPath = "$PSScriptRoot\..\dist\SDIR-windows-x64-portable.zip")
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath (Split-Path $PSScriptRoot -Parent)
$exe = (Resolve-Path -LiteralPath $Executable).Path
$expected = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
$stage = Join-Path ([IO.Path]::GetTempPath()) ('sdir-package-' + [Guid]::NewGuid().ToString('N'))
$folder = Join-Path $stage 'SDIR'
New-Item -ItemType Directory -Path (Join-Path $folder 'docs') -Force | Out-Null
Copy-Item -LiteralPath $exe -Destination (Join-Path $folder 'SDIR.exe')
foreach ($file in @('README.md','THIRD_PARTY_NOTICES.md','THIRD_PARTY_LICENSES.txt','Cargo.lock')) { Copy-Item -LiteralPath $file -Destination $folder }
foreach ($file in @('user-journeys.md','verification.md','reference-features.md','distribution.md','shortcuts.md','remaining-features.md','test-summary.md','performance-improvement.md','performance-comparison.md','development-storage.md','everything-analysis.md','search-settings.md')) { Copy-Item -LiteralPath (Join-Path 'docs' $file) -Destination (Join-Path $folder 'docs') }
Copy-Item -LiteralPath 'scripts\check-runtime.ps1','packaging\Start-SDIR.cmd' -Destination $folder
[IO.File]::WriteAllText((Join-Path $folder 'portable.flag'),'Portable profile: data\settings.json')
@{executableSha256=$expected;architecture='windows-x64';profile='data';webview2='Evergreen Runtime required';userDataIncluded=$false} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $folder 'manifest.json') -Encoding UTF8
$output = [IO.Path]::GetFullPath($OutputPath)
New-Item -ItemType Directory -Force -Path (Split-Path $output -Parent) | Out-Null
$temp = $output + '.new.zip'
Compress-Archive -LiteralPath $folder -DestinationPath $temp -Force
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::OpenRead($temp)
$entries = @{}
foreach ($item in $zip.Entries) { $entries[$item.FullName.Replace('\','/')] = $item }
foreach ($required in @('SDIR/docs/test-summary.md','SDIR/manifest.json','SDIR/Start-SDIR.cmd','SDIR/check-runtime.ps1')) { if ($null -eq $entries[$required]) { $zip.Dispose(); throw "Portable required file missing: $required" } }
$entry = $entries['SDIR/SDIR.exe']
if ($null -eq $entry) { $zip.Dispose(); throw 'Portable executable entry is missing.' }
$stream = $entry.Open()
$hash = [Security.Cryptography.SHA256]::Create()
$actual = [BitConverter]::ToString($hash.ComputeHash($stream)).Replace('-','').ToLowerInvariant()
$stream.Dispose(); $hash.Dispose()
if ($actual -ne $expected -or $null -eq $entries['SDIR/portable.flag']) { $zip.Dispose(); throw 'Portable archive validation failed.' }
foreach ($entry in $zip.Entries) { if ($entry.FullName.Replace('\','/') -match '^SDIR/(data|webview)/') { $zip.Dispose(); throw 'A portable archive must not include private user data.' } }
$zip.Dispose()
Move-Item -LiteralPath $temp -Destination $output -Force
Remove-Item -LiteralPath $stage -Recurse -Force
Write-Host "Portable ZIP: $output"
