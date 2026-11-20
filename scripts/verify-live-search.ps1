param([string]$Executable="$PSScriptRoot\..\docs\task-id\live-search\SDIR-search-20261117.exe",[switch]$Large,[switch]$SearchPreferences,[string]$EvidenceDirectory='')
$ErrorActionPreference='Stop'
Set-Location -LiteralPath (Split-Path $PSScriptRoot -Parent)
$exe=(Resolve-Path -LiteralPath $Executable).Path
$expected=(Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Host 'Full-drive runtime test requires administrator authentication in up to three Windows UAC prompts.'
Write-Host 'Use the Windows prompt only. Do not send credentials in chat. Cancellation leaves this test incomplete.'
$node=(Get-Command node -ErrorAction Stop).Source
$evidence=if($EvidenceDirectory){[IO.Path]::GetFullPath($EvidenceDirectory)}else{Join-Path (Get-Location) 'docs\task-id\live-search\actual-live'}
New-Item -ItemType Directory -Path $evidence -Force|Out-Null
$runRoot=Join-Path ([IO.Path]::GetTempPath()) ('sdir-test-run-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $runRoot|Out-Null
[IO.File]::WriteAllText((Join-Path $runRoot '.sdir-audit-fixture'),'owned')
@{owner='SDIR elevated journey';created=[DateTime]::UtcNow.ToString('o')}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $runRoot '.sdir-test-run.json') -Encoding UTF8
$previous=$env:SDIR_TEST_FIXTURE_ROOT;$env:SDIR_TEST_FIXTURE_ROOT=$runRoot;$code=1
try {
 $extra=@('--live');if($Large){$extra+='--large'};if($SearchPreferences){$extra+='--preferences'}
 & $node (Join-Path $PSScriptRoot 'search-smoke.mjs') $exe @extra
 if(Test-Path -LiteralPath 'docs\task-id\live-search\live-search-journey.json'){Copy-Item -LiteralPath 'docs\task-id\live-search\live-search-journey.json' -Destination (Join-Path $evidence 'last-attempt.json')}
 if($LASTEXITCODE -ne 0){throw 'Actual full-volume journey failed or Windows consent was not provided.'}
 $result=Get-Content -Raw -LiteralPath 'docs\task-id\live-search\live-search-journey.json'|ConvertFrom-Json
 if(-not $result.ok -or $result.sha256 -ne $expected){throw 'Actual journey did not verify this executable.'}
 Copy-Item -LiteralPath 'docs\task-id\live-search\live-search-journey.json' -Destination (Join-Path $evidence 'verified-journey.json')
 $code=0
} catch {Write-Error -Message $_.Exception.Message -ErrorAction Continue}
finally {
 $env:SDIR_TEST_FIXTURE_ROOT=$previous
 $inventory=Join-Path $evidence 'fixture-cleanup.json'
 @{directories=@(@{path=$runRoot;eligible=$true;markers=@('.sdir-audit-fixture');references=@()})}|ConvertTo-Json -Depth 5|Set-Content -LiteralPath $inventory -Encoding UTF8
 $deadline=[DateTime]::UtcNow.AddSeconds(12)
 do {
  $active=@(Get-CimInstance Win32_Process|Where-Object{($_.ExecutablePath -and $_.ExecutablePath.StartsWith($runRoot+'\',[StringComparison]::OrdinalIgnoreCase)) -or ($_.CommandLine -and $_.CommandLine.Contains($runRoot))})
  if(-not $active.Count){break};Start-Sleep -Milliseconds 400
 }while([DateTime]::UtcNow -lt $deadline)
 $reportPath='docs\task-id\live-search\live-search-journey.json';$hosts=@()
 if(Test-Path -LiteralPath $reportPath){$last=Get-Content -Raw -LiteralPath $reportPath|ConvertFrom-Json;if($last.fixture -and $last.fixture.StartsWith($runRoot+'\',[StringComparison]::OrdinalIgnoreCase)){$hosts=@($last.observedHosts|Where-Object{$_}|ForEach-Object{Get-Process -Id $_ -ErrorAction SilentlyContinue}|Where-Object{$_.ProcessName -match '^SDIR'})}}
 if($hosts.Count){Write-Warning 'Elevated owned host still exists; preserve its fixture and investigate.';$code=1}
 else {try {& (Join-Path $PSScriptRoot 'cleanup-development.ps1') -Action Inspect -Inventory $inventory;if($LASTEXITCODE -eq 0){& (Join-Path $PSScriptRoot 'cleanup-development.ps1') -Action Apply -Inventory $inventory}}catch{Write-Warning $_.Exception.Message}}
 @{ok=($code -eq 0);exitCode=$code;sha256=$expected;executable=$exe}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $evidence 'runner-result.json') -Encoding UTF8
}
exit $code
