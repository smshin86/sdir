[CmdletBinding()]
param()

Set-StrictMode -Version Latest

if ($env:OS -ne 'Windows_NT') {
    throw 'installer-ui.ps1 is Windows-only.'
}

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

if (-not ('Sdir.Installer.Uat.NativeWindow' -as [type])) {
    Add-Type -Language CSharp -ReferencedAssemblies @('System.dll', 'System.Core.dll', 'System.Drawing.dll', 'Accessibility.dll') -TypeDefinition @'
using System;
using Accessibility;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using Microsoft.Win32.SafeHandles;
using System.Net.WebSockets;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace Sdir.Installer.Uat
{
    public sealed class NativeWindow
    {
        public IntPtr Handle;
        public uint ProcessId;
        public string Title;
        public string ClassName;
        public bool Visible;
        public bool Minimized;
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    public sealed class OutputChunk
    {
        public long Sequence;
        public string StreamName;
        public long Offset;
        public int Length;
    }

    public sealed class ProcessCapture
    {
        public int ProcessId;
        public DateTime StartedUtc;
        public int ExitCode;
        public bool HasExitCode;
        public byte[] StandardOutput;
        public byte[] StandardError;
        public List<OutputChunk> Chunks;
        public bool IsFinal;
        public bool ProcessExited;
        public bool OutputComplete;
        public bool CompletionTimedOut;
        public string CaptureError;
    }

    public sealed class OwnedProcess : IDisposable
    {
        private readonly Process process;
        private readonly int processId;
        private readonly DateTime startedUtc;
        private readonly MemoryStream stdout = new MemoryStream();
        private readonly MemoryStream stderr = new MemoryStream();
        private readonly List<OutputChunk> chunks = new List<OutputChunk>();
        private readonly object outputLock = new object();
        private readonly Task stdoutDrain;
        private readonly Task stderrDrain;
        private bool inputClosed;

        private OwnedProcess(Process started)
        {
            process = started;
            processId = started.Id;
            startedUtc = started.StartTime.ToUniversalTime();
            stdoutDrain = Task.Factory.StartNew(
                delegate { Drain(process.StandardOutput.BaseStream, stdout, "stdout"); },
                CancellationToken.None, TaskCreationOptions.LongRunning, TaskScheduler.Default);
            stderrDrain = Task.Factory.StartNew(
                delegate { Drain(process.StandardError.BaseStream, stderr, "stderr"); },
                CancellationToken.None, TaskCreationOptions.LongRunning, TaskScheduler.Default);
        }

        public static OwnedProcess Start(
            string executable, string arguments, string workingDirectory,
            string[] environmentPairs)
        {
            ProcessStartInfo info = new ProcessStartInfo();
            info.FileName = executable;
            info.Arguments = arguments;
            info.WorkingDirectory = workingDirectory;
            info.UseShellExecute = false;
            info.CreateNoWindow = true;
            info.RedirectStandardInput = true;
            info.RedirectStandardOutput = true;
            info.RedirectStandardError = true;
            if (environmentPairs != null)
            {
                foreach (string pair in environmentPairs)
                {
                    int separator = pair.IndexOf('=');
                    if (separator <= 0) throw new ArgumentException("Invalid environment pair.");
                    info.EnvironmentVariables[pair.Substring(0, separator)] = pair.Substring(separator + 1);
                }
            }
            Process child = new Process();
            child.StartInfo = info;
            if (!child.Start())
            {
                child.Dispose();
                throw new InvalidOperationException("Process.Start returned false.");
            }
            return new OwnedProcess(child);
        }

        private void Drain(Stream input, MemoryStream destination, string name)
        {
            byte[] buffer = new byte[8192];
            int read;
            while ((read = input.Read(buffer, 0, buffer.Length)) > 0)
            {
                lock (outputLock)
                {
                    long offset = destination.Length;
                    destination.Write(buffer, 0, read);
                    OutputChunk chunk = new OutputChunk();
                    chunk.Sequence = chunks.Count;
                    chunk.StreamName = name;
                    chunk.Offset = offset;
                    chunk.Length = read;
                    chunks.Add(chunk);
                }
            }
        }

        public int Id { get { return processId; } }
        public DateTime StartTimeUtc { get { return startedUtc; } }
        public bool HasExited { get { return process.HasExited; } }

        public void WriteInput(string value)
        {
            if (inputClosed) throw new InvalidOperationException("Standard input is closed.");
            byte[] bytes = new UTF8Encoding(false, true).GetBytes(value);
            process.StandardInput.BaseStream.Write(bytes, 0, bytes.Length);
            process.StandardInput.BaseStream.Flush();
        }

        public void CloseInput()
        {
            if (inputClosed) return;
            inputClosed = true;
            process.StandardInput.Close();
        }

        public bool WaitForExit(int milliseconds)
        {
            return process.WaitForExit(milliseconds);
        }

        private static int RemainingCompletionMilliseconds(int total, Stopwatch elapsed)
        {
            return (int)Math.Max(0L, (long)total - elapsed.ElapsedMilliseconds);
        }

        public ProcessCapture CaptureAfterExit(int remainingTotalMilliseconds)
        {
            Stopwatch elapsed = Stopwatch.StartNew();
            bool timedOut = remainingTotalMilliseconds <= 0;
            string error = null;
            if (!timedOut)
            {
                try
                {
                    if (!process.WaitForExit(RemainingCompletionMilliseconds(remainingTotalMilliseconds, elapsed)))
                        timedOut = true;
                    else if (!Task.WaitAll(new Task[] { stdoutDrain, stderrDrain },
                        RemainingCompletionMilliseconds(remainingTotalMilliseconds, elapsed)))
                        timedOut = true;
                    if (elapsed.ElapsedMilliseconds >= remainingTotalMilliseconds) timedOut = true;
                }
                catch (Exception ex) { error = ex.ToString(); }
            }
            return SnapshotCapture(true, timedOut, error);
        }

        private ProcessCapture SnapshotCapture(bool finalRequested, bool timedOut, string error)
        {
            ProcessCapture result = new ProcessCapture();
            result.ProcessId = processId;
            result.StartedUtc = startedUtc;
            result.CompletionTimedOut = timedOut;
            result.CaptureError = error;
            try
            {
                result.ProcessExited = process.HasExited;
                if (result.ProcessExited)
                {
                    result.ExitCode = process.ExitCode;
                    result.HasExitCode = true;
                }
            }
            catch (Exception ex)
            {
                result.CaptureError = (result.CaptureError ?? String.Empty) + ex.ToString();
            }
            if (stdoutDrain.IsFaulted)
                result.CaptureError = (result.CaptureError ?? String.Empty) + stdoutDrain.Exception.ToString();
            if (stderrDrain.IsFaulted)
                result.CaptureError = (result.CaptureError ?? String.Empty) + stderrDrain.Exception.ToString();
            result.OutputComplete = stdoutDrain.Status == TaskStatus.RanToCompletion &&
                stderrDrain.Status == TaskStatus.RanToCompletion;
            lock (outputLock)
            {
                result.StandardOutput = stdout.ToArray();
                result.StandardError = stderr.ToArray();
                result.Chunks = new List<OutputChunk>(chunks);
            }
            result.IsFinal = finalRequested && result.ProcessExited && result.HasExitCode &&
                result.OutputComplete && !result.CompletionTimedOut && result.CaptureError == null;
            return result;
        }

        public ProcessCapture CaptureWhileRunning()
        {
            return SnapshotCapture(false, false, null);
        }

        public void Dispose()
        {
            if (process.HasExited && stdoutDrain.Status == TaskStatus.RanToCompletion &&
                stderrDrain.Status == TaskStatus.RanToCompletion)
            {
                process.Dispose();
                stdout.Dispose();
                stderr.Dispose();
            }
        }
    }

    public sealed class NativeProcessObservation
    {
        public uint ProcessId;
        public long CreationUtcTicks;
        public long ExitUtcTicks;
        public bool Signaled;
        public string ImagePath;
        public bool UsedDeviceImageFallback;
        public int ImageQueryError;
        public string DeviceImagePath;
        public string DeviceVolumeTarget;
    }

    public sealed class NativeProcessExitObservation
    {
        public bool Signaled;
        public int ExitCode;
    }
    public sealed class NativeUninstallCompletion
    {
        public IntPtr DialogHandle { get; set; }
        public IntPtr OwnerHandle { get; set; }
        public IntPtr ButtonHandle { get; set; }
        public IntPtr MessageHandle { get; set; }
        public string Caption { get; set; }
        public string ButtonCaption { get; set; }
        public string Message { get; set; }
    }

    public static class NativeWindowApi
    {
        private delegate bool EnumWindowsCallback(IntPtr handle, IntPtr state);

        [DllImport("user32.dll", SetLastError = true)]
        private static extern bool EnumWindows(EnumWindowsCallback callback, IntPtr state);
        [DllImport("user32.dll", SetLastError = true)]
        private static extern bool EnumChildWindows(IntPtr parent, EnumWindowsCallback callback, IntPtr state);
        [DllImport("user32.dll", SetLastError = true)]
        private static extern uint GetWindowThreadProcessId(IntPtr handle, out uint processId);
        [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern int GetWindowText(IntPtr handle, StringBuilder text, int capacity);
        [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern int GetClassName(IntPtr handle, StringBuilder text, int capacity);
        [DllImport("user32.dll")]
        private static extern bool IsWindowVisible(IntPtr handle);
        [DllImport("user32.dll")]
        private static extern bool IsWindowEnabled(IntPtr handle);
        [DllImport("user32.dll")]
        private static extern bool IsChild(IntPtr parent, IntPtr child);
        [DllImport("user32.dll")]
        private static extern bool IsIconic(IntPtr handle);
        [DllImport("user32.dll", SetLastError = true)]
        private static extern bool GetWindowRect(IntPtr handle, out RECT rectangle);
        [DllImport("user32.dll", SetLastError = true)]
        private static extern bool ShowWindowAsync(IntPtr handle, int command);
        [DllImport("user32.dll", SetLastError = true)]
        private static extern bool SetForegroundWindow(IntPtr handle);
        [DllImport("user32.dll")]
        private static extern IntPtr GetForegroundWindow();
        [DllImport("user32.dll")]
        private static extern IntPtr GetWindow(IntPtr handle, uint command);
        [DllImport("user32.dll", SetLastError = true)]
        private static extern bool PostMessage(IntPtr handle, uint message, IntPtr wParam, IntPtr lParam);
        [DllImport("user32.dll")]
        private static extern bool IsWindow(IntPtr handle);
        [DllImport("user32.dll", SetLastError = true)]
        private static extern uint SendInput(uint count, INPUT[] inputs, int size);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern bool QueryFullProcessImageName(IntPtr process, uint flags, StringBuilder name, ref uint size);
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern IntPtr OpenProcess(uint access, bool inherit, uint processId);
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool CloseHandle(IntPtr handle);
        [DllImport("advapi32.dll", SetLastError = true)]
        private static extern bool OpenProcessToken(IntPtr process, uint desiredAccess, out IntPtr token);
        [DllImport("advapi32.dll", SetLastError = true)]
        private static extern bool GetTokenInformation(IntPtr token, int informationClass, out TOKEN_ELEVATION information, int informationLength, out int returnLength);
        [DllImport("user32.dll", SetLastError = true)]
        private static extern bool PrintWindow(IntPtr handle, IntPtr deviceContext, uint flags);

        private const int SW_RESTORE = 9;
        private const uint GW_OWNER = 4;
        private const uint WM_CLOSE = 0x0010;
        private const uint WM_LBUTTONDOWN = 0x0201;
        private const uint WM_LBUTTONUP = 0x0202;
        private const uint MK_LBUTTON = 0x0001;
        private const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
        private const uint TOKEN_QUERY = 0x0008;
        private const int TokenElevationClass = 20;
        private const uint PW_RENDERFULLCONTENT = 0x00000002;
        private const uint INPUT_KEYBOARD = 1;
        private const uint KEYEVENTF_KEYUP = 0x0002;

        [StructLayout(LayoutKind.Sequential)]
        private struct RECT { public int Left, Top, Right, Bottom; }
        [StructLayout(LayoutKind.Sequential)]
        private struct INPUT { public uint type; public InputUnion data; }
        [StructLayout(LayoutKind.Explicit)]
        private struct InputUnion { [FieldOffset(0)] public MOUSEINPUT mouse; [FieldOffset(0)] public KEYBDINPUT keyboard; }
        [StructLayout(LayoutKind.Sequential)]
        private struct MOUSEINPUT { public int dx, dy; public uint mouseData, dwFlags, time; public IntPtr extraInfo; }
        [StructLayout(LayoutKind.Sequential)]
        private struct KEYBDINPUT { public ushort virtualKey, scan; public uint flags, time; public IntPtr extraInfo; }
        [StructLayout(LayoutKind.Sequential)]
        private struct TOKEN_ELEVATION { public int TokenIsElevated; }

        private static string Text(IntPtr handle)
        {
            StringBuilder value = new StringBuilder(2048);
            GetWindowText(handle, value, value.Capacity);
            return value.ToString();
        }

        private static string Class(IntPtr handle)
        {
            StringBuilder value = new StringBuilder(256);
            GetClassName(handle, value, value.Capacity);
            return value.ToString();
        }

        private static NativeWindow ToRecord(IntPtr handle, uint pid)
        {
            RECT rect;
            GetWindowRect(handle, out rect);
            NativeWindow value = new NativeWindow();
            value.Handle = handle;
            value.ProcessId = pid;
            value.Title = Text(handle);
            value.ClassName = Class(handle);
            value.Visible = IsWindowVisible(handle);
            value.Minimized = IsIconic(handle);
            value.Left = rect.Left;
            value.Top = rect.Top;
            value.Right = rect.Right;
            value.Bottom = rect.Bottom;
            return value;
        }

        public static List<NativeWindow> TopLevelWindows(uint expectedPid, bool includeHidden)
        {
            List<NativeWindow> result = new List<NativeWindow>();
            EnumWindows(delegate(IntPtr handle, IntPtr state)
            {
                uint pid;
                GetWindowThreadProcessId(handle, out pid);
                if (pid == expectedPid && (includeHidden || IsWindowVisible(handle)))
                    result.Add(ToRecord(handle, pid));
                return true;
            }, IntPtr.Zero);
            return result;
        }

        public static List<NativeWindow> ChildWindows(IntPtr parent, uint expectedPid)
        {
            List<NativeWindow> result = new List<NativeWindow>();
            EnumChildWindows(parent, delegate(IntPtr handle, IntPtr state)
            {
                uint pid;
                GetWindowThreadProcessId(handle, out pid);
                if (pid == expectedPid) result.Add(ToRecord(handle, pid));
                return true;
            }, IntPtr.Zero);
            return result;
        }

        public static uint OwnerProcessId(IntPtr handle)
        {
            uint pid;
            GetWindowThreadProcessId(handle, out pid);
            return pid;
        }

        public static IntPtr OwnerWindow(IntPtr handle) { return GetWindow(handle, GW_OWNER); }

        public static bool IsLiveWindow(IntPtr handle) { return IsWindow(handle); }
        public static bool IsVisible(IntPtr handle) { return IsWindowVisible(handle); }
        public static bool IsMinimized(IntPtr handle) { return IsIconic(handle); }

        [StructLayout(LayoutKind.Sequential)]
        private struct NATIVE_FILETIME { public uint Low; public uint High; }

        [DllImport("kernel32.dll", EntryPoint = "GetProcessId", SetLastError = true)]
        private static extern uint GetNativeProcessId(SafeProcessHandle process);
        [DllImport("kernel32.dll", EntryPoint = "GetProcessTimes", SetLastError = true)]
        private static extern bool GetNativeProcessTimes(SafeProcessHandle process, out NATIVE_FILETIME creation, out NATIVE_FILETIME exit, out NATIVE_FILETIME kernel, out NATIVE_FILETIME user);
        [DllImport("kernel32.dll", EntryPoint = "WaitForSingleObject", SetLastError = true)]
        private static extern uint WaitForProcessSignal(SafeProcessHandle process, uint milliseconds);
        [DllImport("kernel32.dll", EntryPoint = "GetExitCodeProcess", SetLastError = true)]
        private static extern bool GetNativeProcessExitCode(SafeProcessHandle process, out uint exitCode);
        [DllImport("kernel32.dll", EntryPoint = "QueryFullProcessImageNameW", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
        private static extern bool QueryFullProcessImageNameW(SafeProcessHandle process, uint flags, StringBuilder name, ref uint size);
        [DllImport("kernel32.dll", EntryPoint = "K32GetProcessImageFileNameW", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
        private static extern uint GetProcessImageFileNameW(SafeProcessHandle process, StringBuilder name, uint size);
        [DllImport("kernel32.dll", EntryPoint = "QueryDosDeviceW", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
        private static extern uint QueryDosDeviceW(string deviceName, [Out] char[] targetPath, int maxChars);
        [DllImport("kernel32.dll", EntryPoint = "GetLogicalDrives", SetLastError = true)]
        private static extern uint GetLogicalDrives();

        private const uint WAIT_OBJECT_0_NATIVE = 0;
        private const uint WAIT_TIMEOUT_NATIVE = 258;
        private const uint WAIT_FAILED_NATIVE = 0xFFFFFFFF;

        private static void RequireSafeProcessHandle(SafeProcessHandle process)
        {
            if (process == null || process.IsInvalid || process.IsClosed)
                throw new InvalidOperationException("A valid retained SafeProcessHandle is required.");
        }

        private static long FileTimeToUtcTicks(NATIVE_FILETIME value)
        {
            ulong raw = ((ulong)value.High << 32) | (ulong)value.Low;
            if (raw == 0 || raw > (ulong)Int64.MaxValue)
                return 0;
            return DateTime.FromFileTimeUtc((long)raw).Ticks;
        }

        private static string ValidateLocalFullImagePath(string value, string description)
        {
            if (String.IsNullOrWhiteSpace(value) || value.Length < 3 || !Char.IsLetter(value[0]) ||
                value[1] != (char)58 || value[2] != (char)92)
                throw new InvalidOperationException(description + " must be a local absolute drive path.");
            string full = Path.GetFullPath(value);
            if (!String.Equals(full, value, StringComparison.OrdinalIgnoreCase))
                throw new InvalidOperationException(description + " is not a canonical full path.");
            return full;
        }

        private static bool IsDirectHarddiskVolumeTarget(string target)
        {
            string separator = ((char)92).ToString();
            string prefix = separator + "Device" + separator + "HarddiskVolume";
            if (String.IsNullOrEmpty(target) || !target.StartsWith(prefix, StringComparison.OrdinalIgnoreCase) || target.Length == prefix.Length)
                return false;
            for (int index = prefix.Length; index < target.Length; index++)
                if (!Char.IsDigit(target[index])) return false;
            return true;
        }

        private static string[] ReadDosDeviceTargets(string drive)
        {
            char[] buffer = new char[32768];
            uint length = QueryDosDeviceW(drive, buffer, buffer.Length);
            if (length == 0)
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "QueryDosDeviceW failed for an active local drive.");
            List<string> targets = new List<string>();
            int position = 0;
            while (position < buffer.Length && buffer[position] != (char)0)
            {
                int end = Array.IndexOf(buffer, (char)0, position);
                if (end < 0) throw new InvalidOperationException("QueryDosDeviceW returned an unterminated mapping.");
                targets.Add(new String(buffer, position, end - position));
                position = end + 1;
            }
            if (targets.Count == 0) throw new InvalidOperationException("QueryDosDeviceW returned no active mapping.");
            return targets.ToArray();
        }

        private static string MapDeviceImageToFirstLocalVolume(string deviceImagePath, string expectedImagePath, out string matchedTarget)
        {
            string expected = ValidateLocalFullImagePath(expectedImagePath, "Expected image path");
            if (String.IsNullOrWhiteSpace(deviceImagePath))
                throw new InvalidOperationException("The retained process has no device image path.");
            uint drives = GetLogicalDrives();
            if (drives == 0)
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "GetLogicalDrives failed.");
            for (int index = 0; index < 26; index++)
            {
                if ((drives & (1u << index)) == 0) continue;
                string letter = ((char)(65 + index)).ToString();
                string[] targets = ReadDosDeviceTargets(letter + ":");
                string firstTarget = targets[0];
                if (!IsDirectHarddiskVolumeTarget(firstTarget)) continue;
                int directCount = 0;
                foreach (string target in targets)
                    if (IsDirectHarddiskVolumeTarget(target)) directCount++;
                if (directCount != 1)
                    throw new InvalidOperationException("The active drive has an ambiguous HarddiskVolume mapping.");
                if (deviceImagePath.Length <= firstTarget.Length ||
                    !deviceImagePath.StartsWith(firstTarget, StringComparison.OrdinalIgnoreCase) ||
                    deviceImagePath[firstTarget.Length] != (char)92) continue;
                string mapped = ValidateLocalFullImagePath(letter + ":" + deviceImagePath.Substring(firstTarget.Length), "Mapped device image path");
                if (!String.Equals(mapped, expected, StringComparison.OrdinalIgnoreCase))
                    throw new InvalidOperationException("The first active local HarddiskVolume mapping does not match the complete CIM or pinned image path.");
                matchedTarget = firstTarget;
                return mapped;
            }
            throw new InvalidOperationException("The device image path has no supported first active local HarddiskVolume mapping.");
        }

        public static NativeProcessObservation ObserveProcessIdentity(SafeProcessHandle process, string expectedImagePath)
        {
            RequireSafeProcessHandle(process);
            string expected = String.IsNullOrWhiteSpace(expectedImagePath) ? null :
                ValidateLocalFullImagePath(expectedImagePath, "Expected CIM or pinned image path");
            uint processId = GetNativeProcessId(process);
            if (processId == 0)
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "GetProcessId failed on the retained handle.");
            NATIVE_FILETIME creationBefore, exitBefore, kernelBefore, userBefore;
            if (!GetNativeProcessTimes(process, out creationBefore, out exitBefore, out kernelBefore, out userBefore))
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "GetProcessTimes failed on the retained handle.");
            long creationTicks = FileTimeToUtcTicks(creationBefore);
            if (creationTicks <= 0) throw new InvalidOperationException("The retained handle has no native creation time.");
            StringBuilder image = new StringBuilder(32768);
            uint size = (uint)image.Capacity;
            bool imageRead = QueryFullProcessImageNameW(process, 0, image, ref size);
            int imageError = imageRead ? 0 : Marshal.GetLastWin32Error();
            string imagePath;
            string devicePath = null;
            string volumeTarget = null;
            bool usedFallback = false;
            if (imageRead)
            {
                imagePath = ValidateLocalFullImagePath(image.ToString(), "QueryFullProcessImageNameW result");
            }
            else
            {
                if (imageError != 31)
                    throw new System.ComponentModel.Win32Exception(imageError, "QueryFullProcessImageNameW failed outside the reviewed error-31 exit case.");
                if (expected == null)
                    throw new InvalidOperationException("Error-31 device fallback requires a nonempty CIM or pinned image path.");
                uint waitBeforeFallback = WaitForProcessSignal(process, 0);
                if (waitBeforeFallback != WAIT_OBJECT_0_NATIVE)
                    throw new InvalidOperationException("QueryFullProcessImageNameW error 31 did not coincide with a signaled retained process handle.");
                NATIVE_FILETIME creationAtExit, exitAtExit, kernelAtExit, userAtExit;
                if (!GetNativeProcessTimes(process, out creationAtExit, out exitAtExit, out kernelAtExit, out userAtExit) ||
                    FileTimeToUtcTicks(creationAtExit) != creationTicks || FileTimeToUtcTicks(exitAtExit) < creationTicks)
                    throw new InvalidOperationException("The signaled retained process handle did not preserve a valid native creation and exit interval.");
                StringBuilder device = new StringBuilder(32768);
                uint deviceLength = GetProcessImageFileNameW(process, device, (uint)device.Capacity);
                if (deviceLength == 0)
                    throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "K32GetProcessImageFileNameW failed on the retained handle.");
                devicePath = device.ToString();
                imagePath = MapDeviceImageToFirstLocalVolume(devicePath, expected, out volumeTarget);
                usedFallback = true;
            }
            if (expected != null && !String.Equals(imagePath, expected, StringComparison.OrdinalIgnoreCase))
                throw new InvalidOperationException("The complete retained native image path differs from the current CIM or pinned image path.");
            uint wait = WaitForProcessSignal(process, 0);
            bool signaled;
            if (wait == WAIT_OBJECT_0_NATIVE) signaled = true;
            else if (wait == WAIT_TIMEOUT_NATIVE) signaled = false;
            else if (wait == WAIT_FAILED_NATIVE)
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "WaitForSingleObject failed on the retained process handle.");
            else throw new InvalidOperationException("WaitForSingleObject returned an unsupported process wait state.");
            NATIVE_FILETIME creationAfter, exitAfter, kernelAfter, userAfter;
            if (!GetNativeProcessTimes(process, out creationAfter, out exitAfter, out kernelAfter, out userAfter))
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "GetProcessTimes failed during retained identity confirmation.");
            if (FileTimeToUtcTicks(creationAfter) != creationTicks)
                throw new InvalidOperationException("The retained handle native creation time changed during identity confirmation.");
            long exitTicks = FileTimeToUtcTicks(exitAfter);
            if (exitTicks != 0 && !signaled)
            {
                uint confirmWait = WaitForProcessSignal(process, 0);
                if (confirmWait != WAIT_OBJECT_0_NATIVE)
                    throw new InvalidOperationException("A native exit time was present without a signaled retained process handle.");
                signaled = true;
            }
            if ((signaled && exitTicks < creationTicks) || (!signaled && exitTicks != 0))
                throw new InvalidOperationException("The retained process signal and native exit time are inconsistent.");
            return new NativeProcessObservation
            {
                ProcessId = processId, CreationUtcTicks = creationTicks, ExitUtcTicks = exitTicks,
                Signaled = signaled, ImagePath = imagePath, UsedDeviceImageFallback = usedFallback,
                ImageQueryError = imageError, DeviceImagePath = devicePath, DeviceVolumeTarget = volumeTarget
            };
        }

        public static NativeProcessExitObservation ReadSignaledProcessExitStatus(SafeProcessHandle process)
        {
            RequireSafeProcessHandle(process);
            uint wait = WaitForProcessSignal(process, 0);
            if (wait == WAIT_TIMEOUT_NATIVE) return new NativeProcessExitObservation { Signaled = false, ExitCode = 0 };
            if (wait == WAIT_FAILED_NATIVE)
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "WaitForSingleObject failed before native exit-code capture.");
            if (wait != WAIT_OBJECT_0_NATIVE)
                throw new InvalidOperationException("The retained process handle returned an unsupported wait state before native exit-code capture.");
            uint exitCode;
            if (!GetNativeProcessExitCode(process, out exitCode))
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "GetExitCodeProcess failed after the retained handle was signaled.");
            return new NativeProcessExitObservation { Signaled = true, ExitCode = unchecked((int)exitCode) };
        }
        public static string ProcessImagePath(int processId)
        {
            IntPtr handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, (uint)processId);
            if (handle == IntPtr.Zero) throw new InvalidOperationException("OpenProcess failed for owned PID " + processId);
            try
            {
                StringBuilder name = new StringBuilder(32768);
                uint size = (uint)name.Capacity;
                if (!QueryFullProcessImageName(handle, 0, name, ref size))
                    throw new InvalidOperationException("QueryFullProcessImageName failed for owned PID " + processId);
                return name.ToString();
            }
            finally { CloseHandle(handle); }
        }

        public static bool IsProcessElevated(int processId)
        {
            IntPtr process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, (uint)processId);
            if (process == IntPtr.Zero) throw new InvalidOperationException("OpenProcess failed while checking token elevation.");
            IntPtr token = IntPtr.Zero;
            try
            {
                if (!OpenProcessToken(process, TOKEN_QUERY, out token))
                    throw new InvalidOperationException("OpenProcessToken failed while checking token elevation.");
                TOKEN_ELEVATION elevation;
                int returned;
                if (!GetTokenInformation(token, TokenElevationClass, out elevation,
                    Marshal.SizeOf(typeof(TOKEN_ELEVATION)), out returned))
                    throw new InvalidOperationException("GetTokenInformation(TokenElevation) failed.");
                return elevation.TokenIsElevated != 0;
            }
            finally
            {
                if (token != IntPtr.Zero) CloseHandle(token);
                CloseHandle(process);
            }
        }

        public static bool RestoreAndFocus(IntPtr handle, uint expectedPid)
        {
            if (!IsWindow(handle) || OwnerProcessId(handle) != expectedPid) return false;
            ShowWindowAsync(handle, SW_RESTORE);
            Stopwatch deadline = Stopwatch.StartNew();
            while (deadline.ElapsedMilliseconds < 2000)
            {
                if (!IsWindow(handle) || OwnerProcessId(handle) != expectedPid) return false;
                if (IsWindowVisible(handle) && !IsIconic(handle))
                {
                    SetForegroundWindow(handle);
                    if (GetForegroundWindow() == handle) return true;
                }
                Thread.Sleep(25);
            }
            return false;
        }

        public static bool SetOwnedWindowTestState(IntPtr handle, uint expectedPid, bool minimize)
        {
            if (!IsWindow(handle) || OwnerProcessId(handle) != expectedPid) return false;
            ShowWindowAsync(handle, minimize ? 6 : 0);
            return true; // Request only. The caller must observe the resulting state.
        }

        public static bool PostButtonClick(IntPtr handle, uint expectedPid)
        {
            if (!IsWindow(handle) || OwnerProcessId(handle) != expectedPid || !IsWindowVisible(handle)) return false;
            bool down = PostMessage(handle, WM_LBUTTONDOWN, new IntPtr(MK_LBUTTON), IntPtr.Zero);
            bool up = PostMessage(handle, WM_LBUTTONUP, IntPtr.Zero, IntPtr.Zero);
            return down && up;
        }

        public static bool IsEnabled(IntPtr handle) { return IsWindowEnabled(handle); }

        public static bool IsDescendant(IntPtr parent, IntPtr child) { return IsChild(parent, child); }

        private static bool IsOwnedInnoWizard(IntPtr wizard, uint expectedPid)
        {
            RECT bounds;
            return IsWindow(wizard) && OwnerProcessId(wizard) == expectedPid &&
                Class(wizard) == "TWizardForm" && GetAncestor(wizard, 2) == wizard &&
                IsWindowVisible(wizard) && IsWindowEnabled(wizard) && !IsIconic(wizard) &&
                GetWindowRect(wizard, out bounds) && bounds.Right > bounds.Left && bounds.Bottom > bounds.Top;
        }

        private static bool IsOwnedInnoChild(IntPtr wizard, IntPtr handle, uint expectedPid, string className)
        {
            RECT bounds;
            return IsWindow(handle) && OwnerProcessId(handle) == expectedPid &&
                IsChild(wizard, handle) && GetAncestor(handle, 2) == wizard && Class(handle) == className &&
                IsWindowVisible(handle) && IsWindowEnabled(handle) && GetWindowRect(handle, out bounds) &&
                bounds.Right > bounds.Left && bounds.Bottom > bounds.Top;
        }

        public static bool IsInnoButtonCaption(string role, string caption, bool native)
        {
            if (native)
            {
                switch (role)
                {
                    case "Next": return caption == "\uB2E4\uC74C(&N)" || caption == "\uB2E4\uC74C(&N) >" || caption == "&Next" || caption == "&Next >";
                    case "Back": return caption == "\uB4A4\uB85C(&B)" || caption == "< \uB4A4\uB85C(&B)" || caption == "&Back" || caption == "< &Back";
                    case "Install": return caption == "\uC124\uCE58(&I)" || caption == "&Install";
                    case "Finish": return caption == "\uB9C8\uCE68(&F)" || caption == "&Finish";
                    case "Cancel": return caption == "\uCDE8\uC18C" || caption == "Cancel";
                    default: return false;
                }
            }
            switch (role)
            {
                case "Next": return caption == "\uB2E4\uC74C(N)" || caption == "\uB2E4\uC74C(N) >" || caption == "Next" || caption == "Next >";
                case "Back": return caption == "\uB4A4\uB85C(B)" || caption == "< \uB4A4\uB85C(B)" || caption == "Back" || caption == "< Back";
                case "Install": return caption == "\uC124\uCE58(I)" || caption == "Install";
                case "Finish": return caption == "\uB9C8\uCE68(F)" || caption == "Finish";
                case "Cancel": return caption == "\uCDE8\uC18C" || caption == "Cancel";
                default: return false;
            }
        }

        public static List<NativeWindow> OwnedInnoButtons(IntPtr wizard, uint expectedPid, string role)
        {
            var matches = new List<NativeWindow>();
            if (!IsOwnedInnoWizard(wizard, expectedPid)) return matches;
            foreach (NativeWindow child in ChildWindows(wizard, expectedPid))
            {
                if (child.ClassName == "TNewButton" && IsInnoButtonCaption(role, child.Title, true))
                    matches.Add(child);
            }
            return matches;
        }

        public static bool PostOwnedInnoButtonClick(IntPtr wizard, IntPtr handle, uint expectedPid, string role, string caption)
        {
            var buttons = OwnedInnoButtons(wizard, expectedPid, role);
            if (buttons.Count != 1 || buttons[0].Handle != handle || buttons[0].Title != caption ||
                !IsOwnedInnoWizard(wizard, expectedPid) || !IsOwnedInnoChild(wizard, handle, expectedPid, "TNewButton") ||
                Text(handle) != caption) return false;
            return PostButtonClick(handle, expectedPid);
        }

        [DllImport("user32.dll")]
        private static extern IntPtr GetAncestor(IntPtr handle, uint flags);

        [DllImport("user32.dll", SetLastError = true)]
        private static extern bool GetGUIThreadInfo(uint threadId, ref GUI_THREAD_INFO info);

        [DllImport("user32.dll")]
        private static extern short GetAsyncKeyState(int key);

        [DllImport("oleacc.dll", ExactSpelling = true)]
        private static extern int AccessibleObjectFromWindow(IntPtr handle, uint objectId,
            ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out IAccessible accessible);

        [StructLayout(LayoutKind.Sequential)]
        private struct GUI_THREAD_INFO
        {
            public int Size;
            public uint Flags;
            public IntPtr Active, Focus, Capture, MenuOwner, MoveSize, Caret;
            public RECT CaretBounds;
        }

        public sealed class InnoCheckItem
        {
            public IntPtr Handle;
            public int ChildId, Role, State, Left, Top, Width, Height;
            public string Name;
            public bool Checked { get { return (State & 0x10) != 0; } }
        }

        public static bool IsInnoCheckItem(string name, string expectedName, int role, int state, int width, int height)
        {
            const int invalidState = 0x0001 | 0x0020 | 0x0040 | 0x8000 | 0x10000;
            return name == expectedName && role == 0x2c && (state & invalidState) == 0 && width > 0 && height > 0;
        }

        private static IAccessible InnoAccessible(IntPtr handle)
        {
            Guid iid = typeof(IAccessible).GUID;
            IAccessible accessible;
            int result = AccessibleObjectFromWindow(handle, unchecked((uint)-4), ref iid, out accessible);
            if (result != 0 || accessible == null)
                throw new InvalidOperationException(String.Format("MSAA OBJID_CLIENT returned HRESULT 0x{0:X8}.", result));
            return accessible;
        }

        public static InnoCheckItem ReadOwnedInnoCheckItem(IntPtr wizard, uint expectedPid, string expectedName)
        {
            InnoCheckItem match = null;
            if (!IsOwnedInnoWizard(wizard, expectedPid)) return null;
            foreach (NativeWindow child in ChildWindows(wizard, expectedPid))
            {
                if (child.ClassName != "TNewCheckListBox" || !child.Visible) continue;
                if (!IsOwnedInnoChild(wizard, child.Handle, expectedPid, "TNewCheckListBox"))
                    throw new InvalidOperationException("The visible owned Inno checklist was not enabled and positively sized.");
                IAccessible accessible = InnoAccessible(child.Handle);
                try
                {
                    int count = accessible.accChildCount;
                    if (count < 0 || count > 256)
                        throw new InvalidOperationException("Unexpected MSAA checklist child count.");
                    for (int childId = 1; childId <= count; childId++)
                    {
                        string name = accessible.get_accName(childId);
                        if (name != expectedName) continue;
                        if (match != null) throw new InvalidOperationException("Multiple exact MSAA checklist names were exposed.");
                        var item = new InnoCheckItem { Handle = child.Handle, ChildId = childId, Name = name };
                        object role = accessible.get_accRole(childId);
                        object state = accessible.get_accState(childId);
                        if (!(role is int) || !(state is int))
                            throw new InvalidOperationException("The MSAA checklist role or state was not a VT_I4.");
                        item.Role = (int)role;
                        item.State = (int)state;
                        accessible.accLocation(out item.Left, out item.Top, out item.Width, out item.Height, childId);
                        if (!IsInnoCheckItem(name, expectedName, item.Role, item.State, item.Width, item.Height) ||
                            item.Left >= child.Right || item.Top >= child.Bottom ||
                            (long)item.Left + item.Width <= child.Left || (long)item.Top + item.Height <= child.Top)
                            throw new InvalidOperationException("The named MSAA checkbox lacks the required role, state or visible bounds.");
                        match = item;
                    }
                }
                finally { Marshal.ReleaseComObject(accessible); }
            }
            return match;
        }

        public static bool SameInnoCheckItem(InnoCheckItem expected, InnoCheckItem actual)
        {
            return expected != null && actual != null && expected.Handle == actual.Handle &&
                expected.ChildId == actual.ChildId && expected.Role == actual.Role && expected.Name == actual.Name;
        }

        public static InnoCheckItem PostOwnedInnoCheckItemSpace(IntPtr wizard, uint expectedPid, InnoCheckItem expected)
        {
            if (expected == null || !IsOwnedInnoWizard(wizard, expectedPid))
                throw new InvalidOperationException("The owned checklist wizard is unavailable.");
            InnoCheckItem before = ReadOwnedInnoCheckItem(wizard, expectedPid, expected.Name);
            if (!SameInnoCheckItem(expected, before) || before.Checked != expected.Checked)
                throw new InvalidOperationException("The MSAA checkbox changed before selection.");

            IAccessible accessible = InnoAccessible(before.Handle);
            try
            {
                // Inno 6.7.4 delegates TAKESELECTION to the standard list box accessibility object.
                if (!Object.Equals(accessible.accSelection, before.ChildId))
                    accessible.accSelect(0x2, before.ChildId);
                InnoCheckItem selected = ReadOwnedInnoCheckItem(wizard, expectedPid, before.Name);
                if (!SameInnoCheckItem(before, selected) || selected.Checked != before.Checked ||
                    !Object.Equals(accessible.accSelection, before.ChildId))
                    throw new InvalidOperationException("Typed MSAA did not confirm the same selected checkbox.");

                uint ownerPid;
                uint threadId = GetWindowThreadProcessId(before.Handle, out ownerPid);
                var info = new GUI_THREAD_INFO { Size = Marshal.SizeOf(typeof(GUI_THREAD_INFO)) };
                if (!IsOwnedInnoWizard(wizard, expectedPid) ||
                    !IsOwnedInnoChild(wizard, before.Handle, expectedPid, "TNewCheckListBox") ||
                    ownerPid != expectedPid || threadId == 0 || !GetGUIThreadInfo(threadId, ref info) ||
                    info.Active != wizard || info.Focus != before.Handle || info.Capture != IntPtr.Zero ||
                    GetAsyncKeyState(0x10) < 0 || GetAsyncKeyState(0x11) < 0 ||
                    GetAsyncKeyState(0x12) < 0 || GetAsyncKeyState(0x20) < 0)
                    throw new InvalidOperationException("The owned GUI thread did not authorize a checklist Space message.");

                // VK_SPACE KeyDown/KeyUp reach the selected list item without desktop-wide input.
                uint scan = MapVirtualKey(0x20, 0);
                if (scan == 0 || scan > 0xff) throw new InvalidOperationException("No Space scan code was available.");
                int keyData = 1 | ((int)scan << 16);
                if (!PostMessage(before.Handle, 0x0100, new IntPtr(0x20), new IntPtr(keyData)) ||
                    !PostMessage(before.Handle, 0x0101, new IntPtr(0x20), new IntPtr(keyData | unchecked((int)0xc0000000))))
                    throw new InvalidOperationException("The checklist did not accept the Space message pair.");
                return selected;
            }
            finally { Marshal.ReleaseComObject(accessible); }
        }

        [DllImport("user32.dll", EntryPoint = "MapVirtualKeyW", ExactSpelling = true)]
        private static extern uint MapVirtualKey(uint code, uint mapType);

        [DllImport("user32.dll")]
        public static extern int GetDlgCtrlID(IntPtr handle);

        [DllImport("user32.dll")]
        private static extern IntPtr GetDlgItem(IntPtr dialog, int id);

        public static bool IsInnoExitCaption(string caption)
        {
            return caption == "\uC124\uCE58 \uC885\uB8CC" || caption == "Exit Setup";
        }

        public static bool IsInnoExitChoice(string role, int id, string caption, bool native)
        {
            // Winuser.h: IDYES = 6, IDNO = 7.
            if (role == "Yes" && id == 6)
                return native ? caption == "\uC608(&Y)" || caption == "&Yes" :
                    caption == "\uC608(Y)" || caption == "Yes";
            if (role == "No" && id == 7)
                return native ? caption == "\uC544\uB2C8\uC694(&N)" || caption == "&No" :
                    caption == "\uC544\uB2C8\uC694(N)" || caption == "No";
            return false;
        }

        public static List<NativeWindow> OwnedInnoExitChoices(IntPtr wizard, IntPtr dialog, uint expectedPid, string role)
        {
            var matches = new List<NativeWindow>();
            RECT bounds;
            if (wizard == dialog || !IsWindow(wizard) || OwnerProcessId(wizard) != expectedPid ||
                Class(wizard) != "TWizardForm" || GetAncestor(wizard, 2) != wizard ||
                !IsWindowVisible(wizard) || IsIconic(wizard) || IsWindowEnabled(wizard) ||
                !IsWindow(dialog) || OwnerProcessId(dialog) != expectedPid || OwnerWindow(dialog) != wizard ||
                GetAncestor(dialog, 2) != dialog || Class(dialog) != "#32770" || !IsInnoExitCaption(Text(dialog)) ||
                !IsWindowVisible(dialog) || !IsWindowEnabled(dialog) || IsIconic(dialog) ||
                !GetWindowRect(dialog, out bounds) || bounds.Right <= bounds.Left || bounds.Bottom <= bounds.Top)
                return matches;
            foreach (NativeWindow child in ChildWindows(dialog, expectedPid))
            {
                int id = GetDlgCtrlID(child.Handle);
                if (IsInnoExitChoice(role, id, child.Title, true) && GetDlgItem(dialog, id) == child.Handle &&
                    GetAncestor(child.Handle, 1) == dialog && IsOwnedInnoChild(dialog, child.Handle, expectedPid, "Button"))
                    matches.Add(child);
            }
            return matches;
        }

        public static bool PostOwnedInnoExitChoice(IntPtr wizard, IntPtr dialog, IntPtr handle, uint expectedPid,
            string role, int id, string caption)
        {
            var choices = OwnedInnoExitChoices(wizard, dialog, expectedPid, role);
            return choices.Count == 1 && choices[0].Handle == handle && choices[0].Title == caption &&
                GetDlgCtrlID(handle) == id && IsInnoExitChoice(role, id, Text(handle), true) &&
                PostButtonClick(handle, expectedPid);
        }

        public static bool IsInnoUninstallChoice(string role, int id, string caption)
        {
            return (role == "Yes" && id == 6 && caption == "\uC608(&Y)") ||
                (role == "No" && id == 7 && caption == "\uC544\uB2C8\uC694(&N)");
        }

        public static List<NativeWindow> OwnedInnoUninstallChoices(IntPtr dialog, uint expectedPid)
        {
            var matches = new List<NativeWindow>();
            RECT bounds;
            if (expectedPid == 0 || !IsWindow(dialog) || OwnerProcessId(dialog) != expectedPid ||
                OwnerWindow(dialog) != IntPtr.Zero || GetAncestor(dialog, 2) != dialog ||
                GetAncestor(dialog, 3) != dialog || Class(dialog) != "#32770" ||
                Text(dialog) != InnoUninstallCaption ||
                !IsWindowVisible(dialog) || !IsWindowEnabled(dialog) || IsIconic(dialog) ||
                !GetWindowRect(dialog, out bounds) || bounds.Right <= bounds.Left || bounds.Bottom <= bounds.Top)
                return matches;

            List<NativeWindow> windows = TopLevelWindows(expectedPid, false);
            if (windows.Count != 1 || windows[0].Handle != dialog) return matches;

            List<NativeWindow> children = ChildWindows(dialog, expectedPid);
            foreach (string role in new[] { "Yes", "No" })
            {
                int id = role == "Yes" ? 6 : 7;
                NativeWindow choice = null;
                foreach (NativeWindow child in children)
                {
                    if (GetDlgCtrlID(child.Handle) != id) continue;
                    if (choice != null || !IsInnoUninstallChoice(role, id, child.Title) ||
                        GetDlgItem(dialog, id) != child.Handle || GetAncestor(child.Handle, 1) != dialog ||
                        !IsOwnedInnoChild(dialog, child.Handle, expectedPid, "Button"))
                        return new List<NativeWindow>();
                    choice = child;
                }
                if (choice == null) return new List<NativeWindow>();
                matches.Add(choice);
            }
            return matches;
        }

        public static bool PostOwnedInnoUninstallChoice(IntPtr dialog, IntPtr handle, SafeProcessHandle process,
            uint expectedPid, long expectedStartTicks, string expectedImage, string role, int id, string caption)
        {
            if (!IsInnoUninstallChoice(role, id, caption)) return false;
            List<NativeWindow> choices = OwnedInnoUninstallChoices(dialog, process, expectedPid, expectedStartTicks, expectedImage);
            if (choices.Count != 2) return false;
            NativeWindow choice = choices[id - 6];
            return choice.Handle == handle && GetDlgCtrlID(handle) == id &&
                IsInnoUninstallChoice(role, id, Text(handle)) &&
                GetAncestor(handle, 1) == dialog && IsOwnedInnoChild(dialog, handle, expectedPid, "Button") &&
                PostButtonClick(handle, expectedPid);
        }

        private static bool IsLiveRetainedUninstallProcess(SafeProcessHandle process,
            uint expectedPid, long expectedStartTicks, string expectedImage)
        {
            NativeProcessObservation current = ObserveProcessIdentity(process, expectedImage);
            return !current.Signaled && current.ProcessId == expectedPid && current.CreationUtcTicks == expectedStartTicks &&
                String.Equals(current.ImagePath, expectedImage, StringComparison.OrdinalIgnoreCase);
        }

        public static List<NativeWindow> OwnedInnoUninstallChoices(IntPtr dialog, SafeProcessHandle process,
            uint expectedPid, long expectedStartTicks, string expectedImage)
        {
            return IsLiveRetainedUninstallProcess(process, expectedPid, expectedStartTicks, expectedImage) ?
                OwnedInnoUninstallChoices(dialog, expectedPid) : new List<NativeWindow>();
        }

        private const string InnoUninstallCaption = "SDIR UAT \uC81C\uAC70";
        private const int InnoUninstallCompletionId = 2; // Observed Inno Setup 6.7.4 Korean completion button.

        [DllImport("user32.dll", EntryPoint = "SendMessageTimeoutW", CharSet = CharSet.Unicode, ExactSpelling = true)]
        private static extern IntPtr ReadUninstallControlText(IntPtr handle, uint message, UIntPtr capacity,
            [Out] StringBuilder value, uint flags, uint timeoutMilliseconds, out UIntPtr result);

        private static string UninstallControlText(IntPtr handle)
        {
            const uint wmGetText = 0x000D;
            const uint abortIfHungOrExited = 0x0022;
            StringBuilder value = new StringBuilder(512);
            UIntPtr length;
            if (ReadUninstallControlText(handle, wmGetText, new UIntPtr((uint)value.Capacity),
                    value, abortIfHungOrExited, 100, out length) == IntPtr.Zero ||
                length.ToUInt64() >= (ulong)(value.Capacity - 1))
                return null;
            return value.ToString();
        }

        public static bool IsInnoUninstallCompletionMessage(string text)
        {
            // Inno Setup 6.7.4 Korean.isl, UninstalledAll / UninstalledMost, with the UAT app name.
            return text == "SDIR UAT이(가) 컴퓨터에서 성공적으로 제거되었습니다." ||
                text == "SDIR UAT 제거가 완료되었습니다.\r\n\r\n일부 요소를 제거할 수 없습니다. 수동으로 제거할 수 있습니다.";
        }

        public static bool IsInnoUninstallCompletionButton(int id, string caption)
        {
            return id == InnoUninstallCompletionId && caption == "\uD655\uC778";
        }

        private static bool IsOwnedInnoUninstallCompletionDialog(IntPtr dialog, IntPtr owner, uint expectedPid)
        {
            RECT bounds;
            if (expectedPid == 0 || !IsWindow(dialog) || OwnerProcessId(dialog) != expectedPid ||
                OwnerWindow(dialog) != owner || GetAncestor(dialog, 2) != dialog ||
                Class(dialog) != "#32770" || Text(dialog) != InnoUninstallCaption ||
                !IsWindowVisible(dialog) || !IsWindowEnabled(dialog) || IsIconic(dialog) ||
                !GetWindowRect(dialog, out bounds) || bounds.Right <= bounds.Left || bounds.Bottom <= bounds.Top)
                return false;

            if (owner == IntPtr.Zero) return GetAncestor(dialog, 3) == dialog;
            if (!IsWindow(owner) || OwnerProcessId(owner) != expectedPid || GetAncestor(owner, 2) != owner)
                return false;
            string ownerClass = Class(owner);
            return ownerClass == "TApplication" || ownerClass == "TUninstallProgressForm";
        }

        public static NativeUninstallCompletion FindOwnedInnoUninstallCompletion(SafeProcessHandle process,
            uint expectedPid, long expectedStartTicks, string expectedImage)
        {
            if (!IsLiveRetainedUninstallProcess(process, expectedPid, expectedStartTicks, expectedImage)) return null;

            NativeUninstallCompletion match = null;
            foreach (NativeWindow window in TopLevelWindows(expectedPid, false))
            {
                IntPtr dialog = window.Handle;
                IntPtr owner = OwnerWindow(dialog);
                if (!IsOwnedInnoUninstallCompletionDialog(dialog, owner, expectedPid)) continue;

                IntPtr button = GetDlgItem(dialog, InnoUninstallCompletionId);
                if (GetAncestor(button, 1) != dialog || !IsOwnedInnoChild(dialog, button, expectedPid, "Button"))
                    continue;
                string caption = UninstallControlText(button);
                if (!IsInnoUninstallCompletionButton(GetDlgCtrlID(button), caption)) continue;

                IntPtr message = IntPtr.Zero;
                string body = null;
                int visibleButtons = 0;
                int okControls = 0;
                bool unexpectedText = false;
                foreach (NativeWindow child in ChildWindows(dialog, expectedPid))
                {
                    if (GetAncestor(child.Handle, 1) != dialog) continue;
                    if (GetDlgCtrlID(child.Handle) == InnoUninstallCompletionId) okControls++;
                    if (!child.Visible) continue;
                    if (child.ClassName == "Button") visibleButtons++;
                    if (child.ClassName != "Static") continue;
                    string text = UninstallControlText(child.Handle);
                    if (text == "") continue; // The icon control has no message text.
                    if (!IsInnoUninstallCompletionMessage(text) ||
                        !IsOwnedInnoChild(dialog, child.Handle, expectedPid, "Static"))
                    {
                        unexpectedText = true;
                        continue;
                    }
                    if (message != IntPtr.Zero) return null;
                    message = child.Handle;
                    body = text;
                }
                if (visibleButtons != 1 || okControls != 1 || unexpectedText || message == IntPtr.Zero) continue;
                if (match != null) return null;
                match = new NativeUninstallCompletion
                {
                    DialogHandle = dialog, OwnerHandle = owner, ButtonHandle = button, MessageHandle = message,
                    Caption = window.Title, ButtonCaption = caption, Message = body
                };
            }

            return match != null && IsLiveRetainedUninstallProcess(process, expectedPid, expectedStartTicks, expectedImage) ?
                match : null;
        }

        public static bool PostOwnedInnoUninstallCompletion(NativeUninstallCompletion expected,
            SafeProcessHandle process, uint expectedPid, long expectedStartTicks, string expectedImage)
        {
            if (expected == null) return false;
            NativeUninstallCompletion current = FindOwnedInnoUninstallCompletion(process, expectedPid, expectedStartTicks, expectedImage);
            if (current == null || current.DialogHandle != expected.DialogHandle || current.OwnerHandle != expected.OwnerHandle ||
                current.ButtonHandle != expected.ButtonHandle || current.MessageHandle != expected.MessageHandle ||
                current.Caption != expected.Caption || current.ButtonCaption != expected.ButtonCaption || current.Message != expected.Message)
                return false;

            IntPtr dialog = current.DialogHandle;
            IntPtr button = current.ButtonHandle;
            return IsOwnedInnoUninstallCompletionDialog(dialog, current.OwnerHandle, expectedPid) &&
                GetDlgItem(dialog, InnoUninstallCompletionId) == button && GetAncestor(button, 1) == dialog &&
                GetAncestor(current.MessageHandle, 1) == dialog &&
                IsOwnedInnoChild(dialog, current.MessageHandle, expectedPid, "Static") &&
                IsOwnedInnoChild(dialog, button, expectedPid, "Button") &&
                GetDlgCtrlID(button) == InnoUninstallCompletionId && PostButtonClick(button, expectedPid);
        }

        public static bool PostClose(IntPtr handle, uint expectedPid)
        {
            if (!IsWindow(handle) || OwnerProcessId(handle) != expectedPid) return false;
            return PostMessage(handle, WM_CLOSE, IntPtr.Zero, IntPtr.Zero);
        }

        public static bool SendVirtualKey(ushort virtualKey)
        {
            INPUT[] inputs = new INPUT[2];
            inputs[0].type = INPUT_KEYBOARD;
            inputs[0].data.keyboard.virtualKey = virtualKey;
            inputs[1].type = INPUT_KEYBOARD;
            inputs[1].data.keyboard.virtualKey = virtualKey;
            inputs[1].data.keyboard.flags = KEYEVENTF_KEYUP;
            return SendInput((uint)inputs.Length, inputs, Marshal.SizeOf(typeof(INPUT))) == inputs.Length;
        }

        public static bool CaptureWindowPng(IntPtr handle, uint expectedPid, string path)
        {
            if (!IsWindow(handle) || OwnerProcessId(handle) != expectedPid) return false;
            RECT rect;
            if (!GetWindowRect(handle, out rect)) return false;
            int width = rect.Right - rect.Left;
            int height = rect.Bottom - rect.Top;
            if (width < 1 || height < 1 || width > 12000 || height > 12000) return false;
            using (Bitmap bitmap = new Bitmap(width, height, PixelFormat.Format32bppArgb))
            using (Graphics graphics = Graphics.FromImage(bitmap))
            {
                IntPtr hdc = graphics.GetHdc();
                bool captured;
                try { captured = PrintWindow(handle, hdc, PW_RENDERFULLCONTENT); }
                finally { graphics.ReleaseHdc(hdc); }
                if (!captured) return false;
                bitmap.Save(path, ImageFormat.Png);
                return new FileInfo(path).Length > 0;
            }
        }
    }

    public sealed class CdpSocket : IDisposable
    {
        private readonly ClientWebSocket socket = new ClientWebSocket();
        private readonly object sendLock = new object();

        public void Connect(string uri, int timeoutMilliseconds)
        {
            CancellationTokenSource timeout = new CancellationTokenSource(timeoutMilliseconds);
            try { socket.ConnectAsync(new Uri(uri), timeout.Token).GetAwaiter().GetResult(); }
            finally { timeout.Dispose(); }
        }

        public void SendText(string value, int timeoutMilliseconds)
        {
            byte[] bytes = new UTF8Encoding(false, true).GetBytes(value);
            ArraySegment<byte> segment = new ArraySegment<byte>(bytes);
            CancellationTokenSource timeout = new CancellationTokenSource(timeoutMilliseconds);
            try
            {
                lock (sendLock)
                    socket.SendAsync(segment, WebSocketMessageType.Text, true, timeout.Token).GetAwaiter().GetResult();
            }
            finally { timeout.Dispose(); }
        }

        public string ReceiveText(int timeoutMilliseconds)
        {
            MemoryStream data = new MemoryStream();
            byte[] buffer = new byte[16384];
            CancellationTokenSource timeout = new CancellationTokenSource(timeoutMilliseconds);
            try
            {
                while (true)
                {
                    ArraySegment<byte> segment = new ArraySegment<byte>(buffer);
                    WebSocketReceiveResult result = socket.ReceiveAsync(segment, timeout.Token).GetAwaiter().GetResult();
                    if (result.MessageType == WebSocketMessageType.Close)
                        throw new InvalidOperationException("DevTools WebSocket closed before a response.");
                    data.Write(buffer, 0, result.Count);
                    if (result.EndOfMessage) break;
                }
                return new UTF8Encoding(false, true).GetString(data.ToArray());
            }
            finally { timeout.Dispose(); data.Dispose(); }
        }

        public void Dispose()
        {
            try
            {
                if (socket.State == WebSocketState.Open)
                    socket.CloseAsync(WebSocketCloseStatus.NormalClosure, "UAT session complete", CancellationToken.None).GetAwaiter().GetResult();
            }
            catch { }
            socket.Dispose();
        }
    }
}
'@
}

