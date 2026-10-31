# Passive fixture loader. Open/Close operations require the caller's desktop mutex.
# Every opening attempt stays registered through its UI thread's final disposal,
# including attempts that time out without returning an HWND. Prefix ownership is
# based on the original title, so renaming a form cannot detach cleanup ownership.
if (-not ('OwnedLiveForms' -as [type])) {
    Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Threading;
using System.Windows.Forms;
public static class OwnedLiveForms {
    sealed class Opening {
        public readonly object Sync = new object();
        public string Title;
        public Form Form;
        public Thread Thread;
        public IntPtr Handle;
        public bool Cancelled, Ready, Finished;
        public Exception Error;
    }
    static readonly List<Opening> Openings = new List<Opening>();
    static readonly Dictionary<IntPtr, Opening> Forms = new Dictionary<IntPtr, Opening>();
    [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    static Opening[] Matching(string prefix) {
        var owned = new List<Opening>();
        lock (Openings) {
            foreach (var opening in Openings)
                if (prefix == null || opening.Title.StartsWith(prefix, StringComparison.Ordinal)) owned.Add(opening);
        }
        return owned.ToArray();
    }
    static void Cancel(Opening opening) {
        Form form;
        lock (opening.Sync) { opening.Cancelled = true; form = opening.Form; }
        if (form != null) {
            try {
                if (!form.IsDisposed && form.IsHandleCreated)
                    form.BeginInvoke((MethodInvoker)delegate { form.Close(); });
            } catch (InvalidOperationException) { }
        }
    }
    public static IntPtr Open(string title, int red, int green, int blue) {
        if (title == null) throw new ArgumentNullException("title");
        var opening = new Opening(); opening.Title = title;
        // Own the attempt before the thread can construct or show anything.
        lock (Openings) Openings.Add(opening);
        var thread = new Thread(delegate() {
            Form form = null;
            try {
                form = new Form();
                lock (opening.Sync) {
                    opening.Form = form;
                    if (opening.Cancelled) return;
                }
                form.Text = title; form.Size = new Size(520, 380);
                form.BackColor = Color.FromArgb(red, green, blue);
                form.StartPosition = FormStartPosition.Manual;
                form.Location = new Point(120, 120);
                form.Shown += delegate {
                    bool cancelled;
                    lock (opening.Sync) {
                        cancelled = opening.Cancelled;
                        if (!cancelled) {
                            opening.Handle = form.Handle;
                            lock (Forms) Forms.Add(opening.Handle, opening);
                            opening.Ready = true;
                            Monitor.PulseAll(opening.Sync);
                        }
                    }
                    if (cancelled) form.Close();
                };
                lock (opening.Sync) { if (opening.Cancelled) return; }
                Application.Run(form);
            } catch (Exception error) {
                lock (opening.Sync) opening.Error = error;
            } finally {
                try { if (form != null) form.Dispose(); }
                catch (Exception error) { lock (opening.Sync) opening.Error = error; }
                finally {
                    lock (Forms) {
                        Opening registered;
                        if (Forms.TryGetValue(opening.Handle, out registered) && registered == opening)
                            Forms.Remove(opening.Handle);
                    }
                    lock (opening.Sync) { opening.Finished = true; Monitor.PulseAll(opening.Sync); }
                }
            }
        });
        lock (opening.Sync) opening.Thread = thread;
        try {
            thread.SetApartmentState(ApartmentState.STA); thread.IsBackground = true;
            thread.Start();
        } catch (Exception error) {
            lock (opening.Sync) { opening.Error = error; opening.Finished = true; }
            throw;
        }
        var clock = System.Diagnostics.Stopwatch.StartNew();
        Exception failure;
        lock (opening.Sync) {
            while (!opening.Ready && !opening.Finished && clock.ElapsedMilliseconds < 15000)
                Monitor.Wait(opening.Sync, (int)Math.Max(1, 15000-clock.ElapsedMilliseconds));
            if (opening.Ready) return opening.Handle;
            opening.Cancelled = true;
            failure = opening.Error;
        }
        Cancel(opening);
        if (failure != null) throw new InvalidOperationException("Owned fixture failed to open", failure);
        throw new TimeoutException("Owned fixture timed out; its registered UI thread owns cancellation and disposal");
    }
    public static void SetTitle(IntPtr h, string title) {
        Opening opening;
        lock (Forms) {
            if (!Forms.TryGetValue(h, out opening)) throw new InvalidOperationException("Unknown owned fixture HWND");
        }
        var form = opening.Form;
        form.BeginInvoke((MethodInvoker)delegate { form.Text = title; });
    }
    public static bool Close(IntPtr h) {
        Opening opening;
        lock (Forms) { if (!Forms.TryGetValue(h, out opening)) return false; }
        Cancel(opening);
        return true;
    }
    public static void ClosePrefix(string prefix) {
        foreach (var opening in Matching(prefix)) Cancel(opening);
    }
    public static int PendingForPrefix(string prefix) {
        int pending = 0;
        foreach (var opening in Matching(prefix)) {
            lock (opening.Sync) {
                if (!opening.Finished || (opening.Thread != null && opening.Thread.IsAlive)) pending++;
            }
        }
        return pending;
    }
    public static bool AllClosedForPrefix(string prefix) { return PendingForPrefix(prefix) == 0; }
    public static bool AllClosed { get { return AllClosedForPrefix(null); } }
    public static uint Dpi(IntPtr h) {
        uint pid; GetWindowThreadProcessId(h, out pid);
        if (pid != (uint)System.Diagnostics.Process.GetCurrentProcess().Id)
            throw new InvalidOperationException("Refused foreign fixture HWND");
        return GetDpiForWindow(h);
    }
}
'@ -ErrorAction Stop
}
