# Phase 2C, lane B: input

Reference: niri main `e19df12cf8`, Configuration:-Key-Bindings and Configuration:-Input.

## 1. Keyboard

`WH_KEYBOARD_LL` is primary, installed on the existing hotkey message thread. The callback
captures only a matching non-modifier press, then consumes its repeats and release even
if modifiers are released first, inhibition changes, or the config reloads. Unbound downs
are not retroactively captured. All physical modifiers pass unchanged. A tagged pair of
unassigned VK 0xE8 events masks the shell's lone Win/Alt menu trigger after a captured chord;
there are no fake modifier-up events. Win+L, Win+D and Win+Shift+S remain denied in both paths.
Bare overview Escape is observed, not captured, so applications keep Escape.

`repeat=false`, `cooldown-ms` and `allow-inhibiting=false` are evaluated per binding.
`toggle-keyboard-shortcuts-inhibit` works from IPC or a bind and always remains available;
manual inhibition is global. Windows has no Wayland per-client inhibitor protocol, so automatic
application inhibition is not claimed. An inhibited bind's fallback registration is removed.

Example (niri syntax, except the explicitly labelled release extension):

```kdl
input { mod-key "super"; }
binds {
    Mod+Right { focus-column-right; }
    Mod+Escape { toggle-keyboard-shortcuts-inhibit; }
    Mod+T repeat=false cooldown-ms=500 allow-inhibiting=false { toggle-window-floating; }
    // wiri extension: pinned niri's wiki has no release property.
    Mod+F9 on-release=true { focus-column-first; }
}
```

`release=true` aliases `on-release=true`. Release bindings fire on the captured key-up,
independent of whether modifiers are still down; they are deliberately not registered with
RegisterHotKey, which cannot observe up events. RegisterHotKey remains a down-only fallback
where the low-level hook is unavailable (for example integrity restrictions). A consumed
hook event never generates WM_HOTKEY, so the two paths do not double-dispatch. The fallback
cannot claim shell-owned chords if registration fails, nor paired suppression/release parity.
The old action-wide debounce is gone: separate bindings are not accidentally rate-limited
against one another.

Keyboard validation: warnings-denied release check passed; release tests passed (885 library
and 4 CLI tests, 1 ignored doc test), including 13 keyboard state-machine/config tests.
Desktop gates are added in `scripts/live-phases/lane-b.ps1`; final run results below will
record what was actually verified rather than treating unit coverage as desktop evidence.

No dependencies added. Keyboard changes remove the separate keyboard-hook and action-relay
threads and use the existing message pump. No timer, polling loop, or new resting window.

## 2. Mouse and wheel

`Mod+MouseLeft/Right/Middle/Back/Forward` and `WheelScrollUp/Down/Left/Right` use niri syntax.
An explicit button bind wins over a move/resize gesture and acts on the previously focused
window, not the window clicked. Bound button down/up pairs survive modifier release and reload;
release bindings and cooldown also work. Mod+left moves and Mod+right resizes when not explicitly
bound. The root is found with `GetAncestor(GA_ROOT)` after `WindowFromPoint`, so nested client
children no longer prevent a grab. Resize edges retain their precise edge hit-test; client
interiors choose the nearest corner. Tile resize writes column width and height through existing
engine setters; floating writeback uses a minimal absolute-rectangle wrapper around `place_float`
to retain limits and floating memory without racing queued HWND updates. No other engine logic
is changed. Layout/backend work runs off the LL callback on the existing hotkey thread.

Wheel bindings consume both directions of a bound axis, including fractional packets, but pass
an unbound axis unchanged. Signed deltas accumulate to Windows' 120-unit ticks, resetting on
modifier/config change; natural-scroll and per-bind cooldown apply. TouchpadScroll names alias
the same stream: WH_MOUSE_LL cannot distinguish precision touchpads from wheels, so separate
hardware policies and native gestures are not claimed. Wheel shortcuts must be configured;
there is no hard-coded Alt or Ctrl+Alt wheel action and no worker spawned per scroll event.

```kdl
binds {
    Mod+MouseMiddle { toggle-window-floating; }
    Mod+WheelScrollUp cooldown-ms=150 { focus-workspace-up; }
    Mod+WheelScrollDown cooldown-ms=150 { focus-workspace-down; }
    Mod+WheelScrollLeft { focus-column-left; }
    Mod+WheelScrollRight { focus-column-right; }
}
```

