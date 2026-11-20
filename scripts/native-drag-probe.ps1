param([string]$Fixture,[string]$Source,[int]$AppPid,[int]$TargetX,[int]$TargetY,[int]$CssWidth)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath($Fixture).TrimEnd('\')
$sourcePath=[IO.Path]::GetFullPath($Source)
if(-not ([IO.Path]::GetFileName($root)).StartsWith('sdir-audit-') -or -not (Test-Path -LiteralPath (Join-Path $root '.sdir-audit-fixture')) -or -not $sourcePath.StartsWith($root+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Only owned fixture files can be dragged.'}
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$ownedApp=Get-Process -Id $AppPid
if($ownedApp.ProcessName -notmatch '^sdir'){throw 'Not an owned SDIR process.'}
try { [System.Windows.Automation.AutomationElement]::FromHandle($ownedApp.MainWindowHandle).SetFocus() } catch { }
$code=@'
using System;
using System.Diagnostics;
using System.Drawing;
using System.Threading;
using System.Windows.Forms;
using System.Runtime.InteropServices;
public class SdirAuditDrag {
 [StructLayout(LayoutKind.Sequential)] struct P {public int x,y;}
 [StructLayout(LayoutKind.Sequential)] struct R {public int l,t,r,b;}
 [DllImport("user32.dll")] static extern IntPtr GetWindowLongPtrW(IntPtr h,int n);
 [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h,IntPtr a,int x,int y,int w,int z,uint f);
 [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
 [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")] static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] static extern bool GetCursorPos(out P p);
 [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr h,ref P p);
 [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h,out R r);
 [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(P p);
 [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h,uint f);
 [DllImport("user32.dll")] static extern void mouse_event(uint f,uint x,uint y,uint d,UIntPtr e);
 public static string Run(int pid,string file,int x,int y,int cssWidth) {
  SetProcessDPIAware();
  using(var process=Process.GetProcessById(pid)) {
   if(process.ProcessName.ToLowerInvariant().IndexOf("sdir")<0)throw new Exception("Not owned SDIR app");
   IntPtr target=process.MainWindowHandle;if(target==IntPtr.Zero)throw new Exception("No target window");
   R rect;GetClientRect(target,out rect);double scale=(double)(rect.r-rect.l)/cssWidth;
   if(x<0||y<0||x>=cssWidth)throw new Exception("Invalid target point");
   P drop=new P{x=(int)(x*scale),y=(int)(y*scale)};ClientToScreen(target,ref drop);
   bool wasTop=(GetWindowLongPtrW(target,-20).ToInt64()&8)!=0;SetWindowPos(target,new IntPtr(-1),0,0,0,0,19);SetForegroundWindow(target);Thread.Sleep(150);
   if(GetAncestor(WindowFromPoint(drop),2)!=target){SetWindowPos(target,new IntPtr(wasTop?-1:-2),0,0,0,0,19);return "target-not-visible";}
   P original;GetCursorPos(out original);string result="not-started";bool started=false;
   using(var form=new Form())using(var button=new Button())using(var timeout=new System.Windows.Forms.Timer()) {
    form.Text="SDIR generated file drag test";form.Size=new Size(230,95);form.StartPosition=FormStartPosition.Manual;form.Location=new Point(Math.Max(5,drop.x-500),Math.Max(5,drop.y-300));form.TopMost=true;
    button.Dock=DockStyle.Fill;button.Text="Generated fixture only";form.Controls.Add(button);
    button.MouseDown+=(s,e)=>{
     if(started||e.Button!=MouseButtons.Left)return;started=true;
     P source=new P();Point point=button.PointToScreen(new Point(button.Width/2,button.Height/2));source.x=point.X;source.y=point.Y;
     var mover=new Thread(()=>{Thread.Sleep(300);if(GetAncestor(WindowFromPoint(drop),2)==target){SetCursorPos(drop.x,drop.y);}else{SetCursorPos(source.x,source.y);}Thread.Sleep(350);mouse_event(4,0,0,0,UIntPtr.Zero);});mover.IsBackground=true;mover.Start();
     var data=new DataObject(DataFormats.FileDrop,new string[]{file});
     result=button.DoDragDrop(data,DragDropEffects.Copy).ToString();form.Close();
    };
    form.Shown+=(s,e)=>{var starter=new Thread(()=>{Thread.Sleep(200);form.BeginInvoke((Action)(()=>{Point p=button.PointToScreen(new Point(button.Width/2,button.Height/2));P pp=new P{x=p.X,y=p.Y};if(GetAncestor(WindowFromPoint(pp),2)!=form.Handle){result="source-not-visible";form.Close();return;}SetCursorPos(p.X,p.Y);mouse_event(2,0,0,0,UIntPtr.Zero);}));});starter.IsBackground=true;starter.Start();};
    timeout.Interval=4500;timeout.Tick+=(s,e)=>{result="timeout";mouse_event(4,0,0,0,UIntPtr.Zero);form.Close();};timeout.Start();
    try{Application.Run(form);}finally{mouse_event(4,0,0,0,UIntPtr.Zero);SetCursorPos(original.x,original.y);SetWindowPos(target,new IntPtr(wasTop?-1:-2),0,0,0,0,19);}
   }
   return result;
  }
 }
}
'@
Add-Type -TypeDefinition $code -ReferencedAssemblies System.Windows.Forms,System.Drawing
$result=[SdirAuditDrag]::Run($AppPid,$sourcePath,$TargetX,$TargetY,$CssWidth)
@{effect=$result;ownedSource=$true;ownedTargetProcess=$AppPid} | ConvertTo-Json -Compress