function Get-UatAutomationElements {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][System.IntPtr]$WindowHandle,
        [Parameter(Mandatory = $true)][uint32]$ProcessId
    )

    if ([Sdir.Installer.Uat.NativeWindowApi]::OwnerProcessId($WindowHandle) -ne $ProcessId) {
        throw 'UI Automation refused a window owned by a different process.'
    }
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($WindowHandle)
    if ($null -eq $root) { throw 'The owned window is not available to UI Automation.' }
    return $root.FindAll([System.Windows.Automation.TreeScope]::Descendants,
        [System.Windows.Automation.Condition]::TrueCondition)
}

function Get-UatAutomationSummary {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][System.IntPtr]$WindowHandle,
        [Parameter(Mandatory = $true)][uint32]$ProcessId
    )

    $elements = Get-UatAutomationElements -WindowHandle $WindowHandle -ProcessId $ProcessId
    $items = New-Object System.Collections.Generic.List[object]
    foreach ($element in $elements) {
        try {
            $name = [string]$element.Current.Name
            $type = [string]$element.Current.ControlType.ProgrammaticName
            if (-not [string]::IsNullOrWhiteSpace($name)) {
                [void]$items.Add([pscustomobject]@{
                    name = $name
                    controlType = $type
                    enabled = [bool]$element.Current.IsEnabled
                    offscreen = [bool]$element.Current.IsOffscreen
                    nativeHandle = [int64]$element.Current.NativeWindowHandle
                })
            }
        }
        catch { }
    }
    return @($items.ToArray())
}