Pointer validation: release suite passed (893 library + 4 CLI tests, one ignored doc test),
including seven pure pointer tests and client-quadrant geometry. Real nested-child move/resize,
paired application-queue transitions and wheel-axis gates are in the lane-B desktop phase.
No new dependency, idle timer, input thread, or resting window. The existing lazy snap guide
is not created merely to hide it at drag completion.

## 3. Event-driven focus and warp

`input/focus.rs` replaces the main task's 500 ms cursor poll. WH_MOUSE_LL publishes only the
latest point and one coalesced thread message; hit testing, engine locks and activation run on
the existing hotkey thread. A positive `focus-delay-ms` arms a cancellable one-shot HWND timer
only while a different managed target is pending. Moving within that target retains its entry
deadline. Leaving it, changing foreground, reloading, shutdown or a grab cancels the dwell;
expiry revalidates the live pointer, root, active workspace and output. Entering an empty output
changes output selection without activating a window through an unmanaged overlay.

```kdl
input {
    focus-follows-mouse max-scroll-amount="10%"
    // wiri extension: pinned niri does not expose this dwell option.
    focus-delay-ms 200
    warp-mouse-to-focus mode="center-xy"
}
```

The reveal cap is a percentage of the output work-area width, measured against a candidate
workspace using the engine's actual constrained column widths; rejected targets never mutate
scroll. `0%` permits only an already visible column. Default warp centers only axes outside the
focused window; `center-xy` centers both axes when outside, and `center-xy-always` always centers.
Pointer-driven focus never warps. No `ShowCursor` is used: hidden cursors remain hidden.

Foreground WinEvents cancel obsolete dwell even when warp is disabled. Warp installs an
additional focus/location completion hook only when enabled. Early asynchronous activation or
positioning leaves a pending request until the native DWM rectangle reaches the submitted layout
position on an output. The isolated read-only `TilingEngine::pointer_warp_target` accessor exposes
that placement cache: observed `WindowInfo.bounds` can still be a parked or superseded rectangle.
Owned dialogs use their own native rectangle. No retries by polling, timers or extra threads.

**Unit-tested only:** 27 focus/config tests cover dwell deadlines/cancellation, pure target and
output resolution, negative coordinates, constrained reveal limits (including fractional/exact
boundaries), defaults and all warp modes, and pending-foreground deduplication. One engine test
checks submitted versus stale observed geometry. Single-display live checks cannot prove empty
output entry, multi-output membership, hidden-cursor behavior or an off-screen reveal boundary.
Floating membership still inherits the engine's workspace-id-only model; full multi-output float
parity is not claimed. Automatic client shortcut inhibition, AltGr/non-US layouts, native precision
gestures, elevated applications and the hook-unavailable fallback need further desktop coverage.

## 4. Final validation and desktop evidence

Branch `meylin/sa-cb73e-6b647`; Windows 11 x64, one unchanged 1366x768 display, rustc 1.95.
Final code passed all required commands, with `RUSTFLAGS=-D warnings` throughout:

- `cargo test --release -j 2`: **923 library + 4 CLI passed**, 0 failures, 1 ignored doc example.
  This lane adds 51 library tests over the 872-test baseline.
