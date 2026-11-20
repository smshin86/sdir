param([ValidateSet('DragOut','TerminalCwd','CancelOpenWith','Attributes','Memory','SkipConflict','PrepareOpenWithReceiver','ChooseOpenWith')][string]$Action,[string]$Fixture,[int]$AppPid,[int]$UtilityPid,[int]$SourceX,[int]$SourceY,[int]$CssWidth,[string]$FileName)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath($Fixture).TrimEnd('\')
if(-not $root.StartsWith([IO.Path]::GetTempPath(),[StringComparison]::OrdinalIgnoreCase) -or -not ([IO.Path]::GetFileName($root)).StartsWith('sdir-audit-') -or -not (Test-Path -LiteralPath (Join-Path $root '.sdir-audit-fixture'))){throw 'Only marked owned temporary fixtures are allowed.'}
if($Action -eq 'Attributes'){
 $p=Join-Path $root 'source\remote-owned.txt';[IO.File]::SetAttributes($p,([IO.File]::GetAttributes($p) -bor [IO.FileAttributes]::Offline));@{offline=[bool]([IO.File]::GetAttributes($p) -band [IO.FileAttributes]::Offline)}|ConvertTo-Json -Compress;exit 0
}
$app=Get-Process -Id $AppPid
if($app.ProcessName -notmatch '^SDIR' -or $app.MainWindowHandle -eq 0){throw 'An owned SDIR window is required.'}
if($Action -eq 'Memory'){
 . (Join-Path $PSScriptRoot 'process-memory.ps1')
 $m=Get-SdirProcessMemory -ProcessId $AppPid;$appWorkers=@($m.processes|Where-Object{$_.name -notmatch '^msedgewebview2'});$webview=@($m.processes|Where-Object{$_.name -match '^msedgewebview2'})
 @{processes=$m.processes.Count;appWorkerProcesses=$appWorkers.Count;webViewProcesses=$webview.Count;workingSetBytes=$m.workingSetBytes;privateBytes=$m.privateBytes;appWorkerPrivateBytes=($appWorkers|Measure-Object privateBytes -Sum).Sum;webViewPrivateBytes=($webview|Measure-Object privateBytes -Sum).Sum;identityGuard=$m.identityGuard}|ConvertTo-Json -Compress;exit 0
}
if($Action -eq 'TerminalCwd'){
 $p=Get-CimInstance Win32_Process -Filter "ProcessId=$UtilityPid"
 if($null -eq $p -or $p.Name -ne 'powershell.exe' -or $p.CommandLine -notmatch '\-NoLogo \-NoProfile \-NoExit$'){throw 'Only the exact newly launched utility console is allowed.'}
 Add-Type -TypeDefinition @'
using System;using System.Runtime.InteropServices;
public class SdirConsoleInput{
 [StructLayout(LayoutKind.Explicit,CharSet=CharSet.Unicode,Size=20)]public struct Record{
  [FieldOffset(0)]public ushort type;[FieldOffset(4)]public int down;[FieldOffset(8)]public ushort repeat;[FieldOffset(10)]public ushort key;[FieldOffset(14)]public char ch;
 }
 [DllImport("kernel32.dll")]static extern bool FreeConsole();[DllImport("kernel32.dll")]static extern bool AttachConsole(uint pid);[DllImport("kernel32.dll")]static extern IntPtr GetStdHandle(int n);
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode)]static extern IntPtr CreateFileW(string p,uint a,uint share,IntPtr security,uint mode,uint flags,IntPtr template);[DllImport("kernel32.dll")]static extern bool CloseHandle(IntPtr h);
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode)]static extern bool WriteConsoleInputW(IntPtr h,Record[] r,uint n,out uint written);
 public static void Send(uint pid,string text){FreeConsole();if(!AttachConsole(pid))throw new Exception("Attach owned console failed");try{var r=new Record[text.Length];for(int i=0;i<text.Length;i++)r[i]=new Record{type=1,down=1,repeat=1,key=(ushort)(text[i]=='\r'?13:0),ch=text[i]};uint written;var input=CreateFileW("CONIN$",0xC0000000,3,IntPtr.Zero,3,0,IntPtr.Zero);if(input==new IntPtr(-1))throw new Exception("Owned console buffer unavailable");try{if(!WriteConsoleInputW(input,r,(uint)r.Length,out written)||written!=r.Length)throw new Exception("Owned console input failed");}finally{CloseHandle(input);}}finally{FreeConsole();}}
}
'@
 $out=Join-Path $root 'terminal-cwd.txt';$literal=$out.Replace("'","''");[SdirConsoleInput]::Send($UtilityPid,"[IO.File]::WriteAllText('$literal',`$PWD.Path); exit`r")
 $deadline=[DateTime]::UtcNow.AddSeconds(10);while(-not (Test-Path -LiteralPath $out) -and [DateTime]::UtcNow -lt $deadline){Start-Sleep -Milliseconds 80}
 if(-not(Test-Path -LiteralPath $out)){throw 'The owned terminal did not report its directory.'};@{cwd=[IO.File]::ReadAllText($out);pid=$UtilityPid}|ConvertTo-Json -Compress;exit 0
}
if($Action -eq 'SkipConflict'){
 if($FileName -notmatch '^partial-[a-f0-9-]+-b\.txt$'){throw 'Exact generated conflict name required.'}
 Add-Type -AssemblyName UIAutomationClient;Add-Type -AssemblyName UIAutomationTypes
 $deadline=[DateTime]::UtcNow.AddSeconds(12)
 do{
  $ids=@((Get-CimInstance Win32_Process -Filter "ParentProcessId=$AppPid"|Where-Object{$_.Name -match '^SDIR'}).ProcessId)
  foreach($id in $ids){$c=[System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ProcessIdProperty,[int]$id);foreach($w in [System.Windows.Automation.AutomationElement]::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children,$c)){
   $all=$w.FindAll([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.Condition]::TrueCondition)
   if(-not (@($w.Current.Name)+@($all|ForEach-Object{$_.Current.Name})|Where-Object{$_.Contains($FileName)})){continue}
   foreach($element in $all){if($element.Current.Name -match '(이 파일 건너뛰기|Skip this file|^건너뛰기$|^Skip$)'){$pattern=$null;if($element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern,[ref]$pattern)){$pattern.Invoke();@{skipped=$true;exactFixture=$true;worker=[int]$id}|ConvertTo-Json -Compress;exit 0}}}
  }};Start-Sleep -Milliseconds 100
 }while([DateTime]::UtcNow -lt $deadline)
 throw 'No exact owned conflict with Skip action was found.'
}
if($Action -eq 'PrepareOpenWithReceiver'){
 $program=Join-Path $root 'open-with-receiver.exe'
 Add-Type -TypeDefinition @'
using System;using System.IO;using System.Reflection;
public class SdirOpenReceiver{public static void Main(string[] args){var root=Path.GetDirectoryName(Assembly.GetExecutingAssembly().Location);if(args.Length!=1||!File.Exists(Path.Combine(root,".sdir-audit-fixture"))||!Path.GetFullPath(args[0]).StartsWith(root+"\\",StringComparison.OrdinalIgnoreCase))return;File.WriteAllText(Path.Combine(root,"open-with-receipt.txt"),args[0]+"\n"+Convert.ToBase64String(File.ReadAllBytes(args[0])));}}
'@ -OutputAssembly $program -OutputType WindowsApplication
 @{program=$program}|ConvertTo-Json -Compress;exit 0
}
if($Action -in @('CancelOpenWith','ChooseOpenWith')){
 Add-Type -AssemblyName UIAutomationClient;Add-Type -AssemblyName UIAutomationTypes
 if($FileName -notmatch '^openwith-[a-f0-9-]+\.sdir[a-f0-9]{12}$'){throw 'Unique owned association fixture required.'}
 Add-Type -AssemblyName System.Drawing
 Add-Type -TypeDefinition @'
using System;using System.Drawing;using System.Drawing.Imaging;using System.Runtime.InteropServices;
public class SdirOwnedDialog{
 delegate bool EnumProc(IntPtr h,IntPtr l);[DllImport("user32.dll")]static extern bool EnumWindows(EnumProc p,IntPtr l);[DllImport("user32.dll")]static extern uint GetWindowThreadProcessId(IntPtr h,out uint p);[DllImport("user32.dll")]static extern bool IsWindowVisible(IntPtr h);
 public static IntPtr[] For(int[] ids){var found=new System.Collections.Generic.List<IntPtr>();EnumWindows((h,l)=>{uint p;GetWindowThreadProcessId(h,out p);if(Array.IndexOf(ids,(int)p)>=0&&IsWindowVisible(h))found.Add(h);return true;},IntPtr.Zero);return found.ToArray();}

 [DllImport("user32.dll")]static extern bool EnumChildWindows(IntPtr h,EnumProc p,IntPtr l);[DllImport("user32.dll")]static extern int GetDlgCtrlID(IntPtr h);[DllImport("user32.dll",CharSet=CharSet.Unicode)]static extern int GetWindowTextW(IntPtr h,System.Text.StringBuilder b,int n);[DllImport("user32.dll",CharSet=CharSet.Unicode)]static extern IntPtr SendMessageW(IntPtr h,uint m,IntPtr w,string l);
 [DllImport("user32.dll",EntryPoint="SendMessageW",CharSet=CharSet.Unicode)]static extern IntPtr ReadMessage(IntPtr h,uint m,IntPtr w,System.Text.StringBuilder l);
 static IntPtr Find(IntPtr h,int id){IntPtr result=IntPtr.Zero;EnumChildWindows(h,(c,l)=>{if(GetDlgCtrlID(c)==id){result=c;return false;}return true;},IntPtr.Zero);return result;}
 public static bool Choose(IntPtr h,string program){var edit=IntPtr.Zero;EnumChildWindows(h,(c,l)=>{var cls=new System.Text.StringBuilder(80);GetClassNameW(c,cls,cls.Capacity);if(GetDlgCtrlID(c)==1148&&cls.ToString()=="Edit"){edit=c;return false;}return true;},IntPtr.Zero);var button=Find(h,1);if(edit==IntPtr.Zero||button==IntPtr.Zero)return false;var name=new System.Text.StringBuilder(300);GetWindowTextW(button,name,name.Capacity);if(!name.ToString().Contains("이 프로그램으로 열기"))return false;SendMessageW(edit,12,IntPtr.Zero,program);var text=new System.Text.StringBuilder(32768);ReadMessage(edit,13,new IntPtr(text.Capacity),text);if(text.ToString()!=program)throw new Exception("Owned filename control rejected exact program path");SendMessageW(button,245,IntPtr.Zero,null);return true;}
 [DllImport("user32.dll",CharSet=CharSet.Unicode)]static extern int GetClassNameW(IntPtr h,System.Text.StringBuilder b,int n);
 public static string[] Describe(IntPtr h){var list=new System.Collections.Generic.List<string>();EnumChildWindows(h,(c,l)=>{var name=new System.Text.StringBuilder(120);var cls=new System.Text.StringBuilder(80);GetWindowTextW(c,name,name.Capacity);GetClassNameW(c,cls,cls.Capacity);list.Add(GetDlgCtrlID(c)+" "+cls+" "+name);return list.Count<80;},IntPtr.Zero);return list.ToArray();}
 [StructLayout(LayoutKind.Sequential)]struct R{public int l,t,r,b;}
 [DllImport("user32.dll")]static extern IntPtr GetWindow(IntPtr h,uint cmd);[DllImport("user32.dll")]static extern bool GetWindowRect(IntPtr h,out R r);[DllImport("user32.dll")]static extern bool PrintWindow(IntPtr h,IntPtr dc,uint flags);
 public static bool Owner(IntPtr h,IntPtr app){for(int i=0;i<16&&h!=IntPtr.Zero;i++){h=GetWindow(h,4);if(h==app)return true;}return false;}
 public static bool Capture(IntPtr h,string file){R r;if(!GetWindowRect(h,out r)||r.r-r.l<20||r.b-r.t<20||r.r-r.l>4000||r.b-r.t>4000)return false;using(var b=new Bitmap(r.r-r.l,r.b-r.t))using(var g=Graphics.FromImage(b)){var dc=g.GetHdc();bool ok;try{ok=PrintWindow(h,dc,2);}finally{g.ReleaseHdc(dc);}if(ok)b.Save(file,ImageFormat.Png);return ok;}}
}
'@ -ReferencedAssemblies System.Drawing
 $deadline=[DateTime]::UtcNow.AddSeconds(12);$diagnostics=@()
 do{
  $marker=[IO.Path]::GetExtension($FileName)
  $ids=@((Get-CimInstance Win32_Process -Filter "ParentProcessId=$AppPid"|Where-Object{$_.Name -match '^SDIR' -and $_.CommandLine -match ' --worker$'}).ProcessId)
  foreach($handle in [SdirOwnedDialog]::For([int[]]$ids)){
    $w=[System.Windows.Automation.AutomationElement]::FromHandle($handle)
    if($w.Current.ProcessId -eq $AppPid){continue}
    $ownerProcess=Get-Process -Id $w.Current.ProcessId -ErrorAction SilentlyContinue
    if($ownerProcess.ProcessName -notmatch '^(SDIR.*|OpenWith|PickerHost|ShellExperienceHost|ShellHost|ApplicationFrameHost)$'){continue}
    if(-not $w.Current.Name.Contains($FileName)){continue}
    $all=$w.FindAll([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.Condition]::TrueCondition);$names=@($w.Current.Name)+@($all|ForEach-Object{$_.Current.Name})
    $native=[IntPtr]$handle;$owned=[SdirOwnedDialog]::Owner($native,$app.MainWindowHandle)
    if(-not $owned -and -not ($names|Where-Object{$_.Contains($FileName) -or $_.Contains($marker)})){if($ownerProcess.ProcessName -eq 'OpenWith' -and $diagnostics.Count -lt 2){$diagnostics+=@{owner=$ownerProcess.ProcessName;title=$w.Current.Name;names=@($names|Select-Object -First 24)}};continue}
    if($Action -eq 'ChooseOpenWith'){
     $program=Join-Path $root 'open-with-receiver.exe';if(-not(Test-Path -LiteralPath $program)){throw 'Owned receiver was not prepared.'}
     if([SdirOwnedDialog]::Choose($native,$program)){@{chosen=$true;exactFixture=$true;program=$program;nativeFilenameControl=$true}|ConvertTo-Json -Compress;exit 0}
     $candidates=@($all|Where-Object{$_.Current.AutomationId -in @('1148','1001') -or (($_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Edit -or $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::ComboBox) -and $_.Current.Name -match '(파일 이름|File name)')})
     $value=$null;foreach($candidate in $candidates){if($candidate.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern,[ref]$value) -and -not $value.Current.IsReadOnly){break}else{$value=$null}}
     if($null -eq $value){$controls=@($all|Where-Object{$_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Edit -or $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::ComboBox}|ForEach-Object{@{name=$_.Current.Name;id=$_.Current.AutomationId}});$diagnostics=@([SdirOwnedDialog]::Describe($native));Start-Sleep -Milliseconds 100;continue};$value.SetValue($program)
     $button=$all|Where-Object{$_.Current.AutomationId -eq '1' -and $_.Current.Name.Contains('이 프로그램으로 열기')}|Select-Object -First 1
     $invoke=$null;if($null -eq $button -or -not $button.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern,[ref]$invoke)){throw 'Owned program-picker confirmation button missing.'};$invoke.Invoke();@{chosen=$true;exactFixture=$true;program=$program}|ConvertTo-Json -Compress;exit 0
    }
    $pattern=$null;if($w.TryGetCurrentPattern([System.Windows.Automation.WindowPattern]::Pattern,[ref]$pattern)){$owner=$w.Current.ProcessId;$image=Join-Path $root 'openwith-dialog.png';$captured=[SdirOwnedDialog]::Capture($native,$image);$pattern.Close();@{closed=$true;requestedFixture=$FileName;ownerMatchesApp=$owned;windowOwner=[int]$owner;image=if($captured){$image}else{$null}}|ConvertTo-Json -Compress;exit 0}
  };Start-Sleep -Milliseconds 100
 }while([DateTime]::UtcNow -lt $deadline)
 throw ('No exact owned open-with dialog found: '+($diagnostics|ConvertTo-Json -Depth 3 -Compress))
}
Add-Type -AssemblyName System.Windows.Forms;Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName UIAutomationClient;Add-Type -AssemblyName UIAutomationTypes
try{[System.Windows.Automation.AutomationElement]::FromHandle($app.MainWindowHandle).SetFocus()}catch{}
$code=@'
using System;using System.Diagnostics;using System.Drawing;using System.Threading;using System.Windows.Forms;using System.Runtime.InteropServices;using System.IO;
public class SdirOutgoingDrag{
 [StructLayout(LayoutKind.Sequential)]struct P{public int x,y;}[StructLayout(LayoutKind.Sequential)]struct R{public int l,t,r,b;}
 [DllImport("user32.dll")]static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int z,uint flags);[DllImport("user32.dll")]static extern IntPtr GetWindowLongPtrW(IntPtr h,int n);[DllImport("user32.dll")]static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")]static extern void keybd_event(byte v,byte s,uint f,UIntPtr e);
 [DllImport("user32.dll")]static extern bool SetProcessDPIAware();[DllImport("user32.dll")]static extern bool SetForegroundWindow(IntPtr h);[DllImport("user32.dll")]static extern bool SetCursorPos(int x,int y);[DllImport("user32.dll")]static extern bool GetCursorPos(out P p);[DllImport("user32.dll")]static extern bool ClientToScreen(IntPtr h,ref P p);[DllImport("user32.dll")]static extern bool GetClientRect(IntPtr h,out R r);[DllImport("user32.dll")]static extern IntPtr WindowFromPoint(P p);[DllImport("user32.dll")]static extern IntPtr GetAncestor(IntPtr h,uint f);[DllImport("user32.dll")]static extern void mouse_event(uint f,uint x,uint y,uint d,UIntPtr e);
 public static string[] Run(int pid,string root,int x,int y,int css){SetProcessDPIAware();using(var app=Process.GetProcessById(pid)){
  IntPtr source=app.MainWindowHandle;bool wasTop=(GetWindowLongPtrW(source,-20).ToInt64()&8)!=0;IntPtr previous=GetForegroundWindow();SetWindowPos(source,new IntPtr(-1),0,0,0,0,19);R rect;GetClientRect(source,out rect);if(css<100||x<0||x>=css||y<0)throw new Exception("Invalid owned point");double scale=(double)(rect.r-rect.l)/css;P start=new P{x=(int)(x*scale),y=(int)(y*scale)};ClientToScreen(source,ref start);P original;GetCursorPos(out original);string[] received=null;Exception failure=null;bool pressed=false;
  using(var form=new Form())using(var label=new Label())using(var timer=new System.Windows.Forms.Timer()){
   var screen=Screen.FromHandle(source).WorkingArea;form.Text="SDIR owned outbound drop receiver";form.Size=new Size(300,130);form.StartPosition=FormStartPosition.Manual;form.Location=new Point(screen.Right-315,screen.Bottom-150);form.TopMost=true;form.AllowDrop=true;label.Dock=DockStyle.Fill;label.Text="Only generated SDIR fixture files";label.TextAlign=ContentAlignment.MiddleCenter;form.Controls.Add(label);
   form.DragEnter+=(s,e)=>{if(e.Data.GetDataPresent(DataFormats.FileDrop))e.Effect=DragDropEffects.Copy;};form.DragOver+=(s,e)=>{if(e.Data.GetDataPresent(DataFormats.FileDrop))e.Effect=DragDropEffects.Copy;};
   form.DragDrop+=(s,e)=>{try{received=(string[])e.Data.GetData(DataFormats.FileDrop);var dest=Path.Combine(root,"external-receiver");Directory.CreateDirectory(dest);foreach(var p in received){var full=Path.GetFullPath(p);if(!full.StartsWith(root+"\\",StringComparison.OrdinalIgnoreCase))throw new Exception("Foreign drag payload");File.Copy(full,Path.Combine(dest,Path.GetFileName(full)),false);}e.Effect=DragDropEffects.Copy;}catch(Exception err){failure=err;}form.Close();};
   form.Shown+=(s,e)=>{P end=new P();var pt=form.PointToScreen(new Point(150,60));end.x=pt.X;end.y=pt.Y;SetForegroundWindow(source);form.BringToFront();var mover=new Thread(()=>{Thread.Sleep(250);if(GetAncestor(WindowFromPoint(start),2)!=source){failure=new Exception("Owned source is occluded");form.BeginInvoke((Action)(()=>form.Close()));return;}SetCursorPos(start.x,start.y);mouse_event(2,0,0,0,UIntPtr.Zero);pressed=true;Thread.Sleep(150);SetCursorPos(start.x+12,start.y+2);Thread.Sleep(900);for(int n=1;n<=20;n++){SetCursorPos(start.x+(end.x-start.x)*n/20,start.y+(end.y-start.y)*n/20);Thread.Sleep(35);}Thread.Sleep(300);if(GetAncestor(WindowFromPoint(end),2)!=form.Handle){failure=new Exception("Owned receiver became occluded");keybd_event(27,0,0,UIntPtr.Zero);keybd_event(27,0,2,UIntPtr.Zero);}mouse_event(4,0,0,0,UIntPtr.Zero);pressed=false;});mover.IsBackground=true;mover.Start();};timer.Interval=8000;timer.Tick+=(s,e)=>{failure=new Exception("Native outbound drag timed out");form.Close();};timer.Start();try{Application.Run(form);}finally{if(pressed)mouse_event(4,0,0,0,UIntPtr.Zero);SetCursorPos(original.x,original.y);SetWindowPos(source,new IntPtr(wasTop?-1:-2),0,0,0,0,19);if(GetForegroundWindow()==source&&previous!=IntPtr.Zero)SetForegroundWindow(previous);}if(failure!=null)throw failure;if(received==null)throw new Exception("No native paths received");return received;
  }
 }}
}
'@
Add-Type -TypeDefinition $code -ReferencedAssemblies System.Windows.Forms,System.Drawing
$paths=[SdirOutgoingDrag]::Run($AppPid,$root,$SourceX,$SourceY,$CssWidth)
@{paths=@($paths);effect='Copy';ownedExternalTarget=$true}|ConvertTo-Json -Compress
