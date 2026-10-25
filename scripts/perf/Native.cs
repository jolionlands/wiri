// Read-only, bounded observers for lane-e-perf.ps1. Windows x64 / Windows PowerShell 5.1.
// No hooks, injected DLLs, ETW sessions, timer-resolution changes or busy-waits.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using Microsoft.Win32.SafeHandles;

namespace LaneE {
    public sealed class Resources {
        public long PrivateBytes, WorkingSetBytes;
        public int Threads, Handles;
        public uint GdiObjects, UserObjects;
    }
    public sealed class ThreadCount {
        public long Id, Created;
        public uint Switches;
        public double CpuMs;
    }
    public sealed class ThreadSample {
        public long Stamp, Created;
        public double CpuMs;
        public List<ThreadCount> Threads = new List<ThreadCount>();
    }
    public sealed class ThreadDelta {
        public long Id;
        public long Switches;
        public double CpuMs;
    }
    public sealed class IdleResult {
        public double Seconds, CpuMs, OneCorePercent, SwitchesPerSecond;
        public int NewThreads, ExitedThreads;
        public uint InputTickBefore, InputTickAfter;
        public List<ThreadDelta> Threads = new List<ThreadDelta>();
    }
    public sealed class ColdResult {
        public Process Process;
        public double FirstIpcMs = -1, FirstLayoutMs = -1, StartCallMs, IpcObserverMs, LayoutObserverMs;
        public string IpcReply = "", IpcError = "", LayoutError = "";
        public DateTime StartedUtc;
    }
    public sealed class KeyResult {
        public double MoveMs = -1, MaxObserverGapMs;
        public uint InputsSent, ReleaseInputsSent;
        public string InvalidReason = "";
        public int Polls;
        public long ForegroundBefore, ForegroundAfter;
        public string Before, After;
    }
    public static class Native {
        [DllImport("user32.dll")] static extern uint GetGuiResources(IntPtr h, uint flags);
        [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out Rect r);
        [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int attr, out Rect r, int size);
        [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
        [DllImport("user32.dll")] static extern short GetAsyncKeyState(int key);
        [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
        [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
        [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetNamedPipeServerProcessId(SafePipeHandle pipe, out uint pid);
        [DllImport("user32.dll")] static extern bool GetLastInputInfo(ref LastInput li);
        [DllImport("user32.dll", SetLastError = true)] static extern uint SendInput(uint n, Input[] inputs, int size);
        [DllImport("ntdll.dll")] static extern int NtQuerySystemInformation(int cls, IntPtr data, int size, out int needed);
        [StructLayout(LayoutKind.Sequential)] struct LastInput { public uint Size, Tick; }
        [StructLayout(LayoutKind.Sequential)] struct Rect {
            public int L, T, R, B;
            public override string ToString() { return L + "," + T + "," + R + "," + B; }
        }
        [StructLayout(LayoutKind.Sequential)] struct Input { public uint Type; public InputUnion Data; }
        [StructLayout(LayoutKind.Explicit)] struct InputUnion {
            [FieldOffset(0)] public Keyboard Keyboard;
            [FieldOffset(0)] public Mouse Mouse;
        }
        [StructLayout(LayoutKind.Sequential)] struct Keyboard {
            public ushort Key, Scan; public uint Flags, Time; public UIntPtr Extra;
        }
        [StructLayout(LayoutKind.Sequential)] struct Mouse {
            public int X, Y; public uint Data, Flags, Time; public UIntPtr Extra;
        }
        // SystemProcessInformation (class 5) native x64 layout from winternl / phnt.
        // Full header is necessary: SYSTEM_THREAD_INFORMATION[] follows it inline.
        [StructLayout(LayoutKind.Sequential)] struct ProcessInfo {
            public uint Next, ThreadCount;
            public long WorkingSetPrivateSize;
            public uint HardFaults, ThreadHighWatermark;
            public ulong CycleTime;
            public long Created, UserTime, KernelTime;
            public ushort ImageLength, ImageCapacity;
            public IntPtr ImageBuffer;
            public int BasePriority;
            public IntPtr Pid, ParentPid;
            public uint Handles, SessionId;
            public UIntPtr ProcessKey, PeakVirtualSize, VirtualSize;
            public uint PageFaults;
            public UIntPtr PeakWorkingSet, WorkingSet, PeakPagedPool, PagedPool,
                PeakNonPagedPool, NonPagedPool, Pagefile, PeakPagefile, PrivatePages;
            public long ReadOps, WriteOps, OtherOps, ReadBytes, WriteBytes, OtherBytes;
        }
        [StructLayout(LayoutKind.Sequential)] struct ThreadInfo {
            public long KernelTime, UserTime, Created;
            public uint WaitTime;
            public IntPtr StartAddress, Pid, Tid;
            public int Priority, BasePriority;
            public uint Switches, State, WaitReason;
        }
        static double Ms(long ticks) { return ticks * 1000.0 / Stopwatch.Frequency; }
        static Rect Frame(IntPtr h) {
            Rect r;
            if (!GetWindowRect(h, out r)) throw new InvalidOperationException("GetWindowRect failed");
            return r;
        }
        static Rect Visible(IntPtr h) {
            Rect r;
            return DwmGetWindowAttribute(h, 9, out r, 16) == 0 ? r : Frame(h);
        }
        public static Resources ReadResources(int pid) {
            using (Process p = Process.GetProcessById(pid)) {
                p.Refresh();
                return new Resources { PrivateBytes = p.PrivateMemorySize64, WorkingSetBytes = p.WorkingSet64,
                    Threads = p.Threads.Count, Handles = p.HandleCount,
                    GdiObjects = GetGuiResources(p.Handle, 0), UserObjects = GetGuiResources(p.Handle, 1) };
            }
        }
        public static ThreadSample ReadThreads(int pid) {
            if (IntPtr.Size != 8 || Marshal.SizeOf(typeof(ProcessInfo)) != 256 || Marshal.SizeOf(typeof(ThreadInfo)) != 80 ||
                Marshal.OffsetOf(typeof(ProcessInfo), "Pid").ToInt32() != 80 || Marshal.OffsetOf(typeof(ThreadInfo), "Switches").ToInt32() != 64)
                throw new PlatformNotSupportedException("Thread counter observer requires native x64 layouts (256/80; PID@80, switches@64)");
            int size = 1024 * 1024;
            for (;;) {
                IntPtr data = Marshal.AllocHGlobal(size);
                try {
                    int needed;
                    int status = NtQuerySystemInformation(5, data, size, out needed);
                    if (status == unchecked((int)0xc0000004)) {
                        size = Math.Max(size * 2, needed + 65536);
                        if (size > 64 * 1024 * 1024) throw new IOException("System process snapshot exceeds 64 MiB");
                        continue;
                    }
                    if (status < 0) throw new IOException("NtQuerySystemInformation: 0x" + status.ToString("x8"));
                    int offset = 0;
                    for (;;) {
                        if (offset < 0 || offset > size - 256) throw new IOException("Invalid process snapshot header");
                        IntPtr entry = IntPtr.Add(data, offset);
                        ProcessInfo p = (ProcessInfo)Marshal.PtrToStructure(entry, typeof(ProcessInfo));
                        if (p.Pid.ToInt64() == pid) {
                            if (p.ThreadCount > (size - offset - 256) / 80) throw new IOException("Invalid thread snapshot length");
                            var sample = new ThreadSample { Stamp = Stopwatch.GetTimestamp(), Created = p.Created, CpuMs = (p.KernelTime + p.UserTime) / 10000.0 };
                            for (int i = 0; i < p.ThreadCount; i++) {
                                ThreadInfo t = (ThreadInfo)Marshal.PtrToStructure(IntPtr.Add(entry, 256 + 80 * i), typeof(ThreadInfo));
                                if (t.Pid.ToInt64() != pid) throw new IOException("Thread PID mismatch (native layout changed)");
                                sample.Threads.Add(new ThreadCount { Id = t.Tid.ToInt64(), Created = t.Created,
                                    Switches = t.Switches, CpuMs = (t.KernelTime + t.UserTime) / 10000.0 });
                            }
                            return sample;
                        }
                        if (p.Next == 0) break;
                        offset = checked(offset + (int)p.Next);
                    }
                    throw new InvalidOperationException("Target process is absent from system snapshot");
                } finally { Marshal.FreeHGlobal(data); }
            }
        }
        static uint InputTick() {
            var li = new LastInput { Size = (uint)Marshal.SizeOf(typeof(LastInput)) };
            if (!GetLastInputInfo(ref li)) throw new IOException("GetLastInputInfo failed");
            return li.Tick;
        }
        public static IdleResult Idle(int pid, int milliseconds) {
            uint inputBefore = InputTick();
            ThreadSample a = ReadThreads(pid);
            Thread.Sleep(milliseconds); // Deliberately NO IPC, screenshots, input, or periodic sampling here.
            ThreadSample b = ReadThreads(pid);
            if (a.Created != b.Created) throw new InvalidOperationException("Process creation identity changed during idle observation");
            var result = new IdleResult { Seconds = Ms(b.Stamp - a.Stamp) / 1000,
                CpuMs = b.CpuMs - a.CpuMs, InputTickBefore = inputBefore, InputTickAfter = InputTick() };
            var old = new Dictionary<long, ThreadCount>();
            foreach (ThreadCount t in a.Threads) old.Add(t.Id, t);
            long switches = 0;
            foreach (ThreadCount t in b.Threads) {
                ThreadCount prior;
                bool same = old.TryGetValue(t.Id, out prior) && prior.Created == t.Created;
                long delta = same ? unchecked((uint)(t.Switches - prior.Switches)) : t.Switches;
                result.Threads.Add(new ThreadDelta { Id = t.Id, Switches = delta, CpuMs = t.CpuMs - (same ? prior.CpuMs : 0) });
                switches += delta;
                if (same) old.Remove(t.Id); else result.NewThreads++;
            }
            result.ExitedThreads = old.Count; // Exited threads' final counters cannot be recovered: flag the lower bound.
            result.SwitchesPerSecond = switches / result.Seconds;
            result.OneCorePercent = result.CpuMs / (result.Seconds * 10);
            return result;
        }
        static int Remaining(long deadline) {
            double remaining = Ms(deadline - Stopwatch.GetTimestamp());
            if (remaining <= 0) throw new TimeoutException("IPC total deadline expired");
            return Math.Max(1, (int)Math.Ceiling(remaining));
        }
        public static string Request(string json, int timeoutMs, int expectedPid) {
            long deadline = Stopwatch.GetTimestamp() + (long)(timeoutMs * Stopwatch.Frequency / 1000.0);
            using (var pipe = new NamedPipeClientStream(".", "wiri_control", PipeDirection.InOut, PipeOptions.Asynchronous)) {
                pipe.Connect(Remaining(deadline));
                uint serverPid;
                if (!GetNamedPipeServerProcessId(pipe.SafePipeHandle, out serverPid) || serverPid != expectedPid)
                    throw new IOException("IPC server is not the owned child; no request was written");
                byte[] bytes = Encoding.UTF8.GetBytes(json);
                var write = pipe.WriteAsync(bytes, 0, bytes.Length);
                if (!write.Wait(Remaining(deadline))) throw new TimeoutException("IPC write timeout");
                using (var text = new MemoryStream()) {
                    byte[] buffer = new byte[65536];
                    for (;;) {
                        var read = pipe.ReadAsync(buffer, 0, buffer.Length);
                        if (!read.Wait(Remaining(deadline))) throw new TimeoutException("IPC read timeout");
                        if (read.Result == 0) break; // Server closes a non-subscription connection after one reply.
                        text.Write(buffer, 0, read.Result);
                        if (text.Length > 4 * 1024 * 1024) throw new IOException("IPC reply exceeds 4 MiB");
                    }
                    return Encoding.UTF8.GetString(text.ToArray());
                }
            }
        }
        public static ColdResult Start(ProcessStartInfo info, IntPtr[] windows, int minHeight, int timeoutMs) {
            Rect[] before = Array.ConvertAll(windows, Frame);
            var result = new ColdResult();
            var ready = new ManualResetEvent(false);
            long origin = 0;
            var ipc = new Thread(delegate() {
                ready.WaitOne();
                try {
                    result.IpcObserverMs = Ms(Stopwatch.GetTimestamp() - origin);
                    result.IpcReply = Request("{\"type\":\"get_state\"}", timeoutMs, result.Process.Id);
                    long answered = Stopwatch.GetTimestamp();
                    if (!result.IpcReply.Contains("\"success\":true")) throw new IOException("First IPC reply is not successful");
                    result.FirstIpcMs = Ms(answered - origin);
                } catch (Exception ex) { result.IpcError = ex.Message; }
            });
            var layout = new Thread(delegate() {
                ready.WaitOne();
                try {
                    result.LayoutObserverMs = Ms(Stopwatch.GetTimestamp() - origin);
                    long until = origin + (long)(timeoutMs * Stopwatch.Frequency / 1000.0);
                    do {
                        var frames = new List<Rect>();
                        bool tiled = windows.Length > 0;
                        for (int i = 0; i < windows.Length; i++) {
                            Rect r = Visible(windows[i]);
                            tiled &= IsWindowVisible(windows[i]) && !IsIconic(windows[i]) && r.B - r.T >= minHeight &&
                                Frame(windows[i]).ToString() != before[i].ToString();
                            foreach (Rect prev in frames) tiled &= (r.L >= prev.R - 1 || r.R <= prev.L + 1) &&
                                Math.Abs(r.T - prev.T) <= 2 && Math.Abs(r.B - prev.B) <= 2;
                            frames.Add(r);
                        }
                        if (tiled) { result.FirstLayoutMs = Ms(Stopwatch.GetTimestamp() - origin); return; }
                        Thread.Sleep(1);
                    } while (Stopwatch.GetTimestamp() < until);
                    result.LayoutError = "Existing sentinel windows did not reach non-overlapping tiled heights within timeout";
                } catch (Exception ex) { result.LayoutError = ex.Message; }
            });
            ipc.IsBackground = true; layout.IsBackground = true;
            // Map the OS process-creation FILETIME onto QPC, not the slower PowerShell Start-Process wrapper.
            DateTime callUtc = DateTime.UtcNow;
            long callStamp = Stopwatch.GetTimestamp();
            result.Process = Process.Start(info);
            result.StartCallMs = Ms(Stopwatch.GetTimestamp() - callStamp);
            try {
                result.Process.ErrorDataReceived += delegate(object s, DataReceivedEventArgs e) { }; // drain, never block the child
                result.Process.BeginErrorReadLine();
                result.Process.OutputDataReceived += delegate(object s, DataReceivedEventArgs e) { };
                result.Process.BeginOutputReadLine();
                result.StartedUtc = result.Process.StartTime.ToUniversalTime();
                origin = callStamp + (long)((result.StartedUtc - callUtc).TotalSeconds * Stopwatch.Frequency);
                ipc.Start(); layout.Start(); ready.Set();
                if (!ipc.Join(timeoutMs + 5000) || !layout.Join(timeoutMs + 5000))
                    throw new TimeoutException("Cold observer did not finish within its bounded deadline");
                ready.Dispose();
                return result;
            } catch {
                // Ownership has not reached PowerShell yet. Never orphan a child if observer setup fails.
                ready.Set();
                try {
                    if (!result.Process.HasExited) {
                        try { Request("{\"type\":\"quit\"}", 1000, result.Process.Id); } catch { }
                        if (!result.Process.WaitForExit(2000)) { result.Process.Kill(); result.Process.WaitForExit(2000); }
                    }
                } finally { result.Process.Dispose(); }
                if (ipc.IsAlive) ipc.Join(timeoutMs + 1000);
                if (layout.IsAlive) layout.Join(timeoutMs + 1000);
                ready.Dispose();
                throw;
            }
        }
        static Input Key(ushort key, bool up) {
            var input = new Input { Type = 1 };
            input.Data.Keyboard = new Keyboard { Key = key, Flags = up ? 2u : 0u };
            return input;
        }
        public static KeyResult MaximizeAndObserve(IntPtr window, int timeoutMs) {
            Rect before = Frame(window);
            var result = new KeyResult { Before = before.ToString(), ForegroundBefore = GetForegroundWindow().ToInt64() };
            if (result.ForegroundBefore != window.ToInt64()) {
                result.ForegroundAfter = result.ForegroundBefore; result.InvalidReason = "Target lost foreground before injection"; return result;
            }
            foreach (int key in new int[] { 0x10, 0x11, 0x12, 0x46, 0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5 }) {
                if ((GetAsyncKeyState(key) & 0x8000) != 0) {
                    result.ForegroundAfter = result.ForegroundBefore; result.InvalidReason = "A relevant key is already held; no input injected"; return result;
                }
            }
            var inputs = new Input[] { Key(0x11, false), Key(0x12, false), Key(0x10, false), Key(0x46, false),
                Key(0x46, true), Key(0x10, true), Key(0x12, true), Key(0x11, true) };
            long start = Stopwatch.GetTimestamp(), last = start;
            result.InputsSent = SendInput((uint)inputs.Length, inputs, Marshal.SizeOf(typeof(Input)));
            // Keys were up at the preflight check; release our batch after a partial injection.
            if (result.InputsSent != inputs.Length) {
                result.InvalidReason = "SendInput accepted only part of the batch";
                Input[] release = { Key(0x46, true), Key(0x10, true), Key(0x12, true), Key(0x11, true) };
                result.ReleaseInputsSent = SendInput((uint)release.Length, release, Marshal.SizeOf(typeof(Input)));
            } else {
                do {
                    Rect now = Frame(window);
                    long stamp = Stopwatch.GetTimestamp();
                    result.Polls++;
                    result.MaxObserverGapMs = Math.Max(result.MaxObserverGapMs, Ms(stamp - last));
                    last = stamp;
                    result.After = now.ToString();
                    if (result.After != result.Before) { result.MoveMs = Ms(stamp - start); break; }
                    Thread.Sleep(1);
                } while (Ms(Stopwatch.GetTimestamp() - start) < timeoutMs);
            }
            result.ForegroundAfter = GetForegroundWindow().ToInt64();
            return result;
        }
        public static double Median(double[] values) {
            if (values.Length == 0) return -1;
            var sorted = (double[])values.Clone(); Array.Sort(sorted);
            int n = sorted.Length;
            return n % 2 != 0 ? sorted[n / 2] : (sorted[n / 2 - 1] + sorted[n / 2]) / 2;
        }
    }
}
