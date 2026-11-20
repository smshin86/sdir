param(
    [ValidateSet('FindRecycle','RestoreRecycle','CancelConflict','CompileCloser')][string]$Action,
    [string]$Fixture,
    [string]$FileName,
    [int]$AppPid = 0
)
$ErrorActionPreference = 'Stop'
$rootPath = [IO.Path]::GetFullPath($Fixture).TrimEnd('\')
$tempPath = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
if (-not $rootPath.StartsWith($tempPath,[StringComparison]::OrdinalIgnoreCase) -or
    -not ([IO.Path]::GetFileName($rootPath)).StartsWith('sdir-audit-') -or
    -not (Test-Path -LiteralPath (Join-Path $rootPath '.sdir-audit-fixture'))) {
    throw 'Only a marked, generated SDIR audit fixture is allowed.'
}
if ($Action -eq 'CompileCloser') {
    $source = @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
public class SdirAuditClose {
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
    public static int Main(string[] args) {
        int id;
        if(args.Length != 1 || !int.TryParse(args[0],out id) || id <= 0) return 1;
        using(var p=Process.GetProcessById(id)) {
            if(p.ProcessName.ToLowerInvariant().IndexOf("sdir") < 0 || p.MainWindowHandle == IntPtr.Zero) return 2;
            return PostMessage(p.MainWindowHandle,16,IntPtr.Zero,IntPtr.Zero)?0:3;
        }
    }
}
'@
    $target = Join-Path $rootPath 'close-test.exe'
    Add-Type -TypeDefinition $source -OutputAssembly $target -OutputType ConsoleApplication
    @{compiled=$true;path=$target} | ConvertTo-Json -Compress
    exit 0
}
if ($Action -eq 'CancelConflict') {
    if ($AppPid -le 0 -or -not $FileName.StartsWith('conflict-')) { throw 'An owned test app and unique conflict filename are required.' }
    Add-Type -AssemblyName UIAutomationClient
    Add-Type -AssemblyName UIAutomationTypes
    $ids = @((Get-CimInstance Win32_Process -Filter "ParentProcessId=$AppPid" | Where-Object { $_.Name -match '^SDIR.*\.exe$' }).ProcessId)
    $deadline = [DateTime]::UtcNow.AddSeconds(12)
    while ([DateTime]::UtcNow -lt $deadline) {
        foreach ($id in $ids) {
            $condition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ProcessIdProperty,[int]$id)
            $windows = [System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children,$condition)
            foreach ($window in $windows) {
                $all = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.Condition]::TrueCondition)
                $hasMarker = $false
                foreach ($element in $all) { if ($element.Current.Name.Contains($FileName)) { $hasMarker = $true; break } }
                if (-not $hasMarker) { continue }
                foreach ($element in $all) {
                    if ($element.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and $element.Current.Name -match '^(취소|Cancel)(\s|$|\()') {
                        $pattern = $null
                        if ($element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern,[ref]$pattern)) {
                            $pattern.Invoke()
                            @{cancelled=$true;processId=[int]$id;matchedExactFixture=$true} | ConvertTo-Json -Compress
                            exit 0
                        }
                    }
                }
                $windowPattern = $null
                if ($window.TryGetCurrentPattern([System.Windows.Automation.WindowPattern]::Pattern,[ref]$windowPattern)) {
                    $windowPattern.Close()
                    @{cancelled=$true;processId=[int]$id;matchedExactFixture=$true;method='owned-window-close'} | ConvertTo-Json -Compress
                    exit 0
                }
                @{cancelled=$false;matchedExactFixture=$true;processId=[int]$id;buttonNames=@($all | Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button } | ForEach-Object { $_.Current.Name })} | ConvertTo-Json -Compress
                exit 2
            }
        }
        Start-Sleep -Milliseconds 150
        $ids = @((Get-CimInstance Win32_Process -Filter "ParentProcessId=$AppPid" | Where-Object { $_.Name -match '^SDIR.*\.exe$' }).ProcessId)
    }
    @{cancelled=$false;reason='No exact owned conflict dialog found';workerIds=$ids} | ConvertTo-Json -Compress
    exit 2
}
if (-not $FileName.StartsWith('recycle-') -or [IO.Path]::GetFileName($FileName) -ne $FileName) { throw 'A unique generated recycle filename is required.' }
$shell = New-Object -ComObject Shell.Application
$bin = $shell.Namespace(10)
$match = $null
foreach ($item in $bin.Items()) {
    $from = [string]$item.ExtendedProperty('System.Recycle.DeletedFrom')
    $name = [string]$item.Name
    if (($name -eq $FileName -or $name -eq [IO.Path]::GetFileNameWithoutExtension($FileName)) -and
        $from.TrimEnd('\').Equals($rootPath,[StringComparison]::OrdinalIgnoreCase)) { $match=$item;break }
}
if ($null -eq $match) { @{found=$false} | ConvertTo-Json -Compress;exit 0 }
if ($Action -eq 'FindRecycle') {
    @{found=$true;originalFolderMatches=$true;bytes=(Get-Item -LiteralPath $match.Path).Length} | ConvertTo-Json -Compress
    exit 0
}
$verb = @($match.Verbs()) | Where-Object { $_.Name.Replace('&','') -match '^(복원|Restore)' } | Select-Object -First 1
if ($null -eq $verb) { throw 'No recognized Restore verb exists on this exact test item.' }
$verb.DoIt()
@{found=$true;restoreInvoked=$true;originalFolderMatches=$true} | ConvertTo-Json -Compress
