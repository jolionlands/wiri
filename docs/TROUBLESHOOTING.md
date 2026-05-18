# Troubleshooting

A grab-bag of issues you may hit and how to recover.

## Startup

### "Another wiri instance is already running"

The IPC named pipe at `\\.\pipe\wiri_control` is held by another process. If
you're sure wiri isn't running, the pipe may be orphaned. Either:

```pwsh
wiri-ctl quit         # graceful shutdown of the other instance
# or
Get-Process wiri | Stop-Process
```

Then start wiri again.

### Hotkeys don't fire on ARM64

You have a third-party tool holding the hotkey, or you've enabled
`MOD_NOREPEAT` somewhere. wiri uses software debouncing on ARM64 (see
`backend/message_loop.rs`) because `RegisterHotKey` rejects `MOD_NOREPEAT`
on aarch64. Don't add it back.

Common conflicting tools: AutoHotkey scripts, Windows accessibility shortcuts,
PowerToys Always-On-Top.

### Config "watcher could not start"

`notify` failed to install its file-system watcher. wiri continues with the
config it loaded at startup; hot-reload is just disabled. Run with `-v` to see
the underlying OS error. Common causes:

- The config directory was removed after wiri started.
- The drive doesn't support change notifications (some network mounts).

A `wiri-ctl reload-config` still works.

## Window behaviour

### A window keeps respawning at the wrong size

Some apps fight `SetWindowPos`. wiri retries up to three times; after the third
failure the window is auto-floated and stays where the app puts it. Look for
log lines like `auto-floated <hwnd> after 3 SetWindowPos failures`. Add a
window rule to make this explicit:

```kdl
window-rules {
    class "ProblemApp" { float true }
}
```

### A UWP / Windows Terminal window goes blank after tiling

The `strip_frame_for_tiling` call removes Win32 frames, and a handful of apps
(Windows Terminal is the canonical example) won't repaint until they receive a
DWM frame extension. Enable shadow rendering, which also triggers a repaint:

```kdl
layout {
    shadow-enable true
}
```

### Borders aren't coloured on Windows 10

Coloured borders use `DWMWA_BORDER_COLOR`, which was added in Windows 11 build
22000. On Windows 10 wiri falls back to a wider visual gap for the focused
window — it still distinguishes focus, just less elegantly.

### Anti-cheat games (Valorant, Apex, etc.) crash on launch

The `WH_MOUSE_LL` global mouse hook trips kernel-mode anti-cheat. Quit wiri
before launching, or run wiri without the mouse hook:

```pwsh
.\wiri.exe --no-mouse-hook   # if the flag exists in your build
```

(If the flag isn't there yet, comment out the `start_mouse_hook(...)` call in
`src/main.rs` and rebuild — it's two lines.)

## IPC

### `wiri-ctl ...` says "wiri server busy or not running"

The pipe is unreachable. Either wiri isn't running, or the pipe wait timed out
under heavy IPC load. Bump the timeout:

```pwsh
wiri-ctl --timeout 30 state
```

If wiri *is* running, look for a deadlock — check `wiri-ctl state` for
suspicious `failed_position` entries or zero monitors.

### JSON output instead of formatted text

Add `--json` if you wanted that, omit it if you didn't. Default is
human-formatted; `--json` is the explicit machine-readable opt-in.

## Build & development

### `cargo build` fails with "linker `link.exe` not found"

You need either the Microsoft Build Tools (with the Windows 10 SDK + the C++
build tools workload) or the `windows-msvc` toolchain installed via
`rustup target add aarch64-pc-windows-msvc`. The bundled `link.bat` is a
one-off `lld-link` wrapper for the original author's setup; it's not used by
`cargo build` and is gitignored.

### `--version` doesn't show the git commit hash

The `build.rs` script reads `git rev-parse --short HEAD`. If you're building
from a tarball (not a git checkout) it'll silently produce just the crate
version. If you're in a git checkout and still don't see the hash, the build
script cache is stale — delete `target/release/build/wiri-*` and rebuild.

## Reporting bugs

Open an issue at <https://github.com/jolionlands/wiri/issues> with:

- Windows version (`winver`)
- wiri version output (`wiri --version`)
- The full output of `wiri-ctl list-monitors`
- A minimal config that reproduces the issue
- The tail of `wiri_log.txt` (or stderr captured with `-v`)
