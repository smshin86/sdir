param([string]$Executable = "$PSScriptRoot\..\dist\SDIR.exe")
& "$PSScriptRoot\clipboard-guard.ps1" -Executable $Executable -Journey 'navigation-smoke.mjs'
exit $LASTEXITCODE
