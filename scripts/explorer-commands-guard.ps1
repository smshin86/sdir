param([string]$Executable = "$PSScriptRoot\..\dist\SDIR.exe")
& "$PSScriptRoot\clipboard-guard.ps1" -Executable $Executable -Journey 'explorer-commands-smoke.mjs'
exit $LASTEXITCODE