function Invoke-UatAutomationButton {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][System.IntPtr]$WindowHandle,
        [Parameter(Mandatory = $true)][uint32]$ProcessId,
        [Parameter(Mandatory = $true)][string]$NamePattern,
        [switch]$AsyncPostMessage,
        [IntPtr]$ExpectedNativeHandle = [IntPtr]::Zero
    )

    $elements = Get-UatAutomationElements -WindowHandle $WindowHandle -ProcessId $ProcessId
    $matches = New-Object System.Collections.Generic.List[object]
    foreach ($element in $elements) {
        try {
            if ($element.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and
                $element.Current.IsEnabled -and
                $element.Current.Name -match $NamePattern -and
                -not $element.Current.IsOffscreen) {
                [void]$matches.Add($element)
            }
        }
        catch { }
    }
    if ($matches.Count -ne 1) {
        throw ('Expected one enabled visible owned button matching /{0}/; found {1}.' -f $NamePattern, $matches.Count)
    }
    $button = $matches[0]
    if ($ExpectedNativeHandle -ne [IntPtr]::Zero -and
        ([int64]$button.Current.NativeWindowHandle -ne $ExpectedNativeHandle.ToInt64() -or
         [Sdir.Installer.Uat.NativeWindowApi]::OwnerProcessId($ExpectedNativeHandle) -ne $ProcessId -or
         -not [Sdir.Installer.Uat.NativeWindowApi]::IsDescendant($WindowHandle, $ExpectedNativeHandle))) {
        throw 'The owned UIA button changed its native target before input.'
    }
    if ($AsyncPostMessage) {
        $nativeHandle = [System.IntPtr]::new([int64]$button.Current.NativeWindowHandle)
        if ($nativeHandle -eq [System.IntPtr]::Zero -or
            [Sdir.Installer.Uat.NativeWindowApi]::OwnerProcessId($nativeHandle) -ne $ProcessId) {
            throw 'The Install button has no verified native child window for asynchronous input.'
        }
        if (-not [Sdir.Installer.Uat.NativeWindowApi]::PostButtonClick($nativeHandle, $ProcessId)) {
            throw 'PostMessage could not enqueue the owned Install button input.'
        }
        return [pscustomobject]@{ name = [string]$button.Current.Name; mechanism = 'PostMessage(WM_LBUTTONDOWN/UP)'; nativeHandle = $nativeHandle.ToInt64() }
    }
    $pattern = $null
    if (-not $button.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
        throw ('The owned button does not expose InvokePattern: {0}' -f $button.Current.Name)
    }
    $pattern.Invoke()
    return [pscustomobject]@{ name = [string]$button.Current.Name; mechanism = 'UIA.InvokePattern'; nativeHandle = [int64]$button.Current.NativeWindowHandle }
}

