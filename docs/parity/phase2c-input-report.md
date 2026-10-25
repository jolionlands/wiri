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
