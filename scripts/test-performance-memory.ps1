$ErrorActionPreference='Stop'
$parsed=[Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'process-memory.ps1'),[ref]$null,[ref]$null)
$definition=$parsed.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Get-SdirProcessMemory'},$true)
if(-not $definition){throw 'Memory collector missing.'}
Invoke-Expression -Command $definition.Extent.Text
$rootTime=[datetime]::Parse('2026-11-16T09:00:00Z').ToUniversalTime()
$script:fixture=@(
 @{ProcessId=100;ParentProcessId=1;CreationDate=$rootTime},
 @{ProcessId=200;ParentProcessId=100;CreationDate=$rootTime.AddSeconds(1)},
 @{ProcessId=300;ParentProcessId=200;CreationDate=$rootTime.AddSeconds(2)},
 @{ProcessId=400;ParentProcessId=100;CreationDate=$rootTime.AddDays(-1)},
 @{ProcessId=500;ParentProcessId=400;CreationDate=$rootTime.AddHours(-1)},
 @{ProcessId=600;ParentProcessId=100;CreationDate=$rootTime.AddSeconds(3)}
)|ForEach-Object{[pscustomobject]$_}
function Get-CimInstance {param($ClassName) $script:fixture}
$script:rootReused=$false
function Get-Process {param($Id,$ErrorAction)
 $row=$script:fixture|Where-Object{$_.ProcessId -eq $Id}|Select-Object -First 1
 $time=$row.CreationDate;if($Id -eq 600 -or ($Id -eq 100 -and $script:rootReused)){$time=$time.AddSeconds(5)}
 [pscustomobject]@{Id=$Id;ProcessName='owned-fixture';StartTime=$time;PrivateMemorySize64=100;WorkingSet64=200;CPU=0}
}
$r=Get-SdirProcessMemory 100
if(($r.processes.pid|Sort-Object)-join ',' -ne '100,200,300' -or $r.privateBytes -ne 300){throw 'A stale PID reference contaminated the aggregate.'}
$script:rootReused=$true;$rejected=$false
try{Get-SdirProcessMemory 100|Out-Null}catch{$rejected=$true}
if(-not $rejected){throw 'A replaced root identity must reject the sample.'}
@{ok=$true;checks=@('old orphan ParentPID references and changed live process identity are excluded','changed root identity rejects the sample')}|ConvertTo-Json