function Set-UatAutomationCheckBox {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][System.IntPtr]$WindowHandle,
        [Parameter(Mandatory = $true)][uint32]$ProcessId,
        [Parameter(Mandatory = $true)][string]$NamePattern,
        [Parameter(Mandatory = $true)][bool]$Checked
    )

    $elements = Get-UatAutomationElements -WindowHandle $WindowHandle -ProcessId $ProcessId
    $matches = New-Object System.Collections.Generic.List[object]
    foreach ($element in $elements) {
        try {
            if ($element.Current.ControlType -eq [System.Windows.Automation.ControlType]::CheckBox -and
                $element.Current.IsEnabled -and
                $element.Current.Name -match $NamePattern -and
                -not $element.Current.IsOffscreen) {
                [void]$matches.Add($element)
            }
        }
        catch { }
    }
    if ($matches.Count -ne 1) {
        throw ('Expected one enabled visible owned checkbox matching /{0}/; found {1}.' -f $NamePattern, $matches.Count)
    }
    $checkbox = $matches[0]
    $pattern = $null
    if (-not $checkbox.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) {
        throw ('The owned checkbox does not expose TogglePattern: {0}' -f $checkbox.Current.Name)
    }
    $current = $pattern.Current.ToggleState -eq [System.Windows.Automation.ToggleState]::On
    if ($current -ne $Checked) { $pattern.Toggle() }
    $after = $pattern.Current.ToggleState -eq [System.Windows.Automation.ToggleState]::On
    if ($after -ne $Checked) { throw ('Checkbox state did not reach requested value: {0}' -f $checkbox.Current.Name) }
    return [pscustomobject]@{ name = [string]$checkbox.Current.Name; checked = $after; mechanism = 'UIA.TogglePattern' }
}

