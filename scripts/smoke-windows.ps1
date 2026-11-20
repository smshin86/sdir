param([string]$Executable = "$PSScriptRoot\..\dist\SDIR.exe", [switch]$Large)
$ErrorActionPreference = 'Stop'
$node = Get-Command node -ErrorAction Stop
$arguments = @((Join-Path $PSScriptRoot 'ui-smoke.cjs'), (Resolve-Path -LiteralPath $Executable).Path)
if ($Large) { $arguments += '--large' }
Set-Location -LiteralPath (Split-Path $PSScriptRoot -Parent)
& $node.Source @arguments
if ($LASTEXITCODE -ne 0) { throw 'Windows UI smoke check failed.' }
$report = if ($Large) { 'ui-large.json' } else { 'ui-smoke.json' }
& $node.Source (Join-Path $PSScriptRoot 'recovery-smoke.mjs') $arguments[1] (Join-Path 'docs\task-id\sdir-first-app' $report)
if ($LASTEXITCODE -ne 0) { throw 'Windows recovery journey failed.' }
& $node.Source (Join-Path $PSScriptRoot 'preferences-smoke.mjs') $arguments[1]
if ($LASTEXITCODE -ne 0) { throw 'Windows preferences journey failed.' }
