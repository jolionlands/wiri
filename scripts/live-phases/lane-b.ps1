# Lane B / H22-H25. Dot-sourced after live-test.ps1 constructs $summary.
# Uses the harness's D, cfg, summary, steps, Ctl, Start-Wiri, FocusForm,
# Shot, Rc, Near, WinList and WinOf. Earlier phases have already closed their
# windows and wiri: do not reuse $w, $forms or $hung here.
# Assumed config API: on-release=true (compatibility extension),
# allow-inhibiting=false, toggle-keyboard-shortcuts-inhibit, Mod+Mouse*,
# WheelScroll*, focus-follows-mouse max-scroll-amount="0%", focus-delay-ms,
# warp-mouse-to-focus mode="center-xy-always". Unsupported APIs FAIL, not skip.
# A nested WinForms EDIT child exercises HWND-to-root promotion, not Chrome /
# Electron's native rendering/input stack. No browser is launched or claimed.
& {
    $bDir = Join-Path $D 'lane-b'
    $null = New-Item -ItemType Directory -Path $bDir -Force
    $bDetails = [ordered]@{}
    $bOwned = New-Object 'System.Collections.Generic.List[System.Diagnostics.Process]'
    $bGates = @(
        'h22_win_bind_fires', 'h22_win_bind_does_not_open_start',
        'h22_bound_key_down_and_up_are_consumed', 'h22_unbound_key_down_and_up_pass',
        'h22_left_shift_down_and_up_pass', 'h22_right_shift_down_and_up_pass',
        'h22_left_ctrl_down_and_up_pass', 'h22_right_ctrl_down_and_up_pass',
        'h22_left_alt_down_and_up_pass', 'h22_right_alt_down_and_up_pass',
        'h22_left_win_down_and_up_pass', 'h22_right_win_down_and_up_pass',
        'h22_repeat_false_fires_once', 'h22_repeat_true_fires_for_each_down',
        'h22_cooldown_suppresses_fast_presses', 'h22_cooldown_expires',
        'h22_release_fires_only_on_key_up', 'h22_release_after_modifier_up_does_not_leak',
        'h22_inhibited_bind_passes_to_app', 'h22_uninhibitable_bind_still_fires',
        'h22_leaving_inhibition_restores_bind',
        'h23_unbound_buttons_down_and_up_pass', 'h23_unbound_wheel_both_axes_pass',
        'h23_left_mouse_bind_fires_and_consumes', 'h23_right_mouse_bind_fires_and_consumes',
        'h23_middle_mouse_bind_fires_and_consumes',
        'h23_wheel_up_bind_fires_and_consumes', 'h23_wheel_down_bind_fires_and_consumes',
        'h23_wheel_left_bind_fires_and_consumes', 'h23_wheel_right_bind_fires_and_consumes',
        'h23_high_resolution_wheel_accumulates', 'h23_unbound_axis_is_not_consumed',
        'h24_drag_starts_over_nested_client_child', 'h24_mod_left_drag_moves_root',
        'h24_mod_right_drag_resizes_root', 'h24_floating_resize_survives_refresh',
        'h24_tiled_resize_changes_column_width',
        'h24_tiled_resize_survives_refresh', 'h24_drag_button_transitions_are_consumed',
        'h25_hover_targets_nested_client_child', 'h25_focus_does_not_change_before_delay',
        'h25_focus_changes_after_delay', 'h25_leaving_target_cancels_pending_focus',
        'h25_zero_delay_hover_focuses_without_reentry',
        'h25_disabled_warp_keeps_cursor', 'h25_warp_centres_first_focused_window',
        'h25_warp_tracks_next_focused_window'
    )
    foreach ($g in $bGates) { $summary[$g] = $false }
    function BGate([string]$name, [bool]$pass, $detail) {
        $summary[$name] = $pass
        $bDetails[$name] = [ordered]@{ pass = $pass; observed = $detail }
    }
    function BError([string]$phase, $err) { $bDetails["$phase-error"] = [string]$err }
    function BStart([string]$text, [string]$phase) {
        # Refuse an existing instance, including one we failed to stop. Never
        # borrow the parent's global shutdown helper / clean-slate behaviour.
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw "$phase refuses to start while another wiri instance is running"
        }
        [IO.File]::WriteAllText($cfg, $text, (New-Object Text.UTF8Encoding($false)))
        $err = Join-Path $bDir "$phase.stderr.log"
        $p = Start-Wiri $err ($err + '.out')
        $bOwned.Add($p) # Retain immediately, even if the settling wait is interrupted.
        $null = $p.Handle
        Start-Sleep -Seconds 4
        if ($p.HasExited) { throw "$phase wiri exited before its checks (PID $($p.Id))" }
        return $p
    }
    function BStop($p, [string]$phase) {
        if ($null -eq $p -or -not $bOwned.Contains($p) -or $p.HasExited) { return }
        $stop = [ordered]@{ pid = $p.Id; quit = ''; forced = $false }
        try {
            # Compare the connected pipe's server PID, then write quit on THAT
            # handle. A global Ctl quit could instead stop an unrelated daemon.
            # The daemon serves a per-user, per-session pipe (see WiriPipeName), not a bare wiri_control.
            $stop.quit = [LaneBProbe]::QuitOwned($p.Id, (WiriPipeName))
        } catch { $stop.quit = [string]$_ }
        if (-not $p.WaitForExit(2500)) {
            $stop.forced = $true
            $p.Kill() # Retained owned process handle, never a name/PID search.
            if (-not $p.WaitForExit(5000)) { throw "Owned wiri PID $($p.Id) did not stop" }
        }
        $bDetails["$phase-stop-$($p.Id)"] = $stop
    }
    function BCheckedCtl([string]$name, [string[]]$ctlArgs) {
        $before = $steps.Count
        $output = Ctl $name $ctlArgs
        if ($steps.Count -ne ($before + 1) -or $steps[$before].step -ne $name -or [string]$steps[$before].rc -ne '0') {
            throw "Required IPC step failed: $name (see steps and per-step output)"
        }
        return $output
    }
    function BSpawn([string]$name) {
        $path = Join-Path $bDir "$name.marker.txt"
        [IO.File]::WriteAllText($path, '')
        return ('spawn-sh r#"echo hit>> "' + $path + '""#')
    }
    function BCount([string]$name) {
        return @(Get-Content (Join-Path $bDir "$name.marker.txt") -ErrorAction SilentlyContinue | Where-Object { $_ -eq 'hit' }).Count
    }
    function BObserveCount([string]$name) {
        # Negative evidence needs the full positive dispatch budget too. Shell
        # marker completion is asynchronous; a 160/450 ms sample can lie.
        Start-Sleep -Milliseconds 3400
        return (BCount $name)
    }
    function BWaitCount([string]$name, [int]$n) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        for ($i = 0; $i -lt 30 -and (BCount $name) -lt $n; $i++) { Start-Sleep -Milliseconds 100 }
        # Observe a full bounded interval even after success, to catch duplicates.
        $remaining = 3400 - $clock.ElapsedMilliseconds
        if ($remaining -gt 0) { Start-Sleep -Milliseconds $remaining }
        return (BCount $name)
    }
    function BRelease {
        # Each attempted release is independent; only successful injected downs
        # still held by this lane are eligible. Do this BEFORE closing probes.
        foreach ($err in [LaneBProbe]::ReleaseAll()) { BError 'input-cleanup' $err }
    }
    function BCloseProbes { try { BRelease } finally { [LaneBProbe]::CloseAll() } }
    function BMods([int[]]$mods, [bool]$up) {
        if ($up) {
            $failed = @()
            for ($i = $mods.Count - 1; $i -ge 0; $i--) {
                try { [LaneBProbe]::Key($mods[$i], $true) } catch { $failed += [string]$_ }
            }
            if ($failed.Count) { throw ($failed -join '; ') }
        } else { foreach ($m in $mods) { [LaneBProbe]::Key($m, $false) } }
    }
    function BChord([int[]]$mods, [int]$vk, [int]$downs = 1) {
        try {
            BMods $mods $false
            for ($i = 0; $i -lt $downs; $i++) { [LaneBProbe]::Key($vk, $false); Start-Sleep -Milliseconds 90 }
        } finally { try { [LaneBProbe]::Key($vk, $true) } finally { BMods $mods $true } }
        Start-Sleep -Milliseconds 180
    }
    function BFocus([IntPtr]$h) {
        FocusForm $h
        if ([LT]::GetForegroundWindow() -ne $h) { throw "Probe is not foreground: $h" }
        [LaneBProbe]::FocusChild($h)
        Start-Sleep -Milliseconds 100
    }
    function BEvents([IntPtr]$h, [string]$tag) {
        $events = @([LaneBProbe]::Events($h))
        $events | Set-Content (Join-Path $bDir "$tag.events.txt")
        return $events
    }
    function BExact($events, [string[]]$expected) {
        return ((@($events) -join ',') -ceq ($expected -join ','))
    }
    function BPointer([IntPtr]$h, [double]$x = 0.5, [double]$y = 0.5) {
        if (-not [LaneBProbe]::Owns($h) -or [LT]::GetForegroundWindow() -ne $h) { throw 'Pointer target is not our foreground probe' }
        $pt = [LaneBProbe]::ChildPoint($h, $x, $y)
        if (-not [LaneBProbe]::AtChild($h, $pt)) { throw 'Pointer target is obscured or not the nested EDIT' }
        [LaneBProbe]::Move($pt.X, $pt.Y)
        Start-Sleep -Milliseconds 100
        if ([LT]::GetForegroundWindow() -ne $h -or -not [LaneBProbe]::AtChild($h, [LaneBProbe]::Cursor())) { throw 'Pointer left the owned foreground EDIT' }
        return $pt
    }
    function BClick([int[]]$mods, [string]$button) {
        try { BMods $mods $false; [LaneBProbe]::Button($button, $false); Start-Sleep -Milliseconds 60 }
        finally { try { [LaneBProbe]::Button($button, $true) } finally { BMods $mods $true } }
        Start-Sleep -Milliseconds 250
    }
    function BWheel([int[]]$mods, [bool]$horizontal, [int]$delta) {
        try { BMods $mods $false; [LaneBProbe]::Wheel($horizontal, $delta) }
        finally { BMods $mods $true }
        Start-Sleep -Milliseconds 160
    }
    function BDrag([IntPtr]$h, [string]$button, [double]$x, [double]$y, [int]$dx, [int]$dy) {
        $pt = BPointer $h $x $y
        try {
            BMods @(0xA2, 0xA4) $false
            [LaneBProbe]::Button($button, $false)
            Start-Sleep -Milliseconds 100
            for ($i = 1; $i -le 10; $i++) {
                [LaneBProbe]::Move(($pt.X + [int]($dx * $i / 10)), ($pt.Y + [int]($dy * $i / 10)))
                Start-Sleep -Milliseconds 30
            }
        } finally { try { [LaneBProbe]::Button($button, $true) } finally { BMods @(0xA2, 0xA4) $true } }
        Start-Sleep -Milliseconds 1200
    }
    function BStartOpen {
        # Start's process name is locale-independent; also recognise older shell hosts.
        foreach ($h in [LT]::TopLevel()) {
            if (-not [LT]::IsWindowVisible($h) -or [LT]::Cloaked($h) -ne 0) { continue }
            try { $pn = (Get-Process -Id ([LT]::Pid($h)) -ErrorAction Stop).ProcessName } catch { continue }
            if ($pn -eq 'StartMenuExperienceHost' -or ([LT]::Title($h) -eq 'Start' -and [LT]::Class($h) -match 'CoreWindow|Xaml')) { return $true }
        }
        return $false
    }

    $bSavedCursor = $null
    try {
        # Application-queue capture, not a second low-level hook: a consumed event
        # must never appear here. Preserve side identity using scan/extended bits.
        Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing, System.Core -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.Drawing;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;
using Microsoft.Win32.SafeHandles;
public static class LaneBProbe {
  [StructLayout(LayoutKind.Sequential)] struct KI { public ushort vk, scan; public uint flags, time; public UIntPtr extra; }
  [StructLayout(LayoutKind.Sequential)] struct MI { public int x, y; public uint data, flags, time; public UIntPtr extra; }
  [StructLayout(LayoutKind.Explicit)] struct U { [FieldOffset(0)] public KI k; [FieldOffset(0)] public MI m; }
  [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public U u; }
  [DllImport("user32.dll", SetLastError=true)] static extern uint SendInput(uint n, INPUT[] input, int size);
  [DllImport("user32.dll")] static extern uint MapVirtualKey(uint vk, uint type);
  [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
  [DllImport("user32.dll")] static extern bool GetCursorPos(out Point p);
  [DllImport("user32.dll")] static extern bool IsChild(IntPtr parent, IntPtr child);
  [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point point);
  [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint flags);
  [DllImport("user32.dll")] static extern short GetAsyncKeyState(int vk);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetNamedPipeServerProcessId(SafePipeHandle pipe, out uint pid);
  static readonly object heldLock = new object();
  static readonly List<int> heldKeys = new List<int>();
  static readonly List<string> heldButtons = new List<string>();
  static void Send(INPUT i) { if (SendInput(1, new INPUT[] { i }, Marshal.SizeOf(typeof(INPUT))) != 1) throw new Win32Exception(Marshal.GetLastWin32Error()); }
  static void RequireForeground() {
    if (!Owns(GetForegroundWindow())) throw new InvalidOperationException("Input refused: foreground is not a Lane B probe");
  }
  static void RequirePointer() {
    IntPtr h = GetForegroundWindow();
    if (!Owns(h) || !AtChild(h, Cursor())) throw new InvalidOperationException("Input refused: pointer/foreground is not the owned nested EDIT");
  }
  public static void Key(int vk, bool up) {
    lock(heldLock) {
      bool held = heldKeys.Contains(vk);
      if (up && !held) return;
      if (!up) {
        RequireForeground();
        if (!held && GetAsyncKeyState(vk) < 0) throw new InvalidOperationException("Input refused: key already held: " + vk);
      }
      INPUT i = new INPUT(); i.type = 1; i.u.k.vk = (ushort)vk; i.u.k.scan = (ushort)MapVirtualKey((uint)vk, 0);
      i.u.k.flags = (up ? 2u : 0u) | ((vk == 0xA3 || vk == 0xA5 || vk == 0x5B || vk == 0x5C) ? 1u : 0u); Send(i);
      if (up) heldKeys.Remove(vk); else if (!held) heldKeys.Add(vk);
    }
  }
  public static void Button(string b, bool up) {
    int vk = b == "L" ? 1 : b == "R" ? 2 : b == "M" ? 4 : 0;
    if (vk == 0) throw new ArgumentException("Unknown button");
    lock(heldLock) {
      bool held = heldButtons.Contains(b);
      if (up && !held) return;
      if (!up) {
        RequirePointer();
        if (!held && GetAsyncKeyState(vk) < 0) throw new InvalidOperationException("Input refused: button already held: " + b);
      }
      INPUT i = new INPUT(); i.u.m.flags = b == "L" ? (up ? 4u : 2u) : b == "R" ? (up ? 16u : 8u) : (up ? 64u : 32u); Send(i);
      if (up) heldButtons.Remove(b); else if (!held) heldButtons.Add(b);
    }
  }
  public static string[] ReleaseAll() {
    var errors = new List<string>();
    lock(heldLock) {
      string[] buttons = heldButtons.ToArray(); int[] keys = heldKeys.ToArray();
      for (int i = buttons.Length-1; i >= 0; --i) { try { Button(buttons[i], true); } catch(Exception ex) { errors.Add("Button " + buttons[i] + ": " + ex.Message); } }
      for (int i = keys.Length-1; i >= 0; --i) { try { Key(keys[i], true); } catch(Exception ex) { errors.Add("Key " + keys[i] + ": " + ex.Message); } }
    }
    return errors.ToArray();
  }
  public static void Wheel(bool horizontal, int delta) {
    RequirePointer();
    INPUT i = new INPUT(); i.u.m.flags = horizontal ? 0x1000u : 0x800u; i.u.m.data = unchecked((uint)delta); Send(i);
  }
  public static void Move(int x, int y) {
    // Exercise real mouse input. SetCursorPos alone can leave the system in
    // keyboard/touch pointer-suppression mode and is not a hardware move.
    int left = GetSystemMetrics(76), top = GetSystemMetrics(77);
    int width = GetSystemMetrics(78), height = GetSystemMetrics(79);
    if (width <= 0 || height <= 0) throw new InvalidOperationException("Invalid virtual screen");
    x = Math.Max(left, Math.Min(left + width - 1, x)); y = Math.Max(top, Math.Min(top + height - 1, y));
    INPUT i = new INPUT(); i.u.m.flags = 0xC001u; // MOVE | VIRTUALDESK | ABSOLUTE
    i.u.m.x = (int)(((long)(x - left) * 65536 + 32768) / width);
    i.u.m.y = (int)(((long)(y - top) * 65536 + 32768) / height); Send(i);
  }
  public static Point Cursor() { Point p; if (!GetCursorPos(out p)) throw new InvalidOperationException("GetCursorPos"); return p; }
  public static string QuitOwned(int pid, string pipeName) {
    using (var pipe = new NamedPipeClientStream(".", pipeName, PipeDirection.InOut, PipeOptions.Asynchronous)) {
      pipe.Connect(800);
      uint server;
      if (!GetNamedPipeServerProcessId(pipe.SafePipeHandle, out server)) throw new Win32Exception(Marshal.GetLastWin32Error());
      if (server != (uint)pid) return "refused-unowned-server-" + server;
      byte[] data = Encoding.UTF8.GetBytes("{\"type\":\"quit\"}");
      IAsyncResult write = pipe.BeginWrite(data, 0, data.Length, null, null);
      using (var done = write.AsyncWaitHandle) {
        if (!done.WaitOne(1500)) return "owned-quit-write-timeout";
        pipe.EndWrite(write);
      }
      return "owned-quit-sent";
    }
  }
  sealed class Probe : IMessageFilter {
    public Form form; public TextBox edit; public Thread thread; public IntPtr hwnd;
    public volatile bool cancelled; public Exception error;
    public readonly ManualResetEvent ready = new ManualResetEvent(false);
    public readonly List<string> events = new List<string>();
    public bool PreFilterMessage(ref Message m) {
      if (m.HWnd != form.Handle && !IsChild(form.Handle, m.HWnd)) return false;
      string e = null; int msg = m.Msg;
      if (msg == 0x100 || msg == 0x101 || msg == 0x104 || msg == 0x105) {
        int vk = m.WParam.ToInt32(), lp = unchecked((int)m.LParam.ToInt64()); bool ext = (lp & 0x1000000) != 0;
        if (vk == 0x10) vk = ((lp >> 16) & 255) == 0x36 ? 0xA1 : 0xA0;
        if (vk == 0x11) vk = ext ? 0xA3 : 0xA2;
        if (vk == 0x12) vk = ext ? 0xA5 : 0xA4;
        e = ((msg == 0x101 || msg == 0x105) ? "KU:" : "KD:") + vk;
      } else if (msg >= 0x201 && msg <= 0x208) {
        if (msg == 0x201 || msg == 0x202) e = (msg == 0x201 ? "MD:" : "MU:") + "L";
        if (msg == 0x204 || msg == 0x205) e = (msg == 0x204 ? "MD:" : "MU:") + "R";
        if (msg == 0x207 || msg == 0x208) e = (msg == 0x207 ? "MD:" : "MU:") + "M";
      } else if (msg == 0x20A || msg == 0x20E) {
        int delta = unchecked((short)((m.WParam.ToInt64() >> 16) & 65535));
        e = (msg == 0x20A ? "W:V:" : "W:H:") + delta;
      }
      if (e != null) lock(events) { events.Add(e); }
      return false;
    }
  }
  static readonly object probeLock = new object();
  static readonly Dictionary<IntPtr, Probe> probes = new Dictionary<IntPtr, Probe>();
  // Includes pending, not-yet-Shown UI threads, so timeout cannot orphan a form.
  static readonly List<Probe> pending = new List<Probe>();
  static Probe Get(IntPtr h) { lock(probeLock) { return probes[h]; } }
  public static bool Owns(IntPtr h) { lock(probeLock) { return probes.ContainsKey(h); } }
  public static bool AtChild(IntPtr h, Point pt) {
    var p = Get(h); IntPtr hit = WindowFromPoint(pt);
    return hit != IntPtr.Zero && GetAncestor(hit, 2) == h && p.edit.IsHandleCreated &&
      (hit == p.edit.Handle || IsChild(p.edit.Handle, hit));
  }
  static void Cancel(Probe p) {
    p.cancelled = true;
    var f = p.form;
    try { if (f != null && f.IsHandleCreated && !f.IsDisposed) f.BeginInvoke((MethodInvoker)delegate { f.Close(); }); }
    catch (InvalidOperationException) { } // Its finally has disposed/already closed it.
  }
  public static IntPtr Open(string title) {
    var p = new Probe();
    p.thread = new Thread(delegate() {
      try {
        if (p.cancelled) return;
        var f = new Form(); p.form = f;
        f.Text = title; f.Size = new Size(560,400); f.StartPosition = FormStartPosition.CenterScreen;
        var panel = new Panel(); panel.Dock = DockStyle.Fill; panel.Padding = new Padding(24);
        p.edit = new TextBox(); p.edit.Multiline = true; p.edit.AcceptsReturn = true; p.edit.AcceptsTab = true;
        p.edit.Dock = DockStyle.Fill; p.edit.ScrollBars = ScrollBars.Both; p.edit.WordWrap = false;
        p.edit.ContextMenu = new ContextMenu(); p.edit.Text = "Lane B nested EDIT child\r\nInput delivered to this application is recorded.";
        panel.Controls.Add(p.edit); f.Controls.Add(panel);
        f.HandleCreated += delegate { if (p.cancelled) f.BeginInvoke((MethodInvoker)delegate { f.Close(); }); };
        f.Shown += delegate {
          p.hwnd = f.Handle;
          lock(probeLock) { if (!p.cancelled) probes[p.hwnd] = p; }
          if (p.cancelled) f.Close(); else p.edit.Focus();
          p.ready.Set();
        };
        if (p.cancelled) return;
        Application.AddMessageFilter(p);
        try { Application.Run(f); } finally { Application.RemoveMessageFilter(p); }
      } catch (Exception ex) { p.error = ex; }
      finally {
        try { if (p.form != null) p.form.Dispose(); }
        finally {
          lock(probeLock) { probes.Remove(p.hwnd); pending.Remove(p); }
          p.ready.Set();
        }
      }
    });
    p.thread.IsBackground = true; p.thread.SetApartmentState(ApartmentState.STA);
    lock(probeLock) { pending.Add(p); }
    try { p.thread.Start(); } catch { lock(probeLock) { pending.Remove(p); } throw; }
    if (!p.ready.WaitOne(5000)) {
      Cancel(p);
      bool joined = p.thread.Join(2500);
      throw new TimeoutException("Lane B probe did not open; thread joined=" + joined);
    }
    if (p.error != null || p.hwnd == IntPtr.Zero || !Owns(p.hwnd)) {
      Cancel(p); p.thread.Join(2500);
      throw new InvalidOperationException("Lane B probe failed/closed while opening", p.error);
    }
    return p.hwnd;
  }
  static void OnUI(Probe p, MethodInvoker action) {
    IAsyncResult call = p.form.BeginInvoke(action);
    using (var done = call.AsyncWaitHandle) {
      if (!done.WaitOne(2000)) throw new TimeoutException("Lane B probe UI did not respond");
      p.form.EndInvoke(call);
    }
  }
  public static string[] Events(IntPtr h) { var p = Get(h); lock(p.events) { return p.events.ToArray(); } }
  public static void Clear(IntPtr h) { var p = Get(h); lock(p.events) { p.events.Clear(); } }
  public static void FocusChild(IntPtr h) { var p = Get(h); OnUI(p, delegate { p.edit.Focus(); }); }
  public static void ActivateOwned(IntPtr h) { var p = Get(h); OnUI(p, delegate { p.form.Activate(); p.edit.Focus(); }); }
  public static Point ChildPoint(IntPtr h, double x, double y) {
    var p = Get(h); Point pt = new Point();
    OnUI(p, delegate { pt = p.edit.PointToScreen(new Point((int)(p.edit.ClientSize.Width*x), (int)(p.edit.ClientSize.Height*y))); });
    return pt;
  }
  public static void CloseAll() {
    Probe[] all; lock(probeLock) { all = pending.ToArray(); }
    foreach(var p in all) Cancel(p);
    var clock = Stopwatch.StartNew();
    foreach(var p in all) {
      int remaining = (int)Math.Max(0, 5000 - clock.ElapsedMilliseconds);
      if (!p.thread.Join(remaining)) throw new TimeoutException("Lane B probe thread did not close");
    }
  }
}
'@
        $bSavedCursor = [LaneBProbe]::Cursor()

        # H22: bounded synthetic physical transitions, including explicit repeated
        # downs. F24 keeps standalone Win/Alt tests from opening shell/menu UI.
        $b22 = $null
        try {
            $text = @"
input {
    mod-key "win"
}
binds {
    Mod+F6 repeat=false { $(BSpawn 'h22-win'); }
    Ctrl+Alt+F6 repeat=false { $(BSpawn 'h22-once'); }
    Ctrl+Alt+F7 repeat=true { $(BSpawn 'h22-repeat'); }
    Ctrl+Alt+F8 cooldown-ms=1800 { $(BSpawn 'h22-cool'); }
    Ctrl+Alt+F9 on-release=true { $(BSpawn 'h22-release'); }
    Ctrl+Alt+F10 allow-inhibiting=false { toggle-keyboard-shortcuts-inhibit; }
    Ctrl+Alt+F11 { $(BSpawn 'h22-inhibit'); }
    Ctrl+Alt+F12 allow-inhibiting=false { $(BSpawn 'h22-bypass'); }
}
"@
            # The Windows shell on some systems consumes Win+F24 down. First
            # capture its real no-wiri behavior, then demand an identical whole
            # sequence AND a paired modifier down/up with the hook enabled.
            $winBaseline = @{}
            $base = [LaneBProbe]::Open('wiri-lane-b-h22-baseline')
            foreach ($vk in @(0x5B,0x5C)) {
                [LaneBProbe]::ActivateOwned($base); Start-Sleep -Milliseconds 150
                if ([LT]::GetForegroundWindow() -ne $base) { throw 'No-wiri baseline probe activation failed' }
                [LaneBProbe]::Clear($base)
                BChord @($vk) 0x87
                $winBaseline[$vk] = @(BEvents $base ("h22-baseline-$vk"))
            }
            BCloseProbes
            $b22 = BStart $text 'h22'
            $h = [LaneBProbe]::Open('wiri-lane-b-h22-keys')
            Start-Sleep -Milliseconds 1500
            BFocus $h
            $startBefore = BStartOpen
            [LaneBProbe]::Clear($h)
            BChord @(0x5B) 0x75
            $startAfterLeft = BStartOpen
            BChord @(0x5C) 0x75
            $n = BWaitCount 'h22-win' 2
            $ev = @(BEvents $h 'h22-win')
            BGate 'h22_win_bind_fires' ($n -eq 2) $n
            BGate 'h22_win_bind_does_not_open_start' (-not $startBefore -and -not $startAfterLeft -and -not (BStartOpen) -and [LT]::GetForegroundWindow() -eq $h) @($startBefore, $startAfterLeft, [LT]::Title([LT]::GetForegroundWindow()))
            BGate 'h22_bound_key_down_and_up_are_consumed' (@($ev | Where-Object { $_ -eq 'KD:117' -or $_ -eq 'KU:117' }).Count -eq 0 -and $n -eq 2) $ev

            BFocus $h; [LaneBProbe]::Clear($h)
            BChord @() 0x41
            $ev = @(BEvents $h 'h22-unbound')
            BGate 'h22_unbound_key_down_and_up_pass' (BExact $ev @('KD:65','KU:65')) $ev
            foreach ($side in @(
                @('left_shift',0xA0), @('right_shift',0xA1), @('left_ctrl',0xA2), @('right_ctrl',0xA3),
                @('left_alt',0xA4), @('right_alt',0xA5), @('left_win',0x5B), @('right_win',0x5C)
            )) {
                BFocus $h; [LaneBProbe]::Clear($h)
                $vk = [int]$side[1]; BChord @($vk) 0x87
                $ev = @(BEvents $h ("h22-" + $side[0]))
                if ($winBaseline.ContainsKey($vk)) {
                    $pair = @($ev | Where-Object { $_ -ne 'KD:135' -and $_ -ne 'KU:135' })
                    $pass = (BExact $pair @("KD:$vk","KU:$vk")) -and (BExact $ev $winBaseline[$vk])
                    BGate ("h22_" + $side[0] + '_down_and_up_pass') $pass @{ without_wiri = $winBaseline[$vk]; with_wiri = $ev }
                } else {
                    BGate ("h22_" + $side[0] + '_down_and_up_pass') (BExact $ev @("KD:$vk",'KD:135','KU:135',"KU:$vk")) $ev
                }
            }
            BFocus $h
            BChord @(0xA2,0xA4) 0x75 4
            $n = BWaitCount 'h22-once' 1
            BGate 'h22_repeat_false_fires_once' ($n -eq 1) $n
            BChord @(0xA2,0xA4) 0x76 4
            $n = BWaitCount 'h22-repeat' 4
            BGate 'h22_repeat_true_fires_for_each_down' ($n -eq 4) $n
            BChord @(0xA2,0xA4) 0x77
            BChord @(0xA2,0xA4) 0x77
            $n = BWaitCount 'h22-cool' 1
            BGate 'h22_cooldown_suppresses_fast_presses' ($n -eq 1) $n
            Start-Sleep -Milliseconds 1900
            BChord @(0xA2,0xA4) 0x77
            $n = BWaitCount 'h22-cool' 2
            BGate 'h22_cooldown_expires' ($n -eq 2) $n

            [LaneBProbe]::Clear($h)
            $releaseKeyHeld = $false; $releaseModsHeld = $false
            try {
                $releaseModsHeld = $true; BMods @(0xA2,0xA4) $false
                $releaseKeyHeld = $true; [LaneBProbe]::Key(0x78,$false)
                $beforeUp = BObserveCount 'h22-release'
                # A captured key-up must stay captured after its modifiers leave.
                BMods @(0xA2,0xA4) $true; $releaseModsHeld = $false
                [LaneBProbe]::Key(0x78,$true); $releaseKeyHeld = $false
            } finally {
                if ($releaseKeyHeld) { [LaneBProbe]::Key(0x78,$true) }
                if ($releaseModsHeld) { BMods @(0xA2,0xA4) $true }
            }
            $n = BWaitCount 'h22-release' 1
            $ev = @(BEvents $h 'h22-release')
            BGate 'h22_release_fires_only_on_key_up' ($beforeUp -eq 0 -and $n -eq 1) @($beforeUp,$n)
            BGate 'h22_release_after_modifier_up_does_not_leak' ($n -eq 1 -and @($ev | Where-Object { $_ -match '^K[DU]:120$' }).Count -eq 0) $ev

            BChord @(0xA2,0xA4) 0x7A
            $baseline = BWaitCount 'h22-inhibit' 1
            BChord @(0xA2,0xA4) 0x79
            [LaneBProbe]::Clear($h)
            BChord @(0xA2,0xA4) 0x7A
            $inhibitedCount = BObserveCount 'h22-inhibit'
            $ev = @(BEvents $h 'h22-inhibited')
            BGate 'h22_inhibited_bind_passes_to_app' ($baseline -eq 1 -and $inhibitedCount -eq 1 -and (BExact $ev @('KD:162','KD:164','KD:122','KU:122','KU:164','KU:162'))) $ev
            [LaneBProbe]::Clear($h)
            BChord @(0xA2,0xA4) 0x7B
            $n = BWaitCount 'h22-bypass' 1
            $ev = @(BEvents $h 'h22-bypass')
            BGate 'h22_uninhibitable_bind_still_fires' ($n -eq 1 -and @($ev | Where-Object { $_ -match '^K[DU]:123$' }).Count -eq 0) $ev
            BChord @(0xA2,0xA4) 0x79
            BChord @(0xA2,0xA4) 0x7A
            $n = BWaitCount 'h22-inhibit' 2
            BGate 'h22_leaving_inhibition_restores_bind' ($n -eq 2) $n
            Shot 'lane-b-h22-keyboard'
        } catch { BError 'h22' $_ }
        finally { BCloseProbes; Start-Sleep -Milliseconds 350; BStop $b22 'h22' }

        # H23: explicit binds override default Mod drags, consume both button
        # transitions / the bound wheel axis, and accumulate sub-notch deltas.
        $b23 = $null
        try {
            $text = @"
input {
    mod-key "ctrl-alt"
}
binds {
    Mod+MouseLeft { $(BSpawn 'h23-left'); }
    Mod+MouseRight { $(BSpawn 'h23-right'); }
    Mod+MouseMiddle { $(BSpawn 'h23-middle'); }
    Mod+WheelScrollUp { $(BSpawn 'h23-up'); }
    Mod+WheelScrollDown { $(BSpawn 'h23-down'); }
    Mod+WheelScrollLeft { $(BSpawn 'h23-wheel-left'); }
    Mod+WheelScrollRight { $(BSpawn 'h23-wheel-right'); }
    Ctrl+Shift+WheelScrollUp { $(BSpawn 'h23-axis-up'); }
}
"@
            $b23 = BStart $text 'h23'
            $h = [LaneBProbe]::Open('wiri-lane-b-h23-pointer')
            Start-Sleep -Milliseconds 1500; BFocus $h; $null = BPointer $h
            [LaneBProbe]::Clear($h)
            foreach ($button in 'L','R','M') { BClick @() $button }
            $ev = @(BEvents $h 'h23-unbound-buttons' | Where-Object { $_ -match '^M[DU]:' })
            BGate 'h23_unbound_buttons_down_and_up_pass' (BExact $ev @('MD:L','MU:L','MD:R','MU:R','MD:M','MU:M')) $ev
            [LaneBProbe]::Clear($h)
            BWheel @() $false 120; BWheel @() $false -120
            BWheel @() $true -120; BWheel @() $true 120
            $ev = @(BEvents $h 'h23-unbound-wheel' | Where-Object { $_ -like 'W:*' })
            BGate 'h23_unbound_wheel_both_axes_pass' (BExact $ev @('W:V:120','W:V:-120','W:H:-120','W:H:120')) $ev
            foreach ($case in @(@('L','left'),@('R','right'),@('M','middle'))) {
                BFocus $h; $null = BPointer $h; [LaneBProbe]::Clear($h)
                BClick @(0xA2,0xA4) $case[0]
                $n = BWaitCount ("h23-" + $case[1]) 1
                $ev = @(BEvents $h ("h23-bound-" + $case[1]) | Where-Object { $_ -match '^M[DU]:' })
                BGate ("h23_" + $case[1] + '_mouse_bind_fires_and_consumes') ($n -eq 1 -and $ev.Count -eq 0) @($n,$ev)
            }
            foreach ($case in @(@('up',$false,120,'up'),@('down',$false,-120,'down'),@('left',$true,-120,'wheel-left'),@('right',$true,120,'wheel-right'))) {
                [LaneBProbe]::Clear($h)
                BWheel @(0xA2,0xA4) $case[1] $case[2]
                $n = BWaitCount ("h23-" + $case[3]) 1
                $ev = @(BEvents $h ("h23-bound-wheel-" + $case[0]) | Where-Object { $_ -like 'W:*' })
                BGate ("h23_wheel_" + $case[0] + '_bind_fires_and_consumes') ($n -eq 1 -and $ev.Count -eq 0) @($n,$ev)
            }
            [LaneBProbe]::Clear($h)
            # Keep one chord held: accumulation must not depend on preserving a
            # remainder across distinct modifier sessions.
            try {
                BMods @(0xA2,0xA4) $false
                [LaneBProbe]::Wheel($false,40); Start-Sleep -Milliseconds 160
                [LaneBProbe]::Wheel($false,40); Start-Sleep -Milliseconds 160
                $after80 = BObserveCount 'h23-up'
                [LaneBProbe]::Wheel($false,40); Start-Sleep -Milliseconds 160
            } finally { BMods @(0xA2,0xA4) $true }
            $n = BWaitCount 'h23-up' 2
            $ev = @(BEvents $h 'h23-high-resolution' | Where-Object { $_ -like 'W:*' })
            BGate 'h23_high_resolution_wheel_accumulates' ($after80 -eq 1 -and $n -eq 2 -and $ev.Count -eq 0) @($after80,$n,$ev)
            [LaneBProbe]::Clear($h)
            BWheel @(0xA2,0xA0) $true 120
            $axisCount = BObserveCount 'h23-axis-up'
            $ev = @(BEvents $h 'h23-unbound-axis' | Where-Object { $_ -like 'W:*' })
            BGate 'h23_unbound_axis_is_not_consumed' ((BExact $ev @('W:H:120')) -and $axisCount -eq 0) $ev
            Shot 'lane-b-h23-mouse-binds'
        } catch { BError 'h23' $_ }
        finally { BCloseProbes; Start-Sleep -Milliseconds 350; BStop $b23 'h23' }

        # H24: never grab the title bar / resize frame. The point is inside an
        # EDIT inside a panel. Floating geometry and a tiled column are checked.
        $b24 = $null
        try {
            $b24 = BStart "input {`n    mod-key `"ctrl-alt`"`n}`n" 'h24'
            $h = [LaneBProbe]::Open('wiri-lane-b-h24-client-drag')
            Start-Sleep -Milliseconds 1500; BFocus $h
            $null = BCheckedCtl 'lane-b-h24-float' @('action','move-window-to-floating')
            Start-Sleep -Milliseconds 1300
            if ((WinOf (WinList 'lane-b-h24-float-prerequisite') $h).floating -ne $true) { throw 'Drag prerequisite: probe did not become floating' }
            $pt = BPointer $h
            $native = New-Object LT+POINT; $native.X = $pt.X; $native.Y = $pt.Y
            $child = [LT]::WindowFromPoint($native)
            $root = [LT]::GetAncestor($child,2)
            $isChild = ($child -ne [IntPtr]::Zero -and $child -ne $h -and $root -eq $h)
            BGate 'h24_drag_starts_over_nested_client_child' $isChild @([int64]$child,[int64]$root,[LT]::Class($child))
            [LaneBProbe]::Clear($h)
            $before = Rc $h
            $area = [System.Windows.Forms.Screen]::FromHandle($h).WorkingArea
            $dx = if ($before.R + 90 -lt $area.Right) { 80 } else { -80 }
            $dy = if ($before.B + 65 -lt $area.Bottom) { 55 } else { -55 }
            BDrag $h 'L' 0.5 0.5 $dx $dy
            $moved = Rc $h
            BGate 'h24_mod_left_drag_moves_root' ($isChild -and (Near ($moved.L - $before.L) $dx 18) -and (Near ($moved.T - $before.T) $dy 18) -and (WinOf (WinList 'lane-b-h24-after-move') $h).floating -eq $true) @($before,$moved)
            BDrag $h 'R' 0.8 0.8 80 45
            $resized = Rc $h
            $floatResized = ($isChild -and ($resized.W - $moved.W) -ge 40 -and ($resized.H - $moved.H) -ge 20)
            BGate 'h24_mod_right_drag_resizes_root' $floatResized @($moved,$resized)
            $null = BCheckedCtl 'lane-b-h24-float-refresh' @('action','refresh')
            Start-Sleep -Milliseconds 1100; $floatRefresh = Rc $h
            BGate 'h24_floating_resize_survives_refresh' ($floatResized -and (Near $floatRefresh.L $resized.L 14) -and (Near $floatRefresh.T $resized.T 14) -and (Near $floatRefresh.W $resized.W 14) -and (Near $floatRefresh.H $resized.H 14)) @($resized,$floatRefresh)
            $null = BCheckedCtl 'lane-b-h24-tile' @('action','move-window-to-tiling')
            Start-Sleep -Milliseconds 1300; BFocus $h
            $tiled = Rc $h
            BDrag $h 'R' 0.8 0.55 -100 0
            $sized = Rc $h
            $changed = (($tiled.W - $sized.W) -ge 45 -and (WinOf (WinList 'lane-b-h24-after-resize') $h).floating -eq $false)
            BGate 'h24_tiled_resize_changes_column_width' $changed @($tiled,$sized)
            $null = BCheckedCtl 'lane-b-h24-refresh-1' @('action','refresh')
            Start-Sleep -Milliseconds 1100; $refresh1 = Rc $h
            $null = BCheckedCtl 'lane-b-h24-refresh-2' @('action','refresh')
            Start-Sleep -Milliseconds 1100; $refresh2 = Rc $h
            BGate 'h24_tiled_resize_survives_refresh' ($changed -and (Near $sized.W $refresh1.W 14) -and (Near $sized.W $refresh2.W 14)) @($sized,$refresh1,$refresh2)
            $ev = @(BEvents $h 'h24-drags' | Where-Object { $_ -match '^M[DU]:' })
            BGate 'h24_drag_button_transitions_are_consumed' ($isChild -and $ev.Count -eq 0) $ev
            Shot 'lane-b-h24-client-drags'
        } catch { BError 'h24' $_ }
        finally { BCloseProbes; Start-Sleep -Milliseconds 350; BStop $b24 'h24' }

        # H25: delay is measured without IPC in the early observation interval.
        # Both targets fit on screen, so no strip scrolling is needed to focus.
        $b25 = $null; $bWarp = $null
        try {
            $text = @'
input {
    mod-key "ctrl-alt"
    focus-follows-mouse max-scroll-amount="0%"
    focus-delay-ms 1000
    warp-mouse-to-focus false
}
layout {
    column-width 400
}
'@
            $b25 = BStart $text 'h25-delay'
            $a = [LaneBProbe]::Open('wiri-lane-b-h25-a')
            $b = [LaneBProbe]::Open('wiri-lane-b-h25-b')
            Start-Sleep -Milliseconds 1800
            BFocus $a; $null = BPointer $a
            $pt = [LaneBProbe]::ChildPoint($b,0.3,0.35)
            $native = New-Object LT+POINT; $native.X = $pt.X; $native.Y = $pt.Y
            $child = [LT]::WindowFromPoint($native)
            $hit = ($child -ne $b -and $child -ne [IntPtr]::Zero -and [LT]::GetAncestor($child,2) -eq $b)
            BGate 'h25_hover_targets_nested_client_child' $hit @([int64]$child,[LT]::Class($child))
            $clock = [Diagnostics.Stopwatch]::StartNew()
            [LaneBProbe]::Move($pt.X,$pt.Y)
            $early = $true; $lastEarly = 0; $firstChange = $null
            while ($clock.ElapsedMilliseconds -lt 600) {
                $lastEarly = $clock.ElapsedMilliseconds
                if ([LT]::GetForegroundWindow() -ne $a) { $early = $false; $firstChange = $clock.ElapsedMilliseconds; break }
                Start-Sleep -Milliseconds 50
            }
            while ($clock.ElapsedMilliseconds -lt 2300 -and $null -eq $firstChange) {
                if ([LT]::GetForegroundWindow() -ne $a) { $firstChange = $clock.ElapsedMilliseconds; break }
                Start-Sleep -Milliseconds 50
            }
            $fg = [LT]::GetForegroundWindow(); $cursor = [LaneBProbe]::Cursor()
            BGate 'h25_focus_does_not_change_before_delay' ($hit -and $early -and $lastEarly -ge 450) @($lastEarly,$firstChange)
            $hoverWorks = ($hit -and $fg -eq $b -and $null -ne $firstChange -and $firstChange -ge 800 -and $firstChange -lt 2300)
            BGate 'h25_focus_changes_after_delay' $hoverWorks @($firstChange,[int64]$fg)
            BGate 'h25_disabled_warp_keeps_cursor' ($fg -eq $b -and (Near $cursor.X $pt.X 2) -and (Near $cursor.Y $pt.Y 2)) @($pt,$cursor)
            BFocus $a; $null = BPointer $a
            [LaneBProbe]::Move($pt.X,$pt.Y); Start-Sleep -Milliseconds 250
            $back = [LaneBProbe]::ChildPoint($a,0.5,0.5)
            [LaneBProbe]::Move($back.X,$back.Y)
            $cancelled = ([LT]::GetForegroundWindow() -eq $a)
            for ($i = 0; $i -lt 14; $i++) {
                Start-Sleep -Milliseconds 100
                if ([LT]::GetForegroundWindow() -ne $a) { $cancelled = $false }
            }
            BGate 'h25_leaving_target_cancels_pending_focus' ($hoverWorks -and $cancelled) ([int64][LT]::GetForegroundWindow())
            Shot 'lane-b-h25-delayed-focus'
            # Reuse this strip: both probes have positively demonstrated a
            # visible hit. A single zero-delay entry must focus without another
            # packet/re-entry, even when GetCursorPos trails WH_MOUSE_LL.
            [IO.File]::WriteAllText($cfg, $text.Replace('focus-delay-ms 1000','focus-delay-ms 0'), (New-Object Text.UTF8Encoding($false)))
            $null = BCheckedCtl 'lane-b-h25-zero-delay-reload' @('reload-config')
            Start-Sleep -Milliseconds 900
            BFocus $a; $null = BPointer $a
            $instant = [LaneBProbe]::ChildPoint($b,0.3,0.35)
            $instantHit = [LaneBProbe]::AtChild($b,$instant)
            if (-not $instantHit) { throw 'Zero-delay probe is not a visible nested EDIT' }
            [LaneBProbe]::Move($instant.X,$instant.Y)
            $instantClock = [Diagnostics.Stopwatch]::StartNew()
            while ($instantClock.ElapsedMilliseconds -lt 700 -and [LT]::GetForegroundWindow() -ne $b) { Start-Sleep -Milliseconds 25 }
            BGate 'h25_zero_delay_hover_focuses_without_reentry' ($hoverWorks -and $instantHit -and [LT]::GetForegroundWindow() -eq $b) $instantClock.ElapsedMilliseconds
            BStop $b25 'h25-delay'
            $bWarp = BStart @'
input {
    mod-key "ctrl-alt"
    focus-follows-mouse false
    warp-mouse-to-focus mode="center-xy-always"
}
layout {
    column-width 400
}
'@ 'h25-warp'
            # Start with the other window focused so both assertions are actual
            # focus changes; place the pointer away from the expected centre.
            BFocus $b
            foreach ($case in @(@($a,'first'),@($b,'next'))) {
                $target = [IntPtr]$case[0]
                # The next tile may still be parked off-screen. Generate real
                # motion over the CURRENT visible probe, not its old rectangle.
                $current = [LT]::GetForegroundWindow()
                $null = BPointer $current 0.2 0.2
                $pre = [LaneBProbe]::Cursor()
                BFocus $target
                # The warp is applied by the event-driven focus handler after
                # the IPC reply returns, and warp_geometry_ready deliberately
                # waits out the native layout settle, so the daemon aims at the
                # centre of the rect it saw at warp time - which can differ
                # from the settled rect the gate measures (cycle evidence:
                # warp destination x=1121 vs settled centre x=1158). Poll
                # until the cursor comes to rest inside the focused window's
                # rect instead of sampling once against a pre-settle centre.
                $warpClock = [Diagnostics.Stopwatch]::StartNew()
                $post = [LaneBProbe]::Cursor()
                $r = Rc $target
                while ($warpClock.ElapsedMilliseconds -lt 800 -and -not ($post.X -ge $r.L -and $post.X -le $r.R -and $post.Y -ge $r.T -and $post.Y -le $r.B)) {
                    Start-Sleep -Milliseconds 25
                    $post = [LaneBProbe]::Cursor()
                    $r = Rc $target
                }
                $cx = ($r.L + $r.R) / 2; $cy = ($r.T + $r.B) / 2
                $wasAway = (-not (Near $pre.X $cx 40) -or -not (Near $pre.Y $cy 40))
                BGate ("h25_warp_" + $(if ($case[1] -eq 'first') { 'centres_first_focused_window' } else { 'tracks_next_focused_window' })) ($wasAway -and [LT]::GetForegroundWindow() -eq $target -and ($post.X -ge $r.L) -and ($post.X -le $r.R) -and ($post.Y -ge $r.T) -and ($post.Y -le $r.B)) @($r,$pre,$post,$warpClock.ElapsedMilliseconds)
            }
            Shot 'lane-b-h25-cursor-warp'
        } catch { BError 'h25' $_ }
        finally { BCloseProbes; Start-Sleep -Milliseconds 350; BStop $b25 'h25-delay-final'; BStop $bWarp 'h25-warp' }
    } catch { BError 'setup' $_ }
    finally {
        # Release only the injected controls, close only our own probe windows,
        # and stop only Process objects returned by our own Start-Wiri calls.
        if ('LaneBProbe' -as [type]) {
            try { BCloseProbes } catch { BError 'input-cleanup' $_ }
        }
        foreach ($p in $bOwned) { try { BStop $p 'cleanup' } catch { BError ("cleanup-pid-" + $p.Id) $_ } }
        if ($null -ne $bSavedCursor) { try { [LaneBProbe]::Move($bSavedCursor.X,$bSavedCursor.Y) } catch { BError 'cursor-cleanup' $_ } }
        $summary['ctl_timeouts'] = @($steps | Where-Object { $_.rc -eq 'timeout' }).Count
        $summary['ctl_max_ms'] = (($steps | Where-Object { $_.ms } | Measure-Object -Property ms -Maximum).Maximum)
        $bDetails['gate-count'] = $bGates.Count
        $bDetails['caveats'] = @(
            'Application-queue transitions are captured on harness-owned nested WinForms EDIT controls, not Chrome/Electron.',
            'F24 chords exercise standalone modifier edges without invoking bare Win/Alt shell actions; AltGr/layout synthesis is recorded and fails exact-transition gates.',
            'Delay gates use 50 ms observations, 1000 ms configured delay and an 800 ms lower bound; scheduling stalls can fail them.',
            'Start detection uses visible uncloaked StartMenuExperienceHost / known Start shell windows plus foreground retention; screenshots remain available.',
            'max-scroll-amount=0% is configured, but this single-display on-screen focus check does not prove an off-screen scrolling limit.',
            'Aggregate CLI timings/timeouts include Lane B; required drag control actions must succeed.'
        )
        $bDetails | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $bDir 'results.json')
    }
}