function Send-UatOwnedKey {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][System.IntPtr]$WindowHandle,
        [Parameter(Mandatory = $true)][uint32]$ProcessId,
        [Parameter(Mandatory = $true)][ValidateRange(1, 255)][int]$VirtualKey
    )
    if (-not [Sdir.Installer.Uat.NativeWindowApi]::RestoreAndFocus($WindowHandle, $ProcessId)) {
        throw 'The owned window could not be restored and focused; no keyboard input was sent.'
    }
    if (-not [Sdir.Installer.Uat.NativeWindowApi]::SendVirtualKey([uint16]$VirtualKey)) {
        throw 'SendInput failed for the validated owned window.'
    }
    return [pscustomobject]@{ virtualKey = $VirtualKey; mechanism = 'SendInput'; windowHandle = $WindowHandle.ToInt64() }
}

function Capture-UatOwnedWindow {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][System.IntPtr]$WindowHandle,
        [Parameter(Mandatory = $true)][uint32]$ProcessId,
        [Parameter(Mandatory = $true)][string]$Path
    )
    if (-not [Sdir.Installer.Uat.NativeWindowApi]::CaptureWindowPng($WindowHandle, $ProcessId, $Path)) {
        throw ('Could not capture the verified owned window to {0}.' -f $Path)
    }
    return [System.IO.Path]::GetFullPath($Path)
}

