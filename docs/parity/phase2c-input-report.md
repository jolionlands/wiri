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