- `cargo check --release --all-targets -j 2`: passed.
- `cargo build --release -j 2`: passed.
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\live-test.ps1 -Bin target\release -Out .live7 -LockWaitMinutes 60`:
  completed normally, but one existing gate failed (below). `summary.json`, per-step files,
  `lane-b/results.json`, application input traces and screenshots are local evidence under
  `.live7/` (not checked into Git). A normal script exit alone does not mean every gate passed.

| Desktop group | Gates | Result | What was actually exercised |
| --- | ---: | --- | --- |
| H22 keyboard | 21 | 21 pass | Win+F6 acts without Start; captured key pair absent from app queue; unbound F24 chord baselines; both sides of Shift/Ctrl/Alt/Win; repeat, cooldown, release after modifiers leave; manual inhibition/escape hatch |
| H23 mouse/wheel binds | 11 | 11 pass | Unbound left/right/middle and both wheel axes reach app unchanged; three bound button pairs and four wheel directions consumed; 40+40+40 high-resolution tick; unbound horizontal axis passes |
| H24 client grabs | 7 | 7 pass | Nested EDIT-to-root hit; Mod+left moves a float; Mod+right resizes float and tile; floating/column geometry survives refresh; button edges do not leak |
| H25 focus/warp | 8 | 8 pass | Nested EDIT hover, 1000 ms dwell (observed 50 ms sampling), leave-to-cancel, zero-delay hover without re-entry, warp-disabled cursor retained; center-xy-always follows two IPC focus changes |
| Existing report-classified gates | 277 | 276 pass, 1 fail | H5 float-above-focused-tile failed; idle positioning, IPC, crash restoration, Escape and all other prior gates passed |
| Total report-classified gates | **324** | **323 pass, 1 fail** | `scripts/live-report.py` classification; 311 boolean fields, including the expected-false crash-record field, plus 13 numeric checks |

**Unresolved existing-gate failure:** `h5_float_stays_above_the_focused_tile=false` in
`.live7`, as also seen intermittently in `.live5`. It passed in `.live6` (323/323 gates on
that earlier code, before the additional zero-delay check). Hover and warp are disabled in H5,
so no pointer-focus WinEvent hooks or dwell timer are active there. Native floating/tiled
stacking is outside this lane; no stacking fix or clean-baseline proof is claimed. The required
all-existing-gates-green acceptance condition is therefore **not met** on the final run.
No assertion was weakened or existing phase changed to disguise this result.

**Remaining numeric discrepancy:** `h9_overlapping_terminal_pairs=1` in the final run,
although the existing report classifies it as information, not a failing gate. In `.live6`
the native Terminal widths were 466 px with two left edges -10 and 445 (455 px spacing):
11 px overlap. This previously observed minimum-width/inset layout issue is not fixed in this
input lane; earlier `.live4` and `.live5` also recorded it. Green report gates would not prove
zero terminal overlap.

Final run: **0 IPC timeouts**, maximum CLI call **399 ms**; settled positioning **0/s** for
both forms and terminals; unknown-key, bind and activation-failure counts **0**; no visible
wiri-owned window at rest. Win+L/Win+D/Win+Shift+S denial is unit-tested, deliberately not
injected on the shared desktop. Mouse Back/Forward, natural-scroll and mouse release/cooldown
logic are unit-tested but not live-proven. The probes are harness-owned WinForms EDIT children,
not Chrome/Electron, and no browser compatibility is claimed.

### Defects the desktop checks found

- `SetCursorPos` in a probe does not exercise the low-level motion path reliably. Probes now
  use checked absolute virtual-desktop `SendInput` movement; the application queue is observed
  for keyboard/button pairs instead of assuming an action proves input suppression.
- Consuming WH_MOUSE_LL motion froze the system cursor; button-up then reported the drag's
  starting point and undid the preview. Motion now passes, while paired button release remains
  captured. Two pure grab-routing tests plus H24 guard this fix.
- Foreground can arrive before activation and native positioning finish. Dropping an early
  request or validating against stale observed geometry lost the warp. Pending-request tests,
  the submitted-frame accessor and H25 cover the correction without a retry clock.
- WH_MOUSE_LL posts motion before Windows updates GetCursorPos. Re-querying it during zero-delay
  focus could settle a target without ever activating it. Immediate focus now carries the hook's
  logical point; timer expiry still obtains a fresh native point. Failed revalidation invalidates
  settlement so later motion can retry. A pure regression and the new H25 zero-delay gate cover it;
  a follow-up read-only review found no further concrete issue in that correction.

### Harness safety, dependencies and idle cost

The machine-wide lock is retained. The harness refuses an existing daemon rather than stopping
unowned processes, and starts only wiri with isolated APPDATA/LOCALAPPDATA below its output
folder; Terminal and other probe processes keep their real environment. Lane B pairs/releases
only injected controls, keeps owned Process handles, checks required IPC return codes, and verifies
the connected pipe server's PID before sending quit. Its one dot-source line precedes summary
serialization; the existing scenarios are unchanged apart from those safety/isolation fixes.

No crate or Windows feature was added or upgraded; Cargo.toml and Cargo.lock are unchanged.
No new always-on thread, idle timer, polling loop or resting window. Keyboard/dispatch work uses
the existing hotkey pump, removing the old separate keyboard-hook/action-relay threads; focus
removes the 500 ms polling task. The sole new timer exists only during a positive hover dwell
and is killed on expiry/cancellation. Optional WinEvent hooks allocate on enable, not at default
disabled startup. Existing main-loop/reconciliation/overlay polling outside this lane remains.