function New-UatCdpSocket {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$WebSocketUrl,
        $OwnerWindow,
        [int]$ListenerProcessId = 0,
        [int]$RootProcessId = 0,
        [long]$RootStartTicks = 0
    )
    $socket = New-Object Sdir.Installer.Uat.CdpSocket
    $socket.Connect($WebSocketUrl, 10000)
    return [pscustomobject]@{ socket = $socket; nextId = 0; ownerWindow = $OwnerWindow; listenerProcessId = $ListenerProcessId; rootProcessId = $RootProcessId; rootStartTicks = $RootStartTicks }
}

function Assert-UatCdpSessionOwner {
    param([Parameter(Mandatory = $true)]$Session)
    if ($Session.ownerWindow) {
        Assert-UatOwnedWindow -Window $Session.ownerWindow
        if ($Session.listenerProcessId -and $Session.rootProcessId -and $Session.listenerProcessId -ne $Session.rootProcessId) {
            [void](Assert-UatDescendantProcess -RootProcessId $Session.rootProcessId -RootStartTicks $Session.rootStartTicks -ChildProcessId $Session.listenerProcessId -ExpectedImageName 'msedgewebview2.exe')
        }
    }
}

function Invoke-UatCdpCommand {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]$Session,
        [Parameter(Mandatory = $true)][string]$Method,
        [hashtable]$Parameters = @{}
    )

    Assert-UatCdpSessionOwner -Session $Session
    $Session.nextId = [int]$Session.nextId + 1
    $id = [int]$Session.nextId
    $message = [ordered]@{ id = $id; method = $Method; params = $Parameters } | ConvertTo-Json -Depth 12 -Compress
    $Session.socket.SendText($message, 10000)
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while ([DateTime]::UtcNow -lt $deadline) {
        $remaining = [int][Math]::Max(100, ($deadline - [DateTime]::UtcNow).TotalMilliseconds)
        $raw = $Session.socket.ReceiveText($remaining)
        $response = $raw | ConvertFrom-Json
        $replyId = $response.PSObject.Properties['id']
        if ($null -eq $replyId -or [int]$replyId.Value -ne $id) { continue }
        $errorProperty = $response.PSObject.Properties['error']
        if ($null -ne $errorProperty -and $null -ne $errorProperty.Value) {
            throw ('CDP {0} failed: {1}' -f $Method, ($errorProperty.Value | ConvertTo-Json -Compress))
        }
        $resultProperty = $response.PSObject.Properties['result']
        if ($null -eq $resultProperty) { throw ('CDP {0} reply omitted its result property.' -f $Method) }
        return $resultProperty.Value
    }
    throw ('Timed out waiting for CDP {0} response {1}.' -f $Method, $id)
}

