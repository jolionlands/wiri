# wiri

A scrollable-tiling window manager for Windows, inspired by [niri](https://github.com/YaLTeR/niri).

Windows are arranged in **columns on an infinite horizontal strip**. Opening a new window never causes existing windows to resize — they just push the viewport. Each monitor gets its own independent strip of dynamic workspaces, à la GNOME.

Built in Rust, runs natively on Win32 (x86_64 and ARM64).

## Status

- Compiles clean on `aarch64-pc-windows-msvc` and `x86_64-pc-windows-msvc`
- 319 unit tests passing
- Released as a working prototype — see `docs/PROGRESS.md` for the current feature matrix

## Build

```pwsh
git clone https://github.com/jolionlands/wiri.git
cd wiri
cargo build --release
```

Produces:
- `target/release/wiri.exe` — the window manager (~2.4 MB)
- `target/release/wiri-ctl.exe` — IPC control CLI (~710 KB)

Minimum Rust: stable 1.78+. No build-script magic.

## Run

```pwsh
.\target\release\wiri.exe          # default config
.\target\release\wiri.exe -v       # verbose logs
.\target\release\wiri.exe -c my.kdl
.\target\release\wiri.exe --no-tray
```

First run writes a default config to `%APPDATA%\wiri\config.kdl`. Edit it and save — wiri hot-reloads.

To stop: `wiri-ctl quit`, the tray menu, or `Ctrl+Alt+Shift+Q`.

## Default key bindings

All defaults use the `Ctrl+Alt` prefix. Override the whole set in the `binds {}` section of `config.kdl`.

| Key | Action |
| --- | --- |
| ← / → | Focus column left/right |
| ↑ / ↓ | Focus tile above/below in column |
| Shift+← / → | Move column left/right |
| H / L | Scroll workspace left/right |
| Q | Close focused window |
| Enter | Spawn `cmd.exe` |
| F | Toggle fullscreen |
| T | Toggle floating |
| Tab | Overview (zoom out) |
| O | Select window in overview |
| 1 – 9 | Switch workspace |
| Shift+Q | Quit wiri |
| **Alt + Left-drag** | Interactive window move |
| **Alt + Right-drag edge** | Interactive resize |

## Configuration

Configuration uses [KDL](https://kdl.dev). The default config in `resources/default_config.kdl` is the source of truth — copy it to `%APPDATA%\wiri\config.kdl` and edit. See `docs/DOCUMENTATION.md` for every supported field.

Highlights:

```kdl
layout {
    column-width 500
    inner-gaps 16
    outer-gaps 8
    border-color "#333333"
    border-color-focused "#0066cc"
}

animations {
    enabled true
    duration 200
    easing "ease-out-cubic"
}

binds {
    Ctrl+Alt+Enter spawn "wt.exe"
    Ctrl+Alt+B spawn "chrome.exe"
}

window-rules {
    class "CalcFrame" { float true }
}
```

## IPC

wiri exposes a named pipe at `\\.\pipe\wiri_control`. The bundled `wiri-ctl` CLI is the friendly front end:

```pwsh
wiri-ctl state                       # current engine state
wiri-ctl windows                     # list managed windows
wiri-ctl switch-workspace 2
wiri-ctl focus-window <hwnd>
wiri-ctl close-window <hwnd>
wiri-ctl move-window <hwnd> <x> <y>
wiri-ctl resize-window <hwnd> <w> <h>
wiri-ctl reload-config
wiri-ctl quit
wiri-ctl events --types '*'          # stream events
```

The wire format is line-delimited JSON. See `docs/DOCUMENTATION.md` § 6 for the full protocol.

## Architecture

See `docs/SPEC.md` for the full design, `docs/DOCUMENTATION.md` for the module reference, and `docs/PROGRESS.md` for what's implemented. A short summary:

```
┌────────────────────── wiri (main.rs) ─────────────────────┐
│                                                            │
│  Config ─→ LayoutConfig ─→ TilingEngine ─→ apply_layout   │
│                                ↑    │                      │
│  Backend (Win32) ── WinEventHook ── BackendEvent           │
│                                ↑    │                      │
│  MessageLoop (hotkeys) ────── Action ──→ TilingEngine      │
│                                                            │
│  MouseHook (WH_MOUSE_LL) ──→ GrabState ─→ move/resize     │
│                                                            │
│  IPC Server ←── Named Pipe ──→ wiri-ctl                    │
│                                                            │
│  TrayIcon ←── Context Menu ──→ Reload / Quit               │
└────────────────────────────────────────────────────────────┘
```

## Limitations

- Fullscreen-exclusive games cannot be tiled (no compositor can).
- Some UWP apps expose limited HWND control.
- A handful of anti-cheat shims dislike `WH_MOUSE_LL` — disable the mouse hook in those games.

## Credits

The scrollable-tiling paradigm and most of the UX vocabulary come from [niri](https://github.com/YaLTeR/niri) by Ivan Molodetskikh. wiri is an independent port to Win32; no niri code is reused.

## License

MIT. See [LICENSE](LICENSE).
