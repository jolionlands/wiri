# wiri vs Other Windows Tiling Managers

A quick orientation table for newcomers deciding between the major tiling
options on Windows.  All four products are good — they target different
philosophies.

| Feature                       | **wiri**                       | **niri** (Linux)             | **komorebi**                   | **FancyZones**           |
| ----------------------------- | ------------------------------ | ---------------------------- | ------------------------------ | ------------------------ |
| Platform                      | Windows 10 / 11 (x86_64, ARM64)| Linux (Wayland)              | Windows 10 / 11                | Windows 10 / 11          |
| Tiling model                  | **Scrollable columns** (niri-style) | Scrollable columns      | Manual/auto BSP, columns       | User-drawn fixed zones   |
| Resizes on new window         | **No** (strip extends right)   | No                            | Yes (split / push)             | No (drop into zone)      |
| Per-monitor workspaces        | **Yes** (dynamic)              | Yes                          | Yes (named or numbered)        | No (single virtual)      |
| Floating-window override      | **Yes** (per-window toggle)    | Yes                          | Yes                            | Yes (default)            |
| Hot-reload config             | **Yes** (~100 ms debounce)     | Yes                          | Yes                            | Limited                  |
| Config format                 | **KDL**                        | KDL                          | YAML / JSONC                   | GUI                      |
| Scripting / IPC               | **Named pipe + `wiri-ctl`**    | Unix socket + `niri msg`     | Named pipe + `komorebic`       | None                     |
| Window rules (regex match)    | **Yes**                        | Yes                          | Yes                            | Limited                  |
| Animations                    | Opt-in, 6 easings              | Yes                          | Optional via komorebi-bar      | No                       |
| Native frame stripping        | **Optional per-window**        | (Wayland — N/A)              | Yes                            | No                       |
| Multi-DPI aware               | **Yes** (per-monitor)          | Yes                          | Yes                            | Yes                      |
| System tray                   | **Yes** (menu + reload)        | n/a                          | Yes (via komorebi-tray)        | Settings app             |
| External daemon vs in-shell   | Daemon (`wiri.exe`)            | Compositor                   | Daemon (`komorebi.exe`) + WHKD | Shell extension          |
| Lines of code                 | ~9 000 Rust                    | ~30 000 Rust                 | ~25 000 Rust                   | (C++ closed source)      |
| License                       | MIT / Apache-2.0 (planned)     | GPL-3.0                      | MIT                            | MIT (PowerToys)          |

## Pick wiri if you…

* Want niri's **scrollable-tiling** model on Windows — no window ever shrinks
  to make room for another.
* Like KDL configs and live reload.
* Want a small, hackable daemon (one binary, one CLI).
* Run on ARM64 / Snapdragon X — wiri is tested on `aarch64-pc-windows-msvc`.

## Pick komorebi if you…

* Prefer classic BSP / tall-stack / columns layouts (i3 / bspwm style).
* Want a more mature feature set (status bar, focus stealing prevention,
  application identifier database, etc.).
* Need to integrate with WHKD / AutoHotkey for hotkeys.

## Pick FancyZones if you…

* Want zero-config tiling — just drag windows into a zone you draw.
* Don't want a separate daemon running.
* Already use Microsoft PowerToys for other utilities.

## Pick niri if you…

* You're on Linux — wiri ports niri's *idea* to Windows; if you can run
  Wayland, niri itself is more featureful.
