param([Parameter(Mandatory=$true)][string]$OwnedRoot)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath($OwnedRoot).TrimEnd('\')
if(-not $root.StartsWith([IO.Path]::GetTempPath(),[StringComparison]::OrdinalIgnoreCase) -or -not (Test-Path -LiteralPath (Join-Path $root '.sdir-benchmark'))){throw 'A marked benchmark fixture is required.'}
Add-Type -TypeDefinition @'
using System;using System.Text;using System.Linq;using System.Collections.Generic;using System.Diagnostics;using System.Runtime.InteropServices;
public class PerfNative {
 public delegate bool EnumProc(IntPtr h,IntPtr l);
 [StructLayout(LayoutKind.Sequential)] public struct Rect {public int Left,Top,Right,Bottom;}
 [StructLayout(LayoutKind.Sequential)] public struct Point {public int X,Y;}
 [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point p);
 [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h,uint flags);
 [DllImport("user32.dll")] static extern bool GetCursorPos(out Point p);
 [DllImport("user32.dll")] static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] static extern void mouse_event(uint flags,uint x,uint y,uint data,UIntPtr extra);
 [DllImport("user32.dll")] static extern IntPtr OpenInputDesktop(uint flags,bool inherit,uint access);
 [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr h);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern bool GetUserObjectInformation(IntPtr h,int index,StringBuilder text,uint bytes,out uint required);
 [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p,IntPtr l);
 [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr h,EnumProc p,IntPtr l);
 [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h,out uint p);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h,StringBuilder b,int n);
 [DllImport("user32.dll",EntryPoint="GetWindowLongW")] static extern int GetWindowLong(IntPtr h,int index);
 [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] static extern bool IsWindowEnabled(IntPtr h);
 [DllImport("user32.dll")] static extern bool AttachThreadInput(uint a,uint b,bool attach);
 [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
 [DllImport("user32.dll")] static extern IntPtr SetFocus(IntPtr h);
 [DllImport("user32.dll")] static extern IntPtr GetFocus();
 [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr h,uint command);
 [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h,int command);
 [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
 [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint access,bool inherit,int pid);
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool QueryFullProcessImageName(IntPtr h,uint flags,StringBuilder name,ref int size);
 [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
 public static string Image(int pid){var h=OpenProcess(0x1000,false,pid);if(h==IntPtr.Zero)throw new Exception("Cannot query owned process");try{var b=new StringBuilder(32768);int n=b.Capacity;if(!QueryFullProcessImageName(h,0,b,ref n))throw new Exception("Cannot query process image");return b.ToString();}finally{CloseHandle(h);}}
 [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h,out Rect r);
 [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int z,uint flags);
 [DllImport("user32.dll")] static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr SendMessageTimeout(IntPtr h,uint m,IntPtr w,IntPtr l,uint f,uint t,out IntPtr result);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr SendMessageTimeout(IntPtr h,uint m,IntPtr w,StringBuilder l,uint f,uint t,out IntPtr result);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr SendMessageTimeout(IntPtr h,uint m,IntPtr w,string l,uint f,uint t,out IntPtr result);
 public class Control {public long hwnd;public string cls;public int x,y,width,height,count,viewStyle,view;public string text;}
 static string Class(IntPtr h){var b=new StringBuilder(256);GetClassName(h,b,b.Capacity);return b.ToString();}
 static IntPtr PackScreenPoint(Point p){if(p.X<short.MinValue||p.X>short.MaxValue||p.Y<short.MinValue||p.Y>short.MaxValue)throw new Exception("Caption coordinates cannot be packed safely");uint packed=(uint)(ushort)p.X|((uint)(ushort)p.Y<<16);return new IntPtr(unchecked((int)packed));}
 static bool OwnedCaptionHit(IntPtr h,int pid,Point p){uint owner;GetWindowThreadProcessId(h,out owner);if(owner!=pid)return false;var under=WindowFromPoint(p);if(under==IntPtr.Zero||GetAncestor(under,2)!=h)return false;IntPtr result;var sent=SendMessageTimeout(h,0x84,IntPtr.Zero,PackScreenPoint(p),2,300,out result);return sent!=IntPtr.Zero&&result.ToInt32()==2;}
 public static string Text(IntPtr h){var b=new StringBuilder(32768);IntPtr n;SendMessageTimeout(h,13,(IntPtr)b.Capacity,b,2,100,out n);return b.ToString();}
 public static long Window(int pid){long value=0;EnumWindows((h,l)=>{uint p;GetWindowThreadProcessId(h,out p);Rect r;GetWindowRect(h,out r);if(p==pid&&IsWindowVisible(h)&&Class(h)!="#32770"&&GetWindow(h,4)==IntPtr.Zero&&r.Right-r.Left>=400&&r.Bottom-r.Top>=300){value=h.ToInt64();return false;}return true;},IntPtr.Zero);return value;}
 public static Control[] Controls(int pid){var main=new IntPtr(Window(pid));var all=new List<Control>();if(main==IntPtr.Zero)return all.ToArray();EnumChildWindows(main,(h,l)=>{if(!IsWindowVisible(h))return true;Rect r;GetWindowRect(h,out r);var c=Class(h);var a=new Control{hwnd=h.ToInt64(),cls=c,x=r.Left,y=r.Top,width=r.Right-r.Left,height=r.Bottom-r.Top,count=-1,viewStyle=GetWindowLong(h,-16)&3};if(c=="SysListView32"){IntPtr n;if(SendMessageTimeout(h,0x1004,IntPtr.Zero,IntPtr.Zero,2,100,out n)!=IntPtr.Zero)a.count=n.ToInt32();if(SendMessageTimeout(h,0x108F,IntPtr.Zero,IntPtr.Zero,2,100,out n)!=IntPtr.Zero)a.view=n.ToInt32();}if(c=="Edit"||c=="msctls_statusbar32")a.text=Text(h);all.Add(a);return true;},IntPtr.Zero);return all.ToArray();}
 public static object Ping(int pid){var h=new IntPtr(Window(pid));var sw=Stopwatch.StartNew();IntPtr n;bool ok=h!=IntPtr.Zero&&SendMessageTimeout(h,0,IntPtr.Zero,IntPtr.Zero,2,150,out n)!=IntPtr.Zero;return new {ok=ok,ms=sw.Elapsed.TotalMilliseconds};}
 public static void Resize(int pid,int width,int height){var h=new IntPtr(Window(pid));if(h==IntPtr.Zero||!SetWindowPos(h,new IntPtr(-1),50,50,width,height,0x50))throw new Exception("No owned main window to resize");}
 public static object Activate(int pid){
  var h=new IntPtr(Window(pid));if(h==IntPtr.Zero)throw new Exception("No owned main window to activate");uint owner;GetWindowThreadProcessId(h,out owner);if(owner!=pid)throw new Exception("Wrong owned main window");
  ShowWindow(h,9);SetForegroundWindow(h);
  if(GetForegroundWindow()==h)return State(pid,h);
  // Never inject input on the UAC/Winlogon desktop. A normal owned caption click
  // is the fallback when Windows rejects a programmatic foreground request.
  var desktop=OpenInputDesktop(0,false,1);if(desktop==IntPtr.Zero)throw new Exception("Cannot verify ordinary input desktop");
  try{var name=new StringBuilder(256);uint needed;if(!GetUserObjectInformation(desktop,2,name,512,out needed)||!name.ToString().Equals("Default",StringComparison.OrdinalIgnoreCase))throw new Exception("Native activation requires the ordinary Default desktop");}finally{CloseDesktop(desktop);}
  if(Window(pid)!=h.ToInt64())throw new Exception("Owned main window changed before caption activation");
  if(!SetWindowPos(h,new IntPtr(-1),0,0,0,0,0x53))throw new Exception("Cannot reveal owned test window");
  Rect r;if(!GetWindowRect(h,out r))throw new Exception("Cannot locate owned caption");var target=new Point{X=r.Left+(r.Right-r.Left)/2,Y=r.Top+12};
  if(!OwnedCaptionHit(h,pid,target))throw new Exception("Owned point is not the visible owned HTCAPTION");
  Point original;if(!GetCursorPos(out original))throw new Exception("Cannot retain pointer position");bool pressed=false;
  try{if(!SetCursorPos(target.X,target.Y)||!OwnedCaptionHit(h,pid,target))throw new Exception("Owned caption changed before click");mouse_event(2,0,0,0,UIntPtr.Zero);pressed=true;mouse_event(4,0,0,0,UIntPtr.Zero);pressed=false;System.Threading.Thread.Sleep(80);}finally{if(pressed)mouse_event(4,0,0,0,UIntPtr.Zero);SetCursorPos(original.X,original.Y);}
  return State(pid,h);
 }
 static object State(int pid,IntPtr h){if(h==IntPtr.Zero)throw new Exception("No owned main window is available");uint owner;GetWindowThreadProcessId(h,out owner);if(owner!=pid)throw new Exception("Main window no longer belongs to the owned process");Rect r;if(!GetWindowRect(h,out r))throw new Exception("Cannot read owned main window bounds");var foreground=GetForegroundWindow();uint foregroundPid;GetWindowThreadProcessId(foreground,out foregroundPid);return new {hwnd=h.ToInt64(),main_hwnd=h.ToInt64(),foreground_hwnd=foreground.ToInt64(),foreground_pid=foregroundPid,cls=Class(h),topmost=(GetWindowLong(h,-20)&8)!=0,foreground=foreground==h,x=r.Left,y=r.Top,width=r.Right-r.Left,height=r.Bottom-r.Top};}
 public static object State(int pid){return State(pid,new IntPtr(Window(pid)));}
 public static object Close(int pid,long window){var h=new IntPtr(window);uint p;GetWindowThreadProcessId(h,out p);if(p!=pid)throw new Exception("Original owned main window no longer belongs to this process");return new {hwnd=window,cls=Class(h),posted=h!=IntPtr.Zero&&IsWindowEnabled(h)&&PostMessage(h,16,IntPtr.Zero,IntPtr.Zero)};}
 public static object WindowMode(int pid,long window,int command){var h=new IntPtr(window);uint p;GetWindowThreadProcessId(h,out p);if(p!=pid)throw new Exception("Wrong owned main window");if(command>=0)ShowWindow(h,command);return new {hwnd=window,minimized=IsIconic(h)};}
 public static void Prepare(int pid,long edit,string path){var h=new IntPtr(edit);uint owner;uint target=GetWindowThreadProcessId(h,out owner);if(owner!=pid||Class(h)!="Edit"||!IsWindowEnabled(h))throw new Exception("Wrong or disabled owned address control");uint current=GetCurrentThreadId();if(!AttachThreadInput(current,target,true))throw new Exception("Could not attach owned input thread");try{SetForegroundWindow(new IntPtr(Window(pid)));SetFocus(h);if(GetFocus()!=h)throw new Exception("Owned address did not take native focus");IntPtr n;if(SendMessageTimeout(h,12,IntPtr.Zero,path,2,500,out n)==IntPtr.Zero)throw new Exception("Address update timed out");}finally{AttachThreadInput(current,target,false);}}
 public static void Enter(int pid,long edit){var h=new IntPtr(edit);uint owner;GetWindowThreadProcessId(h,out owner);if(owner!=pid||Class(h)!="Edit")throw new Exception("Wrong owned address control");PostMessage(h,0x100,(IntPtr)13,IntPtr.Zero);PostMessage(h,0x101,(IntPtr)13,IntPtr.Zero);}
}
'@
function Owned([int]$ProcessId){$image=[PerfNative]::Image($ProcessId);if(-not $image.StartsWith($root+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Refusing a process outside the owned fixture.'};return $ProcessId}
. (Join-Path $PSScriptRoot 'process-memory.ps1')
function Memory([int]$ProcessId){Get-SdirProcessMemory -ProcessId $ProcessId}

@{ready=$true}|ConvertTo-Json -Compress
while($line=[Console]::ReadLine()){
 try{$r=$line|ConvertFrom-Json;$p=Owned $r.pid;$value=switch($r.action){
  'controls'{@{main=[PerfNative]::Window($r.pid);controls=@([PerfNative]::Controls($r.pid))}}
  'ping'{[PerfNative]::Ping($r.pid)}
  'resize'{$w=if($r.width){[int]$r.width}else{1400};$h=if($r.height){[int]$r.height}else{900};[PerfNative]::Resize($r.pid,$w,$h);@{ok=$true}}
  'activate'{[PerfNative]::Activate($r.pid)}
  'state'{[PerfNative]::State($r.pid)}
  'close'{[PerfNative]::Close($r.pid,[long]$r.window)}
  'minimize'{[PerfNative]::WindowMode($r.pid,[long]$r.window,6)}
  'restore'{[PerfNative]::WindowMode($r.pid,[long]$r.window,9)}
  'window-mode'{[PerfNative]::WindowMode($r.pid,[long]$r.window,-1)}
  'prepare'{[PerfNative]::Prepare($r.pid,[long]$r.edit,$r.path);@{ok=$true}}
  'enter'{[PerfNative]::Enter($r.pid,[long]$r.edit);@{ok=$true}}
  'memory'{Memory $r.pid}
  default{throw 'Unknown action'}
 };@{id=$r.id;ok=$true;value=$value}|ConvertTo-Json -Depth 7 -Compress}
 catch{@{id=$r.id;ok=$false;error=$_.Exception.Message}|ConvertTo-Json -Compress}
}
