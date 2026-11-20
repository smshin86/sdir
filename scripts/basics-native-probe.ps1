param([ValidateSet('Bounds','Resize','Maximize','Restore','CloseProperties','InspectMain','CloseMain')][string]$Action,[int]$AppPid,[string]$Fixture,[string]$FileName,[long]$WindowHandle)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath($Fixture)
if(-not $root.StartsWith([IO.Path]::GetTempPath(),[StringComparison]::OrdinalIgnoreCase) -or -not (Test-Path -LiteralPath (Join-Path $root '.sdir-audit-fixture'))){throw 'Only an owned fixture is allowed.'}
$app=Get-Process -Id $AppPid
if($app.ProcessName -notmatch '^SDIR' -or ($app.MainWindowHandle -eq 0 -and $Action -notin @('InspectMain','CloseMain'))){throw 'An owned SDIR test window is required.'}
if($Action -eq 'CloseProperties'){
 if($FileName -notmatch '^property-[a-f0-9-]+\.txt$'){throw 'An exact generated property filename is required.'}
 Add-Type -AssemblyName UIAutomationClient
 Add-Type -AssemblyName UIAutomationTypes
 $deadline=[DateTime]::UtcNow.AddSeconds(12)
 do {
  $ids=@((Get-CimInstance Win32_Process -Filter "ParentProcessId=$AppPid" | Where-Object {$_.Name -match '^SDIR.*\.exe$'}).ProcessId)
  foreach($id in $ids){
   $c=[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ProcessIdProperty,[int]$id)
   foreach($window in [System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children,$c)){
    $all=$window.FindAll([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.Condition]::TrueCondition)
    $stem=[IO.Path]::GetFileNameWithoutExtension($FileName)
    $exact=$window.Current.Name.Contains($stem)
    foreach($e in $all){if($e.Current.Name.Contains($stem)){$exact=$true}}
    if($exact){$pattern=$null;if($window.TryGetCurrentPattern([System.Windows.Automation.WindowPattern]::Pattern,[ref]$pattern)){$pattern.Close();@{found=$true;closed=$true;ownedWorker=[int]$id;exactFixture=$true}|ConvertTo-Json -Compress;exit 0}}
   }
  }
  Start-Sleep -Milliseconds 100
 }while([DateTime]::UtcNow -lt $deadline)
 throw 'The exact owned property sheet was not found.'
}
Add-Type -TypeDefinition @'
using System; using System.Text; using System.Runtime.InteropServices;
public class SdirBounds {
 [StructLayout(LayoutKind.Sequential)] public struct Rect{public int Left,Top,Right,Bottom;}
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out Rect r);
 [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h,out Rect r);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr a,int x,int y,int w,int z,uint f);
 [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int c);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h,StringBuilder b,int n);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern IntPtr SendMessageTimeout(IntPtr h,uint m,IntPtr w,IntPtr l,uint f,uint t,out IntPtr result);
}
'@
$handle=$app.MainWindowHandle
if($Action -in @('InspectMain','CloseMain')){$handle=[IntPtr]$WindowHandle}
$owner=[uint32]0;$null=[SdirBounds]::GetWindowThreadProcessId($handle,[ref]$owner)
if($owner -ne $AppPid){throw 'Stored main HWND does not belong to the owned app.'}
$class=[Text.StringBuilder]::new(256);$null=[SdirBounds]::GetClassName($handle,$class,256)
if($class.ToString() -ne 'Window Class'){throw 'The expected SDIR main window class was not found.'}
if($Action -in @('InspectMain','CloseMain')){
 $value=[IntPtr]::Zero;$timer=[Diagnostics.Stopwatch]::StartNew();$pong=[SdirBounds]::SendMessageTimeout($handle,0,[IntPtr]::Zero,[IntPtr]::Zero,2,200,[ref]$value)
 $r=@{hwnd=$handle.ToInt64();class=$class.ToString();visible=[SdirBounds]::IsWindowVisible($handle);enabled=[SdirBounds]::IsWindowEnabled($handle);responding=($pong -ne [IntPtr]::Zero);pingMs=$timer.Elapsed.TotalMilliseconds}
 if($Action -eq 'CloseMain'){$r.posted=[SdirBounds]::PostMessage($handle,16,[IntPtr]::Zero,[IntPtr]::Zero)}
 $r|ConvertTo-Json -Compress;exit 0
}

if($Action -eq 'Resize'){[SdirBounds]::ShowWindow($handle,9)|Out-Null;if(-not [SdirBounds]::SetWindowPos($handle,[IntPtr]::Zero,70,65,1460,940,4)){throw 'Could not resize owned window.'};Start-Sleep -Milliseconds 250}
if($Action -eq 'Maximize'){[SdirBounds]::ShowWindow($handle,3)|Out-Null;Start-Sleep -Milliseconds 250}
if($Action -eq 'Restore'){[SdirBounds]::ShowWindow($handle,9)|Out-Null;Start-Sleep -Milliseconds 250}
$r=[SdirBounds+Rect]::new();$c=[SdirBounds+Rect]::new();[SdirBounds]::GetWindowRect($handle,[ref]$r)|Out-Null;[SdirBounds]::GetClientRect($handle,[ref]$c)|Out-Null
@{hwnd=$handle.ToInt64();x=$r.Left;y=$r.Top;width=$r.Right-$r.Left;height=$r.Bottom-$r.Top;clientWidth=$c.Right;clientHeight=$c.Bottom;dpi=[SdirBounds]::GetDpiForWindow($handle);maximized=[SdirBounds]::IsZoomed($handle)}|ConvertTo-Json -Compress