function Get-UatCdpTarget {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][int]$Port,
        [Parameter(Mandatory = $true)][int]$ProcessId
    )
    if (-not (Get-Command Get-NetTCPConnection -ErrorAction SilentlyContinue)) {
        throw 'Get-NetTCPConnection is required to bind the DevTools port to the owned app PID.'
    }
    $listeners = @(Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction Stop |
        Where-Object { [int]$_.OwningProcess -eq $ProcessId })
    if ($listeners.Count -ne 1) {
        throw ('DevTools port {0} is not owned by the expected app PID {1}.' -f $Port, $ProcessId)
    }
    $response = Invoke-WebRequest -UseBasicParsing -Uri ('http://127.0.0.1:{0}/json/list' -f $Port) -TimeoutSec 10
    $targets = @($response.Content | ConvertFrom-Json)
    $pages = @($targets | Where-Object { $_.type -eq 'page' -and $_.webSocketDebuggerUrl })
    if ($pages.Count -ne 1) { throw ('Expected one owned WebView page target; found {0}.' -f $pages.Count) }
    return $pages[0]
}

function Invoke-UatCdpEvaluate {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]$Session,
        [Parameter(Mandatory = $true)][string]$Expression
    )
    Assert-UatCdpSessionOwner -Session $Session
    $result = Invoke-UatCdpCommand -Session $Session -Method 'Runtime.evaluate' -Parameters @{
        expression = $Expression
        returnByValue = $true
        awaitPromise = $true
        userGesture = $false
    }
    $exceptionProperty = $null
    if ($null -ne $result) { $exceptionProperty = $result.PSObject.Properties['exceptionDetails'] }
    if ($null -ne $exceptionProperty -and $null -ne $exceptionProperty.Value) {
        throw ('Read-only DOM evaluation failed: {0}' -f ($exceptionProperty.Value | ConvertTo-Json -Compress))
    }
    return $result.result.value
}

