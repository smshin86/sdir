param([string]$Executable="$PSScriptRoot\..\dist\SDIR.exe",[switch]$Large,[string]$EvidenceDirectory="$PSScriptRoot\..\docs\task-id\journey-audit",[string]$ResumeAt='')
$ErrorActionPreference='Stop'
Set-Location -LiteralPath (Split-Path $PSScriptRoot -Parent)
$exe=(Resolve-Path -LiteralPath $Executable).Path
$expected=(Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
$node=(Get-Command node -ErrorAction Stop).Source
$evidence=$EvidenceDirectory
New-Item -ItemType Directory -Force -Path $evidence|Out-Null
$uiName=if($Large){'ui-large.json'}else{'ui-smoke.json'}
$steps=@(
 @{id='ui';script='ui-smoke.cjs';extra=@(if($Large){'--large'});reports=@(@{source="docs\task-id\sdir-first-app\$uiName";name='final-ui.json'})},
 @{id='recovery';script='recovery-smoke.mjs';extra=@((Join-Path (Resolve-Path -LiteralPath $evidence).Path 'final-ui.json'));reports=@(@{source='docs\task-id\sdir-first-app\ui-recovery.json';name='final-recovery.json'})},
 @{id='preferences';script='preferences-smoke.mjs';reports=@(@{source='docs\task-id\pane-groups\preferences-journey.json';name='final-preferences.json'})},
 @{id='navigation';guard='clipboard-journey.ps1';reports=@(@{source='docs\task-id\clipboard-navigation\navigation-journey.json';name='final-navigation.json'},@{source='docs\task-id\clipboard-navigation\clipboard-restore.json';name='final-clipboard-restore.json'})},
 @{id='audit';script='audit-smoke.mjs';reports=@(@{source='docs\task-id\journey-audit\audit-journey.json';name='audit-journey.json'})},
 @{id='workspaces';script='workspaces-smoke.mjs';reports=@(@{source='docs\task-id\journey-audit\workspaces-journey.json';name='workspaces-journey.json'})},
 @{id='views';script='views-smoke.mjs';reports=@(@{source='docs\task-id\paste-reveal\views-journey.json';name='views-journey.json'})},
 @{id='basics';script='basics-smoke.mjs';reports=@(@{source='docs\task-id\windows-basics\basics-journey.json';name='basics-journey.json'})},
 @{id='mouse';script='mouse-history-smoke.mjs';reports=@(@{source='docs\task-id\mouse-history\mouse-journey.json';name='mouse-journey.json'})},
 @{id='tree';script='tree-smoke.mjs';reports=@(@{source='docs\task-id\mouse-history\tree-journey.json';name='tree-journey.json'})},
 @{id='active';script='active-pane-smoke.mjs';reports=@(@{source='docs\task-id\active-pane\active-journey.json';name='active-journey.json'})},
 @{id='copy';script='copy-destination-smoke.mjs';reports=@(@{source='docs\task-id\copy-destination\copy-journey.json';name='copy-journey.json'})},
 @{id='explorer';script='explorer-basics-smoke.mjs';reports=@(@{source='docs\task-id\explorer-basics\explorer-journey.json';name='explorer-journey.json'})},
 @{id='commands';guard='explorer-commands-guard.ps1';reports=@(@{source='docs\task-id\explorer-basics\commands-journey.json';name='commands-journey.json'},@{source='docs\task-id\explorer-basics\commands-clipboard-restore.json';name='commands-clipboard-restore.json'})},
 @{id='close';script='fast-close-smoke.mjs';reports=@(@{source='docs\task-id\journey-audit\fast-close.json';name='fast-close.json'})},
 @{id='performance';script='performance-ui-smoke.mjs';reports=@(@{source='docs\task-id\performance-optimize\performance-ui-journey.json';name='performance-ui-journey.json'})},
 @{id='search';script='search-smoke.mjs';extra=@(if($Large){'--large'});reports=@(@{source='docs\task-id\live-search\folder-search-journey.json';name='folder-search-journey.json'})},
 @{id='search-preferences';script='search-preferences-smoke.mjs';reports=@(@{source='docs\task-id\search-preferences\search-preferences-journey.json';name='search-preferences-journey.json'})}
)
# Exercise the new native integrations first, then complete all existing regressions.
$order=@('mouse','explorer','ui','recovery','preferences','audit','workspaces','views','basics','tree','active','copy','close','commands','navigation','performance','search','search-preferences')
$steps=@(foreach($stepId in $order){$steps | Where-Object {$_.id -eq $stepId}})
if($ResumeAt -and $ResumeAt -notin @($steps.id)){throw 'Unknown resume step.'}
# Recovery consumes the preceding UI fixture; recreate that input when a resumed run needs it.
if($ResumeAt -eq 'recovery'){$savedPath=Join-Path $evidence 'final-ui.json';if(Test-Path -LiteralPath $savedPath){$old=Get-Content -Raw -LiteralPath $savedPath|ConvertFrom-Json;if(-not(Test-Path -LiteralPath $old.fixture)){$ResumeAt='ui'}}}
$previousFixtureRoot=$env:SDIR_TEST_FIXTURE_ROOT
$runRoot=Join-Path ([IO.Path]::GetTempPath()) ('sdir-test-run-'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $runRoot|Out-Null
[IO.File]::WriteAllText((Join-Path $runRoot '.sdir-audit-fixture'),'owned')
@{owner='SDIR journey runner';created=[DateTime]::UtcNow.ToString('o')}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $runRoot '.sdir-test-run.json') -Encoding UTF8
$env:SDIR_TEST_FIXTURE_ROOT=$runRoot
$code=0
try {
$running=-not [bool]$ResumeAt;$reused=@()
foreach($step in $steps){
 if($step.id -eq $ResumeAt){$running=$true}
 if(-not $running){$saved=Get-Content -Raw -LiteralPath (Join-Path $evidence $step.reports[0].name)|ConvertFrom-Json;if(-not $saved.ok -or $saved.sha256 -ne $expected){throw "Cannot reuse failed or different executable evidence: $($step.id)"};if($step.id -eq 'ui' -and [bool]$saved.large -ne [bool]$Large){throw 'Large-fixture evidence mismatch.'};$reused+=$step.id;continue}
 Write-Host "Journey: $($step.id)"
 if($step.guard){& powershell.exe -Sta -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot $step.guard) -Executable $exe}
 else{$extras=@($step.extra); & $node (Join-Path $PSScriptRoot $step.script) $exe @extras}
 if($LASTEXITCODE -ne 0){throw "Journey failed: $($step.id)"}
 foreach($pair in $step.reports){$source=(Resolve-Path -LiteralPath $pair.source).Path;$destination=Join-Path (Resolve-Path -LiteralPath $evidence).Path $pair.name;if($source -ne $destination){Copy-Item -LiteralPath $source -Destination $destination}}
 $result=Get-Content -Raw -LiteralPath (Join-Path $evidence $step.reports[0].name)|ConvertFrom-Json;if(-not $result.ok -or $result.sha256 -ne $expected){throw "Report did not verify the current executable: $($step.id)"}
}
@{ok=$true;exitCode=0;executable=$exe;sha256=$expected;steps=$steps.Count;reusedVerifiedSteps=$reused}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $evidence 'runner-result.json') -Encoding UTF8
Write-Host 'All executed journeys passed; see reports for environment gaps.'

} catch {
 $code=1
 @{ok=$false;exitCode=1;failure=$_.Exception.Message;executable=$exe;sha256=$expected}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $evidence 'runner-result.json') -Encoding UTF8
 Write-Error -Message $_.Exception.Message -ErrorAction Continue
} finally {
 $env:SDIR_TEST_FIXTURE_ROOT=$previousFixtureRoot
 $cleanupInventory=Join-Path $evidence 'fixture-cleanup.json'
 @{directories=@(@{path=$runRoot;eligible=$true;markers=@('.sdir-audit-fixture');references=@()})}|ConvertTo-Json -Depth 6|Set-Content -LiteralPath $cleanupInventory -Encoding UTF8
 # Normal WebView shutdown can outlive its parent; preserve active data instead of forcing any process.
 $deadline=[DateTime]::UtcNow.AddSeconds(12)
 do{$active=@(Get-CimInstance Win32_Process|Where-Object{($_.ExecutablePath -and $_.ExecutablePath.StartsWith($runRoot+'\',[StringComparison]::OrdinalIgnoreCase)) -or ($_.CommandLine -and $_.CommandLine.Contains($runRoot))});if(-not $active.Count){break};Start-Sleep -Milliseconds 400}while([DateTime]::UtcNow -lt $deadline)
 try {
  & (Join-Path $PSScriptRoot 'cleanup-development.ps1') -Action Inspect -Inventory $cleanupInventory
  if($LASTEXITCODE -eq 0){& (Join-Path $PSScriptRoot 'cleanup-development.ps1') -Action Apply -Inventory $cleanupInventory}
 } catch {Write-Warning ('Owned test fixture cleanup retained data: '+$_.Exception.Message)}
}
exit $code
