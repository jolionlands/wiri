# wiri Quickstart — Three Minutes to Tiling

A scrollable-tiling window manager for Windows, inspired by
[niri](https://github.com/YaLTeR/niri).

This guide assumes a fresh Windows 10 or 11 machine.

---

## 1. Install prerequisites (~30 seconds)

* **Rust toolchain** — install via [rustup.rs](https://rustup.rs).  wiri builds
  on both `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc` (Snapdragon
  X / Surface Pro 11).
* **Git** — to clone the repo.

```powershell
winget install -e --id Rustlang.Rustup
winget install -e --id Git.Git
```

## 2. Clone and build (~1 minute on a release build)

```powershell
git clone https://github.com/wiri-wm/wiri.git
cd wiri
cargo build --release
```

The output binaries are:

* `target\release\wiri.exe`     — the daemon
* `target\release\wiri-ctl.exe` — the CLI client

There is no installer; copy both into `%LOCALAPPDATA%\Programs\wiri\` (or
any directory on PATH) and you're done.

## 3. Run wiri

```powershell
.\target\release\wiri.exe
```

You'll see a tray icon (right-click for the menu).  Open a few apps —
they will be added to the current workspace's scrollable strip
automatically.  No window is ever resized to fit another; instead the
strip extends infinitely to the right and you scroll between columns.

## 4. Default keybindings

All bindings can be remapped in `config.kdl`.  The default modifier is
**Ctrl+Alt**.

| Key combo            | Action                                  |
| -------------------- | --------------------------------------- |
| `Ctrl+Alt+←/→`       | Focus column left / right               |
| `Ctrl+Alt+↑/↓`       | Focus up / down within a column         |
| `Ctrl+Alt+Shift+←/→` | Move the focused column left / right    |
| `Ctrl+Alt+H/L`       | Scroll the view left / right (vim-keys) |
| `Ctrl+Alt+1..9`      | Switch to workspace N                   |
| `Ctrl+Alt+Tab`       | Toggle overview (zoom out)              |
| `Ctrl+Alt+F`         | Toggle fullscreen                       |
| `Ctrl+Alt+T`         | Toggle floating                         |
| `Ctrl+Alt+Q`         | Close the focused window                |
| `Ctrl+Alt+Return`    | Open `cmd.exe`                          |
| `Ctrl+Alt+Shift+Q`   | Quit wiri                               |
| `Alt + Left-drag`    | Interactive move                        |
| `Alt + Right-drag`   | Interactive resize at the window edge   |

## 5. Edit the config

wiri looks for `config.kdl` in this order:

1. `$WIRI_CONFIG` (explicit override)
2. `%APPDATA%\wiri\config.kdl`            ← canonical Windows location
3. `%USERPROFILE%\.config\wiri\config.kdl` ← XDG-style fallback

Copy the bundled `resources/default_config.kdl` to that location:

```powershell
mkdir $env:APPDATA\wiri -ErrorAction SilentlyContinue
copy resources\default_config.kdl $env:APPDATA\wiri\config.kdl
notepad $env:APPDATA\wiri\config.kdl
```

The config is hot-reloaded on save (~100 ms debounce).  Or right-click
the tray icon → *Reload Config*.

## 6. Drive it from scripts

`wiri-ctl` talks to the running daemon over a named pipe and returns
human-friendly output by default — pass `--json` for raw output suitable
for scripting.

```powershell
wiri-ctl state                          # summary block
wiri-ctl windows                        # aligned columns
wiri-ctl switch-workspace 3
wiri-ctl spawn 'wt.exe'
wiri-ctl set-column-width 1/2
wiri-ctl events --types '*'             # tail the event stream
wiri-ctl windows --json | jq '.result'  # raw JSON
```

## 7. Autostart with Windows

```powershell
wiri-ctl --help          # see all commands
# wiri itself can register/unregister itself in the user Run key:
.\target\release\wiri.exe --register-startup
```

The startup helper writes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
and never touches HKLM.

## Where to next

* [`docs/SPEC.md`](SPEC.md) — full architecture + module overview
* [`docs/COMPARISON.md`](COMPARISON.md) — wiri vs niri vs komorebi vs FancyZones
* [`docs/modules/INPUT.md`](modules/INPUT.md) — hotkey action names
* [`docs/modules/CONFIG.md`](modules/CONFIG.md) — KDL syntax reference
* [`docs/modules/IPC.md`](modules/IPC.md) — wiri-ctl protocol