function Send-UatCdpClick {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]$Session,
        [Parameter(Mandatory = $true)][string]$Selector,
        [Parameter(Mandatory = $true)][string]$ExpectedChangeExpression,
        [int]$TimeoutSeconds = 15
    )
    $selectorJson = ConvertTo-Json -InputObject $Selector -Compress
    $geometryExpression = "(() => { const e=document.querySelector($selectorJson); if(!e) return null; e.scrollIntoView({block:'center',inline:'center'}); const r=e.getBoundingClientRect(); const x=r.left+r.width/2, y=r.top+r.height/2; const h=document.elementFromPoint(x,y); return {x,y,width:r.width,height:r.height,disabled:!!e.disabled,hit:!!h&&(h===e||e.contains(h)),tag:e.tagName}; })()"
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $geometry = $null
    while ([DateTime]::UtcNow -lt $deadline) {
        $geometry = Invoke-UatCdpEvaluate -Session $Session -Expression $geometryExpression
        if ($geometry -and -not $geometry.disabled -and $geometry.hit -and $geometry.width -gt 0 -and $geometry.height -gt 0) { break }
        Start-Sleep -Milliseconds 150
    }
    if (-not $geometry -or $geometry.disabled -or -not $geometry.hit) {
        throw ('CDP input target was not rendered, enabled, and hit-testable: {0}' -f $Selector)
    }
    foreach ($type in @('mouseMoved', 'mousePressed', 'mouseReleased')) {
        $parameters = @{ type = $type; x = [double]$geometry.x; y = [double]$geometry.y; button = 'left'; clickCount = 1 }
        if ($type -eq 'mousePressed') { $parameters.buttons = 1 }
        [void](Invoke-UatCdpCommand -Session $Session -Method 'Input.dispatchMouseEvent' -Parameters $parameters)
    }
    Wait-UatCdpCondition -Session $Session -Expression $ExpectedChangeExpression -TimeoutSeconds $TimeoutSeconds
    return [pscustomobject]@{ selector = $Selector; x = $geometry.x; y = $geometry.y; mechanism = 'CDP Input.dispatchMouseEvent'; hitTest = $true }
}

function Send-UatCdpKey {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]$Session,
        [Parameter(Mandatory = $true)][string]$Key,
        [Parameter(Mandatory = $true)][string]$ExpectedChangeExpression,
        [string]$Text,
        [int]$TimeoutSeconds = 15
    )
    $keyCode = switch ($Key) {
        'Enter' { 13 }
        'Escape' { 27 }
        'Tab' { 9 }
        'F2' { 113 }
        'F5' { 116 }
        default { throw ('Unsupported native key for CDP dispatch: {0}' -f $Key) }
    }
    [void](Invoke-UatCdpCommand -Session $Session -Method 'Input.dispatchKeyEvent' -Parameters @{
        type = 'keyDown'; key = $Key; code = $Key; windowsVirtualKeyCode = $keyCode; nativeVirtualKeyCode = $keyCode
    })
    if ($Text) {
        [void](Invoke-UatCdpCommand -Session $Session -Method 'Input.insertText' -Parameters @{ text = $Text })
    }
    [void](Invoke-UatCdpCommand -Session $Session -Method 'Input.dispatchKeyEvent' -Parameters @{
        type = 'keyUp'; key = $Key; code = $Key; windowsVirtualKeyCode = $keyCode; nativeVirtualKeyCode = $keyCode
    })
    Wait-UatCdpCondition -Session $Session -Expression $ExpectedChangeExpression -TimeoutSeconds $TimeoutSeconds
    return [pscustomobject]@{ key = $Key; textInserted = [bool]$Text; mechanism = 'CDP Input.dispatchKeyEvent/insertText' }
}

function Wait-UatCdpCondition {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]$Session,
        [Parameter(Mandatory = $true)][string]$Expression,
        [int]$TimeoutSeconds = 15
    )
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ([bool](Invoke-UatCdpEvaluate -Session $Session -Expression $Expression)) { return $true }
        Start-Sleep -Milliseconds 150
    }
    throw ('Timed out waiting for an actual DOM change: {0}' -f $Expression)
}
