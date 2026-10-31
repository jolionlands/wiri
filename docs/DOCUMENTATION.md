# wiri — Comprehensive Documentation

> A scrollable-tiling window manager for Windows, inspired by [niri](https://github.com/YaLTeR/niri).
> Written in Rust. ~8,400 lines across 32 source files.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Architecture](#2-architecture)
3. [Installation & Usage](#3-installation--usage)
4. [Configuration Guide](#4-configuration-guide)
5. [Key Bindings](#5-key-bindings)
6. [IPC Protocol](#6-ipc-protocol)
7. [Module Reference](#7-module-reference)
8. [Data Structures](#8-data-structures)
9. [Event Flow](#9-event-flow)
10. [Developer Guide](#10-developer-guide)
11. [Testing](#11-testing)

---

## 1. Overview

**wiri** is a native Windows window manager that arranges windows in **columns on an infinite horizontal strip** — opening new windows never causes existing windows to resize. It brings the scrollable-tiling paradigm from the Linux world (niri) to Windows.

### Key Characteristics

| Feature | Description |
|---|---|
| **Scrollable Tiling** | Windows arranged in columns extending infinitely to the right |
| **Dynamic Workspaces** | Per-monitor workspaces arranged vertically (like GNOME) |
| **Multi-Monitor** | Each monitor has its own independent window strip; workspaces preserved across reconnects |
| **Floating Windows** | Toggle any window to/from floating mode |
| **Configurable** | KDL configuration file with hot-reload support |
| **Windows 11 Focus** | Uses `DWMWA_BORDER_COLOR` for colored window borders on Win11+ |
| **IPC** | Named pipe control interface (`\\.\pipe\wiri_control_<user>_<session>`) |
| **Animation Framework** | Tick-based interpolation with 6 easing functions |
| **Interactive Mouse** | Alt+drag for move/resize via `WH_MOUSE_LL` global hook |

### Project Status

- ✅ Compiles with zero warnings/errors
- ✅ 59 unit tests passing
- ✅ Runs on `aarch64-pc-windows-msvc` (ARM64)
- ✅ 22 default hotkeys (Ctrl+Alt prefix)
- ✅ Full tiling lifecycle (add, remove, focus, move, scroll, fullscreen, float)
- ✅ Interactive move/resize via mouse (Alt+Left/Right drag)
- ✅ DWM border color focus rendering (Windows 11+)
- ✅ Configurable keybindings via KDL `binds {}` section
- ✅ KDL configuration parser with `key=value` and `key "value"` syntax
- ✅ File watcher for live config reload
- ✅ Tray icon with `Shell_NotifyIconW`
- ✅ Animation framework (framework complete, not yet driving layout ticks)
- ✅ Multi-DPI aware (scale factors stored, not fully applied to pixel calculations yet)

---

## 2. Architecture

### High-Level Flow

```
┌─────────────────────────────────────────────────────────────┐
│                      wiri (main.rs)                         │
│                                                              │
│  Config → LayoutConfig → TilingEngine ──→ apply_layout()    │
│     ↑                                      │                 │
│     │                                      ▼                 │
│  Backend (Win32) ←── WinEventHook ─── BackendEvent          │
│     ↑                                      │                 │
│     │                                      ▼                 │
│  MessageLoop (hotkeys) ──→ Action → TilingEngine            │
│                                                              │
│  MouseHook (WH_MOUSE_LL) ──→ GrabState → Move/Resize        │
│                                                              │
│  IPC Server ←── Named Pipe ──→ wiri-ctl                     │
│                                                              │
│  TrayIcon ←── Context Menu ──→ Reload/Quit                  │
└─────────────────────────────────────────────────────────────┘
```

### Module Dependency Graph

```
main.rs
  ├── backend/ ──────────── sends BackendEvent to event handler
  ├── layout/
  │   ├── engine.rs ─────── core tiling logic
  │   ├── workspace.rs ──── Workspace, Column, Tile
  │   ├── mod.rs ────────── Monitor struct
  │   ├── floating.rs ───── FloatManager
  │   ├── borders.rs ────── FocusRenderer
  │   └── animation.rs ──── Animation framework
  ├── window/
  │   ├── mod.rs ────────── Mapped/Unmapped/WindowRef/WindowSet
  │   └── rules.rs ──────── rule resolution
  ├── input/
  │   ├── mod.rs ────────── Action enum, parsers
  │   ├── hotkey.rs ─────── RegisterHotKey management
  │   ├── grab.rs ───────── MoveGrab, ResizeGrab
  │   ├── mouse.rs ──────── MouseTracker
  │   └── low_level_hook.rs ── WH_MOUSE_LL global hook
  ├── config/
  │   ├── types.rs ──────── Config structs
  │   ├── parse.rs ──────── KDL parser
  │   ├── loader.rs ──────── file I/O, file watcher
  │   └── error.rs ──────── error types
  ├── hooks/
  │   ├── mod.rs ────────── SystemIntegration
  │   ├── tray.rs ───────── tray icon
  │   ├── spawner.rs ────── process spawner
  │   └── startup.rs ────── auto-start
  ├── ipc/
  │   └── mod.rs ────────── named pipe server, message types
  ├── utils/
  │   ├── id.rs ─────────── OutputId, WindowId
  │   └── rect.rs ───────── Point, Size, Rect
  └── bin/ctl.rs ────────── CLI client binary
```

### Threading Model

wiri uses several threads:

1. **Main (Tokio async) thread** — Config loading, engine initialization, event handling, IPC server, mouse focus tracking, periodic ticks
2. **Message loop thread** — Windows message pump (`GetMessageW`), hotkey dispatch (`WM_HOTKEY`), key repeat debouncing
3. **WinEvent hook thread** — Receives window event callbacks from `SetWinEventHook`
4. **Config watcher thread** — `notify` file watcher for config reload
5. **Mouse hook thread** — Low-level `WH_MOUSE_LL` global mouse hook

Shared mutable state is protected by `Arc<RwLock<T>>`:
- `TilingEngine` — wrapped in `Arc<RwLock<>>`
- `FocusRenderer` — wrapped in `Arc<RwLock<>>`
- `BackendHandle` — fully cloneable, thread-safe via internal `Arc<RwLock<>>`

---

## 3. Installation & Usage

### Requirements

- Windows 10 1809+ or Windows 11
- Rust toolchain (for building)

### Building

```sh
git clone <repo>
cd WM/wiri
cargo build --release
```

The build produces two binaries:
- `target/release/wiri.exe` — the window manager (2.4 MB)
- `target/release/wiri-ctl.exe` — IPC control CLI (710 KB)

### Running

```sh
wiri                            # with default config
wiri -c my-config.kdl           # with custom config
wiri -v                         # verbose logging (debug level)
wiri --no-tray                  # disable system tray icon
```

### Hot-reload

Edit `config.kdl` while wiri is running and changes take effect automatically via the file watcher, about 200 ms after the last change to the file or to a file it includes; configured hotkeys also reload without restarting.  A mistake in the file is reported (tray balloon, log, `config_loaded` event) and the last good config stays.  See [Errors](#errors).

### Logging

`wiri.exe` is a GUI-subsystem program (it opens no console window), so its log goes to
`%LOCALAPPDATA%\wiri\wiri.log`, rotated at 4 MB (the previous log is kept as `wiri.log.1`).
Started from a shell with `-v` (or `--help`, `--version`) wiri attaches to that shell's console and
shows the log there as well; `wiri -v 2> out.log` redirects it instead.  A plain `wiri` never attaches
to a console, because a daemon attached to a console dies when the console is closed.
Use the `RUST_LOG` env var for fine-grained control:
```sh
RUST_LOG=debug wiri
RUST_LOG=wiri::layout=trace wiri
```

---

## 4. Configuration Guide

wiri's config is a **KDL** file, the syntax niri uses.  A niri `binds {}` block, `window-rule`s,
`include` and `spawn-at-startup` lines paste in as they are.  The default location is
`%APPDATA%\wiri\config.kdl` (wiri writes `resources/default_config.kdl` there on its first start);
`-c <path>` and `$WIRI_CONFIG` override it.  The shipped `resources/default_config.kdl` and
`examples/` show every option wiri reads.

### The syntax

```kdl
// a comment            /* a block comment, /* nested */ */
/-binds { ... }         // slashdash comments out the next node, argument, property or block
layout {                // a section: nodes in braces, one per line or separated by ;
    gaps 16             // a node with an argument
    focus-follows-mouse // a flag: written means on, `false` turns it off
}
output "DP-1" {         // a section with a name
    position x=1280 y=0 // properties are key=value
}
window-rule {
    match app-id=r#"firefox$"#   // r#"..."# is a raw string: no escapes
}
```

Strings are in double quotes with KDL's escapes (`\n \t \\ \" \u{1F600}`) or raw (`r"..."`, `r#"..."#`).
A bad escape is an error that says how to fix it.  For Windows paths, wiri keeps the old format's
convenience: **a string that starts like a path (`C:\...` or `\\server`) keeps its backslashes as
written** (`"C:\Program Files\x"` works); in any other string write `\\` or use a raw string.  A
string that starts with three or more backslashes is KDL's escaped spelling of a UNC or device name,
so `"\\\\.\\DISPLAY1"` and `"\\.\DISPLAY1"` both mean the monitor `\\.\DISPLAY1`.  Numbers
may use `_`, `0x`, `0o`, `0b`.  A byte-order mark, CRLF and UTF-16 files (what PowerShell's `Out-File`
writes) are fine.  wiri also still reads its older spellings: bare words as values (`hide-method hide`),
`key=value` as a whole line (`repeat_delay=600`), and the flat keys of `layout`.

### Errors

A config with a mistake does not load; niri does the same.

* **Errors** reject the file: a syntax error, an unknown section or key (with a "did you mean"), a
  value of the wrong type or out of range, a bad color, an unknown bind action, a bad regular
  expression, a missing `include`.  Each one names `file:line:column`.
* **Warnings** load the file and say what is ignored: a niri option wiri does not have (`touchpad {}`,
  `animations { workspace-switch {...} }`, `prefer-no-csd`), a bind that cannot work on Windows (an AltGr
  modifier, a mouse or media-key trigger, a niri action wiri lacks), a duplicate bind.
* At startup an erroneous config is not used: wiri starts with its defaults and shows a tray balloon
  with the first error.  On a **reload** the last good config stays in use, a balloon names the file
  and line, the log has every error, and the IPC event stream gets `config_loaded` with `failed: true`.
* `wiri-ctl validate-config [path]` runs the same parser without starting anything (exit code 1 when
  there are errors); `wiri-ctl reload-config` answers with the first error when the file is bad.
* The file is re-read about 200 ms after the last change to it or to a file it includes, so an editor
  that saves in steps never gets half a file applied.

### `include`

`include "other.kdl"` is read where it stands; the path is relative to the including file, absolute,
`~/...` or starts with an environment variable (`%USERPROFILE%\...`).  `include optional=true "x.kdl"`
only warns when the file is missing.  A later `binds {}` replaces earlier binds on the same keys.  The
depth is limited to 8, a file that includes itself is an error, and the included files are watched.

### `input {}`

| Option | Meaning |
|---|---|
| `mod-key "ctrl-alt"` | what `Mod` means in binds: `ctrl-alt` (default), `alt`, `ctrl`, `super`, `win-alt`, `ctrl-shift` |
| `keyboard { repeat-delay N; repeat-rate N; numlock }` | `repeat-rate` also paces held-key binds |
| `mouse { natural-scroll; scroll-speed N; acceleration N; sensitivity N; tap-to-click }` | wiri's own wheel binds; `scroll-factor N` is niri's name for `scroll-speed` |
| `focus-follows-mouse` / `warp-mouse-to-focus` / `focus-delay-ms N` | flags (or `true`/`false`) |
| `workspace-auto-back-and-forth` | asking for the workspace you are on goes back to the previous one |
| `touch { enable; tap-to-click; scroll-speed N }` | |

niri's `touchpad`, `trackpoint`, `trackball`, `tablet`, `xkb`, `track-layout`, `mod-key-nested` and
`disable-power-key-handling` are accepted with a warning: Windows has one pointer setup for all devices.

### `cursor {}`

| Option | Meaning |
|---|---|
| `hide-when-typing` | set Windows' own per-user "hide pointer while typing" (`SPI_SETMOUSEVANISH`); only written when the option is there, the previous value is restored on quit or when a reload drops it |
| `hide-after-inactive-ms N` | hide the pointer after `N` ms without pointer movement; any movement or click shows it again. One one-shot timer runs on the existing mouse hook and only while the pointer is visible. The user's cursors are restored on quit, on a reload that removes the option, from the panic hook and at startup |

`xcursor-theme` and `xcursor-size` are accepted with a warning: the cursor scheme belongs to Windows
(Settings > Accessibility > Mouse pointer).

### `layout {}`

| Option | Meaning |
|---|---|
| `gaps N` | niri's one number: between windows and around the screen. `gaps { inner N; outer N }` tells them apart (default 16 and 8) |
| `struts { left N; right N; top N; bottom N }` | room kept free for a bar, in pixels (negative values count as 0) |
| `border { width N; active-color "#rrggbb"; inactive-color; urgent-color; off; radius N }` | `width 0` or `off` turns it off; `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `accent` |
| `focus-ring { width N; active-color; inactive-color; gap N; off }` | |
| `shadow { on; softness N; spread N; offset x= y=; color }` | DWM draws the shadow |
| `column-width N`, `column-width-mode "proportional"` or `"fixed"`, `max-visible-columns N`, `scroll-step N` | wiri's column sizing |
| `dim-unfocused 0.92`, `strip-frame`, `snap-on-drag`, `snap-threshold-px N`, `smart-borders`, `status-bar` | |
| `hide-method "hide"` or `"cloak"` | how the windows of other workspaces are hidden |
| `unified-grid` / `unified-grid true` / `unified-grid false` | **wiri extension.** One horizontal camera shared by every workspace row of a monitor, so the desktop is one XY grid and vertical moves land on the column under the camera. Off by default; also accepted in a per-output `layout {}` override. See `docs/design/unified-grid.md` |
| `unified-grid-chord-ms N` | **wiri extension.** Integer `0..=1000`, default 150: how long two arrow presses count as one diagonal chord, measured from the input events' own timestamps. `0` disables merging |

niri's `center-focused-column`, `preset-column-widths`, `default-column-width`, `tab-indicator`,
`insert-hint`, gradients and the rest are accepted with a warning.  The older flat keys
(`inner-gaps`, `border-width`, `border-color-focused`, `focus-ring-width`, `shadow-enable`, ...) and
the nested `gaps { }`, `borders { }`, `shadows { }` blocks still work.

### `output "name" {}`

`off`, `scale N`, `transform "90"`, `position x= y=` (or `x N` / `y N`), `mode "1920x1080@60"`,
`variable-refresh-rate`, `focus-at-startup`, and a `layout { }` block that overrides the layout for
that monitor.  The name is the Windows device name (`wiri-ctl list-monitors`).

### `workspace "name" {}`

Named workspaces exist from the start, in the order declared, even when empty.  `open-on-output "name"`
and wiri's `monitor`, `layout`, `follow-on-focus` are read.

### `binds {}`

```kdl
binds {
    Mod+T hotkey-overlay-title="Open a Terminal" { spawn "wt.exe"; }
    Mod+Left { focus-column-left; }
    Mod+1 { focus-workspace 1; }                  // an index
    Mod+Shift+W { focus-workspace "web"; }        // a name (a quoted word)
    Mod+R repeat=false cooldown-ms=300 { set-column-width "+10%"; }
    /-Mod+X { quit; }                             // commented out
}
```

A bind is the key combination (`Mod`, `Ctrl`, `Shift`, `Alt`, `Super`/`Win`, then a key name:
xkb names such as `Bracket_Left`, `Page_Up`, `Return`, `Grave`, or letters, digits, `F1`...`F24`) and
exactly one action in braces.  Properties: `repeat=false` (holding the key fires the action once),
`cooldown-ms=` (ignore the bind for that long after it fired), `hotkey-overlay-title=`
(Pango markup, as in niri: `"<b>Browser</b>"`, `"R&amp;D"`, `<span foreground="#ff8800">`; an
invalid title is shown as the whole original string and warned about once at load; `null` hides the
row), `allow-when-locked=` and `allow-inhibiting=` (accepted: Windows never
delivers hotkeys to a locked session, and wiri has no shortcut inhibition).  Action properties of
niri (`focus=false`, `show-pointer=false`, `skip-confirmation=true`) are accepted.

The older line form still works: `Ctrl+Alt+Left focus-column-left`, `Ctrl+Alt+1 switch-workspace 1`,
`Mod+Shift+T spawn "wt.exe"`.  Like niri, a `binds {}` section replaces the built-in defaults: an omitted or empty block binds nothing.
The wiri extension `extend-defaults` inside `binds {}` (the generated config has it) adds the built-in
defaults: a bind in the config wins when it uses the same keys as a default, the other defaults stay.
The same chord twice in one block is an error naming both lines; a bind in an included file overrides
the same chord of the including file.

### `window-rule {}`

```kdl
window-rule {
    match app-id=r#"^firefox"# title="^Picture-in-Picture$"   // one line: all properties must match
    match app-id="^mpv"                                      // another line: either line may match
    exclude title="Private"                                  // none of the exclude lines may match
    open-floating true
    open-on-workspace "chat"
    opacity 0.9
}
```

**Matching.**  A window matches a rule when it matches any `match` line (a rule without one matches
every window) and none of the `exclude` lines; one line is the AND of its properties.  `app-id` is a
regular expression searched in the program's file name (`firefox.exe`, and `firefox` without the
extension) and, for Store apps and programs that name themselves, in the AppUserModelID
(`Microsoft.WindowsTerminal_8wekyb3d8bbwe!App`); `title` is searched in the window title; `class-name` in
the window class.  `is-active`, `is-focused`, `is-active-in-column`, `is-floating`, `is-urgent` and
`at-startup` take `true` or `false` (`at-startup` is true for the windows that were already open when
wiri started).  Regular expressions are Rust's, case sensitive like niri's (`(?i)` turns that off).
When several rules match, a later rule wins for each property it sets.

**What the properties do.**

| Property | Effect |
|---|---|
| `open-on-output "\\.\DISPLAY1"` | the window opens on that monitor (`DISPLAY1` also works); a name no monitor has is ignored.  `wiri-ctl list-monitors` shows the names |
| `open-on-workspace "chat"` | opens on the named workspace (`workspace "chat"` in the config; a name no workspace has is ignored).  When it is not the workspace you are on, the window is hidden there and your focus stays where it is |
| `open-in-workspace 2` | wiri's variant: the workspace at that position from the top (the last one for a number past the end).  The older `workspace "2"` / `workspace "web"` key means the same, a number or a name |
| `open-floating true` / `false` | opens floating, centred; `false` takes back a `float true` of an earlier rule |
| `open-fullscreen true` | opens fullscreen |
| `open-maximized true` | the column opens as wide as the view |
| `open-maximized-to-edges true` | opens maximized to the edges of the work area, keeping its column (see "Fullscreen and maximize") |
| `open-focused false` | the window that has the focus keeps it: when Windows hands the new window the focus, wiri gives it back (within 2.5 s of the window opening) |
| `default-column-width { proportion 0.5; }` or `{ fixed 800; }` or `{}` | the width of the new column; a fixed size is the window's, the border comes on top; `{}` keeps the width the window opened with.  A floating window opens at that width |
| `default-window-height { fixed 400; }` | the height of the tile in a column of several windows (a column of one always fills the view); a floating window opens at that height |
| `min-width`, `max-width`, `min-height`, `max-height` | limits in pixels of the window; the column / tile is held to them, the minimum winning when they clash |
| `opacity 0.8` | the window is drawn with that alpha (together with `dim-unfocused`) |
| wiri's `float`, `blur`, `sticky`, `default-width`, `default-height` | as before (`default-width 800` is the width of the column in pixels) |

The `open-*` and `default-*` properties apply once, when the window opens.  `opacity` and the size
limits follow the window: they are worked out again when its **title** changes, when the **focus**
moves (`is-focused`, `is-active`), when the window **floats** or tiles again (`is-floating`), when it
starts or stops flashing in the taskbar (`is-urgent`), and when the config is reloaded.  Only the
windows concerned are looked at, and only when some rule depends on that kind of state.

The older forms work too: `class-name "x"` directly in the rule and
`match { class-name "x"; process-name-regex "..."; is-active }` blocks.

### `animations {}`

`off` / `on`, `duration N`, `easing "ease-out-cubic"`; niri's per-animation blocks and `slowdown` are
accepted with a warning.

### `overview {}`

```kdl
overview {
    zoom 0.5                      // 0 to 1; lower values make the workspaces smaller
    backdrop-color "#262626"      // behind the workspaces
    workspace-shadow {            // off; or softness / spread / offset / color
        softness 40
        spread 10
        offset x=0 y=10
        color "#00000050"
    }
}
```

The overview is a camera over the real workspaces, drawn by a session-local window of live DWM
previews; applications keep their size. The opening and closing spring is `animations {
overview-open-close { ... } }` (it follows `off` and `slowdown`). The host is opaque (previews of
other applications need that), so an alpha in `backdrop-color` or the shadow colour blends with
black, not with your wallpaper. In the overview: click a window to focus it, drag it onto a
workspace or into the gap above, between or below workspaces, use the wheel to change workspace,
drag with the right button to scroll the view; with a touchscreen the primary finger pans and
resting on a window picks it up. All normal binds keep working.

### `gestures {}`

`hot-corners { top-left; top-right; bottom-left; bottom-right; off }` opens the overview when the
pointer rests in a screen corner for about 120 ms (once per visit). With no name the top-left
corner is on; naming a corner replaces that default; `off` switches them all off. An `output`
block may carry its own `hot-corners {}`, which replaces the global one for that monitor.
`dnd-edge-view-scroll` and `dnd-edge-workspace-switch` configure the drag-and-drop edge actions; an
empty `gestures {}` or either of those blocks turns the drag-capture approximation on, a block
holding only `hot-corners` does not.

With `input { touchpad { } }` (a Precision Touchpad; set Windows' own three- and four-finger
Swipes to Nothing first) three fingers move the view or switch workspace and a four-finger swipe up
or down opens or closes the overview.

### Programs to start

`spawn-at-startup "prog" "arg" ...` (every argument its own string) and `spawn-sh-at-startup "command"`.

---

## 5. Key Bindings

### Default Key Bindings

All default bindings use the `Ctrl+Alt` prefix:

| Key | Action | Description |
|---|---|---|
| ← / → | `focus-column-left` / `focus-column-right` | Focus adjacent column |
| ↑ / ↓ | `focus-up` / `focus-down` | Focus tile above/below in current column |
| Shift+← / Shift+→ | `move-column-left` / `move-column-right` | Move entire column left/right |
| Q | `close-window` | Close focused window |
| Enter | `spawn cmd.exe` | Open terminal |
| F | `toggle-fullscreen` | Toggle focused window fullscreen |
| T | `toggle-floating` | Toggle focused window floating |
| H | `scroll-left` | Scroll workspace left |
| L | `scroll-right` | Scroll workspace right |
| Space | `overview` | Toggle overview (zoom out) mode |
| ` (Grave) | `focus-previous` | Focus the window you were on before (niri's `Mod+Tab`; with the Ctrl+Alt prefix that chord is Windows' task switcher) |
| O | `overview-select` | Select window and exit overview |
| Shift+Q | `quit` | Exit wiri |
| 1–9 | `switch-workspace 1`–`9` | Switch to workspace |
| , / . | `consume-window-into-column` / `expel-window-from-column` | niri: the first window of the column on the right moves under the focused window / the bottom window of the focused column gets a column of its own on the right |
| Shift+[ / Shift+] | `consume-or-expel-window-left` / `consume-or-expel-window-right` | niri: a window alone in its column joins the column on that side (at the bottom); one of several leaves its column on that side |
| K / J | `move-window-up` / `move-window-down` | niri: swap the window with the one above / below in its column |
| Home / End | `focus-column-first` / `focus-column-last` | Focus the first / last column |
| Shift+Home / Shift+End | `move-column-to-first` / `move-column-to-last` | Move the focused column to the start / end of the strip |
| Shift+F | `maximize-column` | niri: the column is as wide as the view (toggle, also for a column of one window) |
| M | `maximize-window-to-edges` | niri: the window fills the work area, no gap, no border, and keeps its column (toggle) |

### Mouse Bindings

| Action | Description |
|---|---|
| **Alt + Left-click drag** | Interactive move — drag a tiled window to reorder columns |
| **Alt + Right-click drag (edge)** | Interactive resize — drag window edges to resize |

### Supported Key Names

| Category | Keys |
|---|---|
| **Letters** | `a`–`z` (case-insensitive) |
| **Digits** | `0`–`9` |
| **Arrows** | `Left`, `Right`, `Up`, `Down` |
| **Arrow aliases** | `Arrow-Left`, `Arrow-Right`, `Arrow-Up`, `Arrow-Down` |
| **Special** | `Enter`, `Return`, `Space`, `Tab`, `Escape`, `Esc`, `Backspace`, `Delete`, `Del`, `Home`, `End`, `Page-Up` (also `Page_Up`, `PageUp`), `Prior`, `Page-Down` (also `Page_Down`, `PageDown`), `Next`, `Insert`, `Ins` |
| **Lock** | `Caps-Lock`, `Num-Lock`, `Scroll-Lock` |
| **Function** | `F1`–`F24` |
| **Other** | `Print`, `Print-Screen`, `Pause` |
| **Punctuation** | `Comma`, `Period`, `Minus`, `Equal`, `Semicolon`, `Slash`, `Backslash`, `Bracket_Left`, `Bracket_Right`, `Apostrophe`, `Grave` (xkb names), or the character itself: `,` `.` `-` `=` `;` `/` `\` `[` `]` `'` |
| **Keypad** | `KP_0`–`KP_9`, `KP_Add`, `KP_Subtract`, `KP_Multiply`, `KP_Divide`, `KP_Decimal`, `KP_Enter` |

Names are matched case-insensitively and `_`, `-` and spaces are ignored, so `Bracket_Left`,
`bracketleft` and `bracket-left` are the same key.

### Supported Action Names

| Action Name | Aliases | Params | Description |
|---|---|---|---|
| `focus-column-left` | `focus-left` | — | Focus column left |
| `focus-column-right` | `focus-right` | — | Focus column right |
| `focus-up` | `focus-window-up` | — | Focus tile above |
| `focus-down` | `focus-window-down` | — | Focus tile below |
| `focus-column-first` | — | — | Focus the first column |
| `focus-column-last` | — | — | Focus the last column |
| `move-column-left` | `move-left` | — | Move column left |
| `move-column-right` | `move-right` | — | Move column right |
| `move-column-to-first` | — | — | Move the column to the start of the strip |
| `move-column-to-last` | — | — | Move the column to the end of the strip |
| `move-window-up` | — | — | Swap the window with the one above it in its column (a floating window moves by 50 px) |
| `move-window-down` | — | — | Swap the window with the one below it in its column |
| `consume-window-into-column` | `consume-window`, `consume-into-column` | — | The first window of the column on the right moves to the bottom of the focused column |
| `expel-window-from-column` | `expel-window`, `expel-from-column` | — | The bottom window of the focused column becomes a column of its own on its right |
| `consume-or-expel-window-left` | — | — | A window alone in its column joins the column on its left; one of several leaves its column to the left |
| `consume-or-expel-window-right` | — | — | The same to the right |
| `maximize-column` | `column-maximize`, `toggle-column-maximize` | — | Toggle: the column is as wide as the view (also for a column of one window); toggling again restores its width |
| `set-column-width` | `set-window-width` | `"50%"`, `"+10%"`, `"-10%"`, `"1200"`, `"+100"` | niri's size argument (below); wiri's `"1/3"`, `"2/3"`, `"full"` and `"cycle"` work too. A floating window is resized |
| `set-window-height` | — | the same | The window's height in its column (the others keep their proportions); a floating window is resized. A column of one window always fills the view |
| `reset-window-height` | — | — | The window gets an equal share of its column again |
| `switch-preset-column-width` | `column-width-preset`, `column-width-cycle` | — | niri: the column takes the next width of a third, a half, two thirds of the view (the first one wider than it is; it goes on from there when it was switched last) |
| `switch-preset-column-width-back` | — | — | The same backwards |
| `expand-column-to-available-width` | `expand-column-to-available`, `expand-column` | — | niri: the column takes the room that the other columns fully in view leave |
| `maximize-window-to-edges` | — | — | niri 25.11: the window fills the work area of its monitor (no gap, no border) and keeps its place in its column; toggles (see "Fullscreen and maximize") |
| `close-window` | `close` | — | Close focused window |
| `toggle-fullscreen` | `fullscreen`, `fullscreen-window` | — | Toggle fullscreen: the window covers the monitor and the taskbar goes; it keeps its column (see "Fullscreen and maximize") |
| `toggle-floating` | `float`, `toggle-window-floating` | — | Toggle floating |
| `move-window-to-floating` | — | — | Float the window (nothing happens when it floats already) |
| `move-window-to-tiling` | — | — | Put a floating window back into a column (nothing happens when it is tiled) |
| `scroll-left` | — | — | Scroll workspace left |
| `scroll-right` | — | — | Scroll workspace right |
| `quit` | `exit` | — | Exit wiri |
| `switch-workspace` | `workspace` | `n` | Focus the `n`-th workspace from the top of the monitor (`Ctrl+Alt+1`–`9`); a number past the last workspace is the last one |
| `focus-workspace-down` | `focus-workspace-next`, `workspace-next` | — | The workspace below (no wrapping) |
| `focus-workspace-up` | `focus-workspace-prev`, `workspace-prev` | — | The workspace above (no wrapping) |
| `focus-workspace-previous` | `focus-workspace-back` | — | niri: the workspace you were on before this one |
| `focus-workspace` | — | `N` or `"name"` | niri: a number is the index of the workspace on the monitor, a quoted string is the name of a workspace (`focus-workspace 2` is the second workspace, `focus-workspace "2"` the one named 2) |
| `move-workspace` | — | `n` | Move window to workspace `n` |
| `spawn` | `exec` | `"cmd"` | Launch a program |
| `maximize` | — | — | wiri's older name: maximize the window to the edges (it is `maximize-window-to-edges` that only sets, not toggles) |
| `minimize` | — | — | Minimize window |
| `center-window` | — | — | Center window |
| `switch-monitor` | — | — | Focus next monitor |
| `focus-monitor-left` | — | — | niri: focus the monitor on the left (nothing at the edge) |
| `focus-monitor-right` | — | — | niri: focus the monitor on the right |
| `move-to-monitor-left` | `move-window-to-monitor-left` | — | Move the window to the monitor on the left |
| `move-to-monitor-right` | `move-window-to-monitor-right` | — | Move the window to the monitor on the right |
| `move-column-to-monitor-left` | — | — | Move the column to the monitor on the left |
| `move-column-to-monitor-right` | — | — | Move the column to the monitor on the right |
| `refresh` | — | — | Re-apply layout |
| `column-toggle-tabbed` | `toggle-column-tabbed-display` | — | Switch the focused column between normal and tabbed display |
| `set-column-display` | — | `"normal"` or `"tabbed"` | niri: set the focused column's display; choosing the current mode does nothing (the selected tab and the focus stay). `wiri-ctl set-column-display tabbed` and the typed `{"Action":{"SetColumnDisplay":{"display":"Tabbed"}}}` do the same |
| `overview` | `overview-toggle`, `zoom-out`, `toggle-overview` | — | Enter/exit overview |
| `open-overview` | — | — | Enter overview (nothing happens when it is open) |
| `overview-exit` | `exit-overview`, `close-overview` | — | Leave overview (nothing happens when it is closed or already closing) |
| `overview-left` | — | — | Navigate left in overview |
| `overview-right` | — | — | Navigate right in overview |
| `overview-select` | `overview-accept` | — | Select window, exit overview |

---

### Fullscreen and maximize

A fullscreen or maximized window stays a participant of the strip, like in niri:

* **Its column stays.**  The window keeps its column and its place in it; the column is made as wide
  as the view while the window is fullscreen or maximized (the state `maximize-column` toggles) and gets
  its old width back afterwards.  A column that was maximized with `maximize-column` before stays
  maximized.
* **It is drawn over the monitor or the work area** instead of its tile: `fullscreen-window` covers the
  whole monitor (the frame is taken away, the window is placed exactly, the shell is told with
  `ITaskbarList2::MarkFullscreenWindow` so the taskbar goes while it is the foreground window and comes
  back afterwards); `maximize-window-to-edges` fills the work area (the taskbar stays) with no gap and no
  border.  The window is drawn where its slot is: focusing another column scrolls the strip by the least
  that shows it (as in niri), the fullscreen window moves with its slot (it stays as wide as the monitor,
  beside the neighbour, and goes off screen as the strip scrolls on) and covers the monitor again when it is
  focused again.
* **Nothing else is hidden.**  The other windows are not hidden, they stay in the taskbar and in
  Alt+Tab; they are out of the way (a view away, parked like every column that is out of view).
* Fullscreen wins over maximized; leaving fullscreen goes back to maximized when the window was.  A
  floating window can be fullscreen (it covers the monitor and gets its rectangle back) but not maximized;
  floating a fullscreen or maximized tile ends both.  A minimized window comes back as it was.
* **Applications are followed, not fought.**  A window that covers its monitor on its own (a video player,
  F11 in a browser, a game that goes borderless) is fullscreen for wiri too: its frame is not touched, the
  taskbar goes, and when it shrinks again wiri tiles it.  When the maximize button of a title bar is pressed
  (or the caption double-clicked), wiri takes the native maximized state away at once and keeps the window
  maximized to the edges instead; the button again gives the window its tile back.  A window that opens
  already maximized or as big as its monitor is treated the same.
* The window rules `open-fullscreen`, `open-maximized` (the column as wide as the view) and
  `open-maximized-to-edges` do the same when a window opens.  `wiri-ctl window-list` reports
  `fullscreen` and `maximized` for every window.

### Workspaces

Workspaces are addressed by position, like in niri: `focus-workspace 2` is the second workspace from
the top of the focused monitor, whichever workspace that is.  There is always one empty workspace at
the bottom; an empty workspace in the middle goes away when you leave it.  A number past the last
workspace is the last one.  `workspace "name"` in the config (with or without a block) declares a
named workspace: it exists from the start, in the order declared, and stays when it is empty.
`focus-workspace "name"` goes to it.  `input { workspace-auto-back-and-forth }` makes
`focus-workspace` for the workspace you are on go back to the previous one.

### niri's size argument

`set-column-width` and `set-window-height` take niri's size argument:

| Argument | Meaning |
|---|---|
| `"50%"` | that proportion of the view: two `"50%"` columns and the gap between them fill the view exactly, three `"33.333%"` columns too |
| `"+10%"` / `"-10%"` | that much more or less (a proportion of the view) than now |
| `"1200"` | the window gets 1200 px (the border around it comes on top) |
| `"+100"` / `"-100"` | 100 px more or less than now |

A column that is maximized loses that state when it gets a width of its own.  `wiri-ctl
set-column-width 50%` and `wiri-ctl action set-window-height 70%` take the same words.

### Porting a niri `binds {}` block

A niri `binds {}` block loads as written: `Mod+T { spawn "alacritty"; }`, with niri's properties
(`repeat=false`, `cooldown-ms=`, `hotkey-overlay-title=`, `allow-when-locked=`).  `Mod` is the
`input { mod-key }` modifier (Ctrl+Alt unless set), `focus-workspace 2` is an index and
`focus-workspace "2"` a name, and the size argument is niri's.  What wiri cannot do is reported when the
config is loaded, with its line (`wiri-ctl validate-config` lists it): a niri action wiri lacks, a mouse
or wheel trigger, an AltGr modifier and a media key are warnings and only that bind is dropped; a
misspelled action or key is an error.  wiri's older line form (`Mod+Shift+T spawn "wt.exe"`) still works.

---

## 6. IPC Protocol

wiri exposes a **named pipe** at `\\.\pipe\wiri_control_<user>_<session>` for external control via the `wiri-ctl` CLI tool.  The name carries the user and the session the daemon runs in, so that a second user logged in on the same machine does not collide with the first one; `WIRI_PIPE` overrides it.

### Connection

The pipe uses JSON-serialized messages over a Windows named pipe. Each message is a single JSON object terminated by a newline.

### Message Types

All messages use a `type`+`data` discriminated union format:

#### Requests

**Tile a window:**
```json
{"type":"tile_request","data":{"window_hwnd":123456,"target_workspace":"2"}}
```

**Move a window:**
```json
{"type":"window_move","data":{"window_hwnd":123456,"x":100,"y":200}}
```

**Resize a window:**
```json
{"type":"window_resize","data":{"window_hwnd":123456,"width":800,"height":600}}
```

**Get engine state:**
```json
{"type":"get_state"}
```

**List windows:**
```json
{"type":"window_list"}
```

**Focus a window:**
```json
{"type":"focus_window","data":{"window_hwnd":123456}}
```

**Switch workspace:**
```json
{"type":"switch_workspace","data":{"id":2}}
```

**Close a window:**
```json
{"type":"close_window","data":{"window_hwnd":123456}}
```

**Run any action (niri's `Request::Action`):**
```json
{"type":"run_action","data":{"name":"set-column-width","args":["+10%"]}}
{"type":"action","data":{"action":"focus-workspace","args":[2]}}
{"type":"run_action","data":{"name":"fullscreen-window","args":["--id",123456]}}
```
The name is a niri action name and the arguments are the words after it in a `binds {}` line (numbers and
booleans stand for their text; `"quoted":[true]` marks a workspace name written in quotes: `focus-workspace "2"`
is the workspace named "2", `focus-workspace 2` the second one).  `--id <hwnd>` names the window to act on
(`close-window --id` closes it and leaves the focus alone; any other action focuses the window first).

**Subscribe to events:**
```json
{"type":"subscribe_events","data":{"event_types":["window_focus_changed","workspace_activated"]}}
```

The stream is niri's: **the state first, then what changes.**  Right after the ack a subscriber gets
`workspaces_changed`, `windows_changed`, `overview_opened_or_closed` and `config_loaded` (the whole state: every
workspace and window with niri's fields, which window has the focus, how the config loaded), then
`window_opened_or_changed`, `window_closed`, `window_focus_changed`, `window_urgency_changed`,
`window_layouts_changed`, `workspace_activated`, `workspace_active_window_changed`, `workspace_urgency_changed`,
`workspaces_changed` and `overview_opened_or_closed` as things change.  A client that keeps a model from them
never has to ask or to poll.  wiri's older events (`window_opened`, `window_focused`, `workspace_switched`,
`column_focused`, ...) are still sent.  Everything is in [IPC_PROTOCOL.md](IPC_PROTOCOL.md) (§6.2) and
[IPC_SCHEMA.json](IPC_SCHEMA.json).

### Using wiri-ctl

```sh
wiri-ctl tile 123456          # Tile a window (by HWND)
wiri-ctl workspace 2          # Switch to workspace 2
wiri-ctl list                 # List all windows
wiri-ctl focus 123456         # Focus a window
wiri-ctl close 123456         # Close a window
wiri-ctl state                # Dump engine state
wiri-ctl windows              # The managed windows, in niri's shape
wiri-ctl window-list          # The managed windows, in wiri's older shape
wiri-ctl workspaces           # The workspaces, in niri's shape
wiri-ctl focused-window       # The window that has the keyboard focus
wiri-ctl overview-state       # Whether the overview is open
wiri-ctl version              # The version of the daemon
wiri-ctl action focus-column-left
wiri-ctl action set-column-width +10%
wiri-ctl action fullscreen-window --id 123456
wiri-ctl action focus-workspace '"2"'   # the workspace named 2 (words in literal double quotes are names)
wiri-ctl events --stream                # the state, then every change, one JSON object per line
wiri-ctl events --stream --filter window_focus_changed,workspace_activated
```

---

## 7. Module Reference

### 7.1 Backend (`src/backend/`)

Encapsulates all direct Win32 API interactions.

#### `mod.rs` — Core Backend Types

**`Backend`** — Main backend struct. On construction:
1. Enumerates all monitors via `EnumDisplayMonitors`
2. Enumerates all visible windows via `EnumWindows`
3. Starts `WinEventHook` for real-time window events

```rust
// Key methods
Backend::new() -> Result<Self>;
Backend::handle(&self) -> BackendHandle;
Backend::take_event_rx(&mut self) -> UnboundedReceiver<BackendEvent>;
Backend::run(&mut self) -> Result<()>;  // Blocks (message loop)
```

**`BackendHandle`** — Clonable, thread-safe handle for backend interaction:

| Method | Description |
|---|---|
| `get_windows()` | Return all tracked windows |
| `get_window(hwnd)` | Get window by HWND |
| `get_monitors()` | Return all monitors |
| `get_monitor(id)` | Get monitor by ID |
| `get_primary_monitor()` | Return primary monitor |
| `set_window_position(hwnd, rect, flags)` | Position a window (with 3 fallback strategies) |
| `show_window(hwnd, show)` | Show/hide a window |
| `send_event(event)` | Send a BackendEvent |
| `add_window/remove_window/update_window()` | Manage window cache |

**`WindowInfo`** — Full window metadata:

| Field | Type | Description |
|---|---|---|
| `hwnd` | `isize` | Window handle (raw) |
| `title` | `String` | Window title text |
| `class_name` | `String` | Window class name |
| `process_id` | `u32` | Owning process PID |
| `bounds` | `Rect` | Window position and size (DWM-aware) |
| `state` | `WindowState` | Normal/Minimized/Maximized/Fullscreen |
| `is_visible` | `bool` | Visibility flag |

**`MonitorInfo`** — Monitor metadata:

| Field | Type | Description |
|---|---|---|
| `id` | `OutputId` | Unique identifier |
| `name` | `String` | Device name (e.g. `\\.\DISPLAY1`) |
| `bounds` | `Rect` | Full monitor bounds |
| `work_area` | `Rect` | Usable area (excluding taskbar) |
| `is_primary` | `bool` | Primary monitor flag |
| `scale_factor` | `f64` | DPI scale (1.0 = 96 DPI) |

**`BackendEvent`** — Window events from WinEvent hook:

```rust
pub enum BackendEvent {
    WindowCreated { hwnd: isize },
    WindowDestroyed { hwnd: isize },
    WindowMoved { hwnd: isize, rect: Rect },
    WindowResized { hwnd: isize, rect: Rect },
    WindowShown { hwnd: isize },
    WindowHidden { hwnd: isize },
    MonitorConnected { id: u64 },
    MonitorDisconnected { id: u64 },
    ForegroundChanged { hwnd: isize },
    WindowTitleChanged { hwnd: isize },
}
```

**Window positioning strategy** (in `set_window_position`):

1. **Primary**: `SetWindowPos` with `SWP_ASYNCWINDOWPOS`
2. **Fallback 1**: `MoveWindow`
3. **Fallback 2**: `SetWindowPos` without `SWP_ASYNCWINDOWPOS`

#### `hooks.rs` — WinEvent Hook

Uses `SetWinEventHook` to receive real-time window events:

| Event | Description |
|---|---|
| `EVENT_OBJECT_CREATE (0x8000)` | Window created |
| `EVENT_OBJECT_DESTROY (0x8001)` | Window destroyed |
| `EVENT_OBJECT_SHOW (0x8003)` | Window shown |
| `EVENT_OBJECT_HIDE (0x8004)` | Window hidden |
| `EVENT_OBJECT_LOCATIONCHANGE (0x800B)` | Moved/resized |
| `EVENT_SYSTEM_FOREGROUND (0x0003)` | Focus changed |
| `EVENT_OBJECT_NAMECHANGE (0x800C)` | Title changed |

Uses `WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS` flags. A global `BACKEND_HANDLE` static forwards events as `BackendEvent` messages to the event channel.

#### `message_loop.rs` — Windows Message Pump

Runs a dedicated Windows message loop thread. Responsibilities:

- **Message pump**: `GetMessageW` → `TranslateMessage` → `DispatchMessageW`
- **Hotkey dispatch**: Receives `WM_HOTKEY` messages, converts to `Action` enum, calls engine methods
- **Hotkey registration**: Uses `RegisterHotKey` / `UnregisterHotKey`
- **Software key-repeat debounce**: 125ms interval (8 actions/sec) — avoids `MOD_NOREPEAT` which fails on ARM64
- **Live reload**: Custom `WM_RELOAD_HOTKEYS` message triggers re-read of config binds
- **22 default bindings**: Implemented as `(modifiers, vk_code, Action)` tuples

Key functions:

```rust
fn default_hotkeys() -> Vec<(u32, u32, Action)>;
fn build_hotkey_list(engine: &Arc<RwLock<TilingEngine>>) -> Vec<(u32, u32, Action)>;
```

`MessageLoop::reload_hotkeys()` — Public API: unregisters all, re-reads config, re-registers.

Action dispatch on `WM_HOTKEY`:
- `Quit` → `PostQuitMessage`
- `Spawn` → `CreateProcess` via shell
- `FocusColumnLeft/Right` → `engine.write().focus_left/right()`
- `FocusUp/Down` → `engine.write().focus_up/down()`
- `MoveColumnLeft/Right` → `engine.write().move_column_left/right()`
- `CloseWindow` → `SendMessage(WM_CLOSE)`
- `ToggleFullscreen` → `engine.write().toggle_fullscreen()`
- `ToggleFloating` → `engine.write().toggle_floating()`
- `ScrollLeft/Right` → `engine.write().scroll()`
- `FocusWorkspace(n)` → `engine.write().switch_workspace()`
- `OverviewToggle` → `engine.write().toggle_overview()`

### 7.2 Layout (`src/layout/`)

The layout engine implements the scrollable-tiling paradigm where windows are arranged in columns on an infinite horizontal strip.

#### `engine.rs` — TilingEngine

The central orchestrator for window management.

**Key state fields:**

| Field | Type | Description |
|---|---|---|
| `monitors` | `HashMap<OutputId, Monitor>` | All monitors with workspaces |
| `config` | `LayoutConfig` | Current layout configuration |
| `tiled_windows` | `HashMap<WindowId, WindowInfo>` | All tiled windows |
| `focused_output` | `Option<OutputId>` | Currently focused monitor |
| `fullscreen_windows` | `HashSet<WindowId>` | Fullscreen windows |
| `floating_windows` | `HashSet<WindowId>` | Floating windows |
| `saved_styles` | `HashMap<WindowId, u32>` | Saved window styles for fullscreen restore |
| `failed_position` | `HashSet<WindowId>` | Windows that reject positioning |
| `animation` | `AnimationManager` | Animation state machine |
| `overview` | `Option<f64>` | Overview zoom level (`Some(zoom_factor)`) |
| `window_rules` | `Vec<WindowRule>` | Active window matching rules |

**Key methods:**

| Method | Description |
|---|---|
| `new(config)` | Create empty engine |
| `register_monitor_with_scale(id, bounds, work_area, scale)` | Add a monitor |
| `unregister_monitor(id)` | Remove a monitor |
| `add_window(window_info, backend)` | Add window: check rules, strip frame, apply opacity |
| `remove_window(window_id, backend)` | Remove window, clean up state |
| `apply_layout_for_monitor(output_id, backend)` | Recalculate + apply positions for a monitor |
| `calculate_positions(workspace, work_rect)` | Compute window rects from layout |
| `focus_left/right/up/down()` | Move focus |
| `move_column_left/right(backend)` | Reorder columns |
| `scroll(direction, backend)` | Scroll workspace (with clamping) |
| `toggle_fullscreen(window_id, backend)` | Toggle fullscreen, save/restore styles |
| `toggle_floating(window_id, backend)` | Move in/out of tiling |
| `switch_workspace(id, backend)` | Switch workspace on focused monitor |
| `set_full_config(config)` | Apply window rules from config |
| `set_dwm_border_color(hwnd, color)` | Set DWM border (Windows 11+) |
| `strip_frame_for_tiling(hwnd)` | Remove invisible 7px resize borders |
| `apply_window_opacity(hwnd, opacity)` | Set window opacity |

**Layout calculation algorithm** (`calculate_positions`):

1. Determine proportional or fixed column widths
2. Calculate column X positions with gaps
3. Apply scroll offset (subtract `workspace.scroll_offset.x`)
4. Apply overview zoom (scale and center)
5. Cull off-screen columns (when scroll puts them outside view)
6. Distribute column height evenly among tiles
7. Return `Vec<(WindowId, Rect)>`

**Column width modes:**

- `Proportional`: `(view_width - total_gaps) / num_columns`, minimum `column_width / 2`
- `Fixed`: Uses each column's explicit `width` hint, or falls back to `column_width`

**Overview mode:**
- Zoom < 1.0 scales all columns proportionally
- Columns are centered in the view
- Each column and tile gets centered with padding

#### `mod.rs` — Monitor

Represents a physical display with independent workspaces.

| Field | Type | Description |
|---|---|---|
| `output_id` | `OutputId` | Unique monitor ID |
| `bounds` | `Rect` | Full display bounds |
| `work_area` | `Rect` | Usable area (sans taskbar) |
| `scale_factor` | `f64` | DPI scale (1.0 = 96 DPI) |
| `workspaces` | `HashMap<i32, Workspace>` | Workspace collection |
| `active_workspace` | `i32` | Current workspace ID |
| `focus_column` | `Option<usize>` | Focused column index |
| `focus_window` | `Option<WindowId>` | Focused window |

Key methods: `add_window`, `add_window_with_width`, `remove_window`, `focus_left/right/up/down`, `switch_workspace`.

#### `workspace.rs` — Workspace, Column, Tile

**`Tile`** — A single window in a column:
```
Tile { window_id: WindowId, size: Size }
```

**`Column`** — A vertical stack of tiles:
```
Column { tiles: Vec<Tile>, width: Option<u32> }
```
- `width: Option<u32>` — Explicit width hint for `ColumnWidthMode::Fixed`

**`Workspace`** — A scrollable workspace:
```
Workspace { columns: Vec<Column>, scroll_offset: Point }
```
- `scroll_offset.x` — Horizontal scroll position (modified by `scroll()` action)
- Methods: `add_window_to_new_column()`, `add_window_to_new_column_with_width()`, `add_window_to_column()`, `remove_window()`, `find_window_column()`, `move_window()`

#### `floating.rs` — FloatManager

Manages windows excluded from tiling layout:
- `FloatWindow { window_id, rect, saved_rect }`
- `FloatManager` is a `HashMap<WindowId, FloatWindow>` wrapper with: `insert`, `remove`, `get`, `get_mut`, `contains`, `bring_to_front`

#### `borders.rs` — FocusRenderer

Visual focus indication:

**Windows 11+ strategy:** Sets `DWMWA_BORDER_COLOR` via `DwmSetWindowAttribute` for colored window borders. Resets with `0xFFFFFFFF` (default) on focus loss.

**Fallback strategy:** The engine applies a wider visual gap for focused windows (`border_w + 4` inset vs `border_w`), providing visual distinction even without DWM support.

**Auto-detection:** Detects DWM support on first use and caches result to avoid repeated failed calls.

#### `animation.rs` — Animation Framework

Tick-based interpolation system.

**`Easing` enum:**
```rust
pub enum Easing {
    None,        // Instant transition
    Linear,      // Constant speed
    EaseIn,      // Slow start, fast end
    EaseOut,     // Fast start, slow end
    EaseInOut,   // Slow start and end
    CubicOut,    // Cubic bézier approximation (ease-out-cubic)
}
```

**`Animation` struct:**
```
Animation { start: f64, end: f64, duration_ms: u32, elapsed_ms: u32, easing: Easing, finished: bool }
```
- `tick(delta_ms)` → Advance and return current value
- `current()` → Current value without advancing
- Supports instant transitions via `Animation::instant(end)`

**`AnimationTarget` enum:**
```rust
pub enum AnimationTarget {
    ScrollX, WindowX, WindowY, WindowW, WindowH, Opacity,
}
```

**`AnimationManager`:**
- `animations: HashMap<AnimationTarget, Animation>`
- `tick(delta_ms)` → Advances all animations, returns completed targets
- `start_animation(target, animation)` → Creates/replaces an animation

**6 easing functions implemented:** `apply_easing(t, easing)` — applies the easing curve to normalized time `t` (0.0–1.0).

**Status:** 8 unit tests passing. Not yet wired into the main event loop's periodic tick.

### 7.3 Window (`src/window/`)

Window state tracking, mirroring niri's window lifecycle architecture.

**Key types:**

| Type | Description |
|---|---|
| `HwndWrapper` | Newtype around `HWND` with `Hash + Eq` |
| `WindowState` | `Normal \| Floating \| Maximized \| Fullscreen \| Minimized` |
| `WindowHandle` | Window metadata: `title`, `class_name`, `process_id` |
| `ResolvedWindowRules` | Computed properties: `float`, `workspace`, `column`, `follow_cursor`, `border`, `opacity` |
| `Mapped` | A visible, managed window |
| `Unmapped` | A window waiting for configure |
| `WindowRef` | `Mapped(Mapped) \| Unmapped(Unmapped)` |
| `WindowSet` | Collection of mapped + unmapped windows |

**`Mapped` fields:**
```rust
pub struct Mapped {
    pub id: WindowId,
    pub handle: WindowHandle,
    pub bounds: Rect,
    pub state: WindowState,
    pub rules: ResolvedWindowRules,
    pub activated: bool,
    pub decorated: bool,
}
```

**Window filtering** (`should_skip_window`):

Skips windows that are:
- Desktop windows: `"Progman"`, `"WorkerW"`
- Shell tool windows: `"Shell_TrayWnd"`, `"Shell_SecondaryTrayWnd"`, `"Shell_TrayWindow"`
- Popup menus (`"#32768"`), tooltips (`"tooltips_class32"`)
- Without visible styles or with popup/disabled styles
- Not owned by the current desktop (input desktop)

### 7.4 Input (`src/input/`)

#### `mod.rs` — Action Definitions & Parsers

**`Action` enum** — All possible actions:
```rust
pub enum Action {
    Quit,
    Spawn(String),
    CloseWindow,
    MoveColumnLeft,
    MoveColumnRight,
    FocusColumnLeft,
    FocusColumnRight,
    FocusUp,
    FocusDown,
    FocusWorkspace(i32),
    MoveWorkspace(i32),
    ToggleFullscreen,
    ToggleFloating,
    Maximize,
    Minimize,
    CenterWindow,
    SwitchMonitor,
    Refresh,
    ScrollLeft,
    ScrollRight,
    OverviewToggle,
    OverviewLeft,
    OverviewRight,
    OverviewSelect,
}
```

**Parsers:**

| Function | Signature | Description |
|---|---|---|
| `parse_action_name(name, args)` | `(&str, &[String]) -> Option<Action>` | Convert config string to Action |
| `parse_key_name(name)` | `(&str) -> Option<u32>` | Convert key name to Windows VK code |
| `parse_modifiers(names)` | `(&[String]) -> u32` | Convert modifier names to MOD_* flags |

Supporting types: `ModKey` (`Alt, Ctrl, Shift, Win`), `KeyboardConfig`, `MouseConfig`, `PointerVisibility`.

#### `hotkey.rs` — HotkeyManager

Dynamic hotkey registration using `RegisterHotKey`:

**`HotkeyBinding`** — `{ id: HotkeyId, modifiers: Vec<ModKey>, key_code: u32, action: Action }`
- `modifier_flags()` → Convert to Windows `MOD_*` flags

**`HotkeyManager`** — `{ bindings: HashMap<u32, HotkeyBinding>, next_id: u32 }`
- `register(modifiers, key_code, action)` → `Result<HotkeyId>`
- `unregister(id)` → Remove a binding

**Note:** Primary hotkey loop resides in `message_loop.rs`. `HotkeyManager` exists for future dynamic re-binding support.

#### `grab.rs` — Interactive Move/Resize

**`MoveGrab`** — Tracks window movement:
- Fields: `window_id`, `initial_cursor`, `initial_window_pos`, `initial_column`
- `delta(current_cursor)` → Movement delta from start
- `new_position(current_cursor)` → New window position
- `target_column(current_cursor, col_width, gap, total_cols)` → Determine column at cursor

**`ResizeGrab`** — Tracks window resizing:
- Fields: `window_id`, `edge`, `initial_cursor`, `initial_rect`
- `new_rect(current_cursor)` → New rectangle based on edge+drag

**`ResizeEdge`** — `Left | Right | Top | Bottom | TopLeft | TopRight | BottomLeft | BottomRight`

**`GrabResult`** — `Consumed | Defer | Done`

#### `mouse.rs` — MouseTracker

Tracks cursor position for focus-follows-mouse behavior:
- `update_cursor_pos()` → Query `GetCursorPos`, return current point
- `cursor_pos()`, `cursor_moved()`, `cursor_delta()`
- `MouseFocusConfig`: `focus_follows_mouse`, `warp_on_focus`, `focus_delay_ms`, `raise_on_click`

#### `low_level_hook.rs` — WH_MOUSE_LL Global Hook

Uses `SetWindowsHookExW(WH_MOUSE_LL, ...)` for global mouse interception:

**`GrabState`** — `None | Move(MoveGrab) | Resize(ResizeGrab)`

**Hook flow:**
1. **Alt+Left-click on tiled window** → Begin `MoveGrab`
2. **Alt+Right-click on window edge** → Detect edge → Begin `ResizeGrab`
3. Mouse move while grab active → Update position (consumes event)
4. Button release → End grab, re-tile window

**Thread safety:** Uses `LazyLock<Mutex<HookState>>` for shared state accessed from hook callback and main thread.

### 7.5 Config (`src/config/`)

KDL configuration system with file watching.

#### `types.rs` — Config Structs

| Struct | Description |
|---|---|
| `Config` | Root config: `input`, `output`, `layout`, `workspace`, `window_rules`, `binds`, `animations` |
| `InputConfig` | Keyboard/mouse settings |
| `LayoutConfig` | Tiling appearance and behavior |
| `OutputConfig` | Per-monitor settings |
| `WorkspaceConfig` | Workspace layout settings |
| `WindowRule` | Matching criteria + properties |
| `BindsConfig` | `hotkeys: Vec<HotkeyBinding>` |
| `AnimationsConfig` | Animation settings |
| `HotkeyBinding` | Single keybinding: `modifiers`, `key`, `command`, `args`, `repeat`, `release` |

**`HotkeyBinding` methods:**
- `parse_action()` → `Option<Action>` (delegates to `parse_action_name`)
- `mod_flags()` → `u32` MOD_* flags (delegates to `parse_modifiers`)
- `vk_code()` → `Option<u32>` VK code (delegates to `parse_key_name`)

**Utility:** `parse_color(color_str)` → `Option<[u8; 4]>` RGBA — parses `#RRGGBB` and `#RRGGBBAA`.

#### `kdl.rs` — KDL reader

A dependency-free KDL reader (nodes, arguments, properties, children, `/-`, comments, raw strings,
escapes, numbers, a BOM) that returns `Document { nodes }` with the `line:column` of every node,
argument and property, or the first `SyntaxError`.  It reads niri's KDL v1 and the v2 spellings where
they agree, plus wiri's older bare words and `key=value` lines.  Why not the `kdl` crate: niri's syntax
is v1 while the crate reads v2 (and pulls `miette` and `winnow`), and Phase 4 counts binary weight.

#### `decode.rs` — document → `Config`

`decode_text(text, file, base_dir)` / `decode_file(path)` map the nodes onto `Config` and return
`Decoded { config, issues, includes }`.  Every issue is a `ValidationIssue { severity, file, line,
column, message }`: an *error* means the file is rejected, a *warning* means part of it is ignored.
It knows niri's sections and keys (so an unsupported one is a warning and a typo is an error with a
"did you mean"), wiri's older spellings, the `include` resolution (relative to the including file,
cycle and depth checks, the include list for the watcher) and the bind grammar.

#### `parse.rs` — entry points

`parse_kdl_config(text)`, `validate_kdl_config(text)` and `validate_config_file(path)` — the names the
rest of the code and `wiri-ctl validate-config` use, all over `decode.rs`.

#### `loader.rs` — Config Loader

File-based config loading with hot-reload:
- `Config::load(path)` — read and decode; the first error as `file:line:column: error: message`
- `load_logged` / `load_quiet(path)` — the decoded config with all its issues (logged or not)
- `ConfigLoader::new(path)` — the config as it is now (the defaults when it has errors)
- `start_watching()` — `notify` watches the directory of the config and of every included file;
  a trailing-edge debounce (`RELOAD_QUIET_MS` = 200 ms of quiet) reads the file once after the
  last change, retries once when it has errors, then publishes a `ReloadOutcome` on `subscribe()`:
  `Applied { config, warnings }` or `Failed { issues, summary }`
- **Error handling**: a failed reload never touches the config in use

### 7.6 IPC (`src/ipc/`)

Named pipe server for external control.

**`IpcServer`:**
- `new()` → Create server
- `set_engine(engine)` — Bind to engine
- `run()` → Start accepting connections (async)

**`IpcMessage`** — Adjacently-tagged JSON enum:
```rust
pub enum IpcMessage {
    TileRequest { window_hwnd: isize, target_workspace: Option<String> },
    WindowMove { window_hwnd: isize, x: i32, y: i32 },
    WindowResize { window_hwnd: isize, width: u32, height: u32 },
    GetState,
    SubscribeEvents { event_types: Vec<String> },
    WindowList,
    FocusWindow { window_hwnd: isize },
    SwitchWorkspace { id: i32 },
    CloseWindow { window_hwnd: isize },
}
```

**`IpcError`** — Typed error enum: `ConnectionFailed`, `ReadError`, `WriteError`, `SerializationError`, `PipeCreationFailed`, `ClientDisconnected`, `Timeout`, `InvalidMessage`, `WindowsError`.

**Pipe path:** `\\.\pipe\wiri_control_<user>_<session>` — the pipe of the user and the session
the daemon runs in (`wiri::ipc::pipe_path()`; `WIRI_PIPE` overrides the name).

### 7.7 Hooks (`src/hooks/`)

System-level integrations.

**`SystemIntegration`:**
- `new(backend)` → Initialize: startup manager, spawner, tray icon
- `poll_tray_action()` → Check for tray menu events
- `run()` → Main loop (async)
- `startup_manager()`, `spawner()`, `tray_icon()`

**Components:**
- `TrayIcon` — `Shell_NotifyIconW` system tray icon with context menu (Reload, Quit)
- `Spawner` — Process creation via `CreateProcess`
- `StartupManager` — Registry-based auto-start management

### 7.8 Utils (`src/utils/`)

**`id.rs` — Identity Types:**

**`OutputId(u64)`** — Monitor identifier:
- `from_name(name)` → Hash-based ID from monitor device name
- Deterministic hash: `fold by 31 * byte_value`
- `Display: "Output:{hex}"`

**`WindowId(isize)`** — Window identifier (wraps HWND):
- `new(hwnd)` → Create from raw HWND
- `as_isize()` → Extract raw value
- `Display: "Window:{raw}"`

**`rect.rs` — Geometry Types:**

**`Point`** — `{ x: i32, y: i32 }` with `add`, `sub`, `contains_point`
**`Size`** — `{ w: u32, h: u32 }`
**`Rect`** — `{ loc: Point, size: Size }` with `contains_point`, `center`, `inset`

---

## 8. Data Structures

### Layout Data Model

```
TilingEngine
  └── monitors: HashMap<OutputId, Monitor>
        └── Monitor
              ├── output_id: OutputId
              ├── bounds: Rect
              ├── work_area: Rect
              ├── scale_factor: f64
              ├── active_workspace: i32
              ├── focus_column: Option<usize>
              ├── focus_window: Option<WindowId>
              └── workspaces: HashMap<i32, Workspace>
                    └── Workspace
                          ├── scroll_offset: Point
                          └── columns: Vec<Column>
                                └── Column
                                      ├── width: Option<u32>
                                      └── tiles: Vec<Tile>
                                            └── Tile
                                                  ├── window_id: WindowId
                                                  └── size: Size
```

### Config Data Model

```
Config
  ├── input: InputConfig
  ├── output: Vec<OutputConfig>
  ├── layout: LayoutConfig
  ├── workspace: Vec<WorkspaceConfig>
  ├── window_rules: Vec<WindowRule>
  ├── binds: BindsConfig { hotkeys: Vec<HotkeyBinding> }
  └── animations: AnimationsConfig
```

### Engine Internal State

```
TilingEngine
  ├── monitors: HashMap<OutputId, Monitor>
  ├── config: LayoutConfig
  ├── tiled_windows: HashMap<WindowId, WindowInfo>
  ├── focused_output: Option<OutputId>
  ├── fullscreen_windows: HashSet<WindowId>
  ├── floating_windows: HashSet<WindowId>
  ├── window_sizing: HashMap<WindowId, SizingMode>
  ├── saved_styles: HashMap<WindowId, u32>
  ├── saved_ex_styles: HashMap<WindowId, u32>
  ├── window_rules: Vec<WindowRule>
  ├── full_config: Option<Config>
  ├── animation: AnimationManager
  ├── overview: Option<f64>
  └── failed_position: HashSet<WindowId>
```

---

## 9. Event Flow

### Window Creation

```
WinEventHook (EVENT_OBJECT_CREATE)
  → winevent_proc callback
    → BackendHandle::send_event(BackendEvent::WindowCreated { hwnd })
      → event_rx (tokio mpsc)
        → handle_backend_event()
          → BackendHandle::add_window() (cache)
          → if should_tile_window():
              → engine.write().add_window(window_info, backend)
                → Check WindowRules (float? workspace?)
                → Monitor::add_window_with_width()
                → strip_frame_for_tiling()
                → apply_layout_for_monitor()
                  → calculate_positions()
                  → backend.set_window_position() for each window
                  → set_dwm_border_color() for focused
                  → apply_window_opacity() for dimmed
```

### Window Destruction

```
WinEventHook (EVENT_OBJECT_DESTROY)
  → winevent_proc callback
    → BackendHandle::send_event(BackendEvent::WindowDestroyed { hwnd })
      → handle_backend_event()
        → engine.write().remove_window(window_id, backend)
          → clear fullscreen/floating/sizing state
          → Monitor::remove_window()
          → apply_layout_for_monitor()
```

### Key Binding

```
RegisterHotKey(hwnd=NULL, id, mod_flags, vk_code)
  → WM_HOTKEY { wParam = id, lParam = mod_flags<<16 | vk_code }
    → MessageLoop message pump
      → match registered action:
          Action::FocusColumnLeft → engine.write().focus_left()
          Action::ToggleFullscreen → engine.write().toggle_fullscreen()
          Action::Spawn(cmd) → CreateProcess(cmd)
          // ... etc
      → apply_layout_for_monitor() (if layout changed)
```

### Mouse Move/Resize

```
WH_MOUSE_LL hook → MSLLHOOKSTRUCT
  → Alt key down + WM_LBUTTONDOWN on tiled window
    → begin_move_grab(window_id, cursor, window_pos, column)
    → GrabState::Move(MoveGrab)
  → WM_MOUSEMOVE while grab active
    → update window position via SetWindowPos
    → return 1 (consume event)
  → WM_LBUTTONUP
    → end grab
    → re-tile window at new position
    → GrabState::None
```

### Config Reload

```
notify file watcher: a change to the config or to a file it includes
  → the reload thread waits until the files have been quiet for 200 ms (trailing edge)
    → read file(s) → decode → Decoded { config, issues, includes }
    → errors: look once more after another quiet period, then
        send ReloadOutcome::Failed { issues, summary } — the last good config stays
    → no errors: store in Arc<RwLock<Config>>, send ReloadOutcome::Applied { config, warnings }
  → main loop (ReloadCtx) receives the outcome
    → Applied: engine.write().set_full_config(); message_loop.reload_hotkeys();
        broadcast IpcEvent::ConfigLoaded { failed: false }
      → PostThreadMessage(WM_RELOAD_HOTKEYS)
        → message loop: UnregisterHotKey all → read config binds → RegisterHotKey all
      → apply_layout_for_monitor()
    → Failed: log every issue, tray balloon "file:line:column: error: message",
        broadcast IpcEvent::ConfigLoaded { failed: true, error }
```

---

## 10. Developer Guide

### Project Layout

```
WM/wiri/
├── src/
│   ├── main.rs              # Entry point, event loop, initialization
│   ├── lib.rs               # Module declarations
│   ├── backend/             # Win32 window/monitor management
│   │   ├── mod.rs           # Backend, BackendHandle, WindowInfo, MonitorInfo
│   │   ├── hooks.rs         # WinEventHook (SetWinEventHook)
│   │   └── message_loop.rs  # Windows message pump, hotkey action dispatch
│   ├── layout/              # Tiling layout engine
│   │   ├── engine.rs        # TilingEngine — core logic
│   │   ├── workspace.rs     # Workspace, Column, Tile
│   │   ├── mod.rs           # Monitor struct + unit tests
│   │   ├── floating.rs      # FloatManager
│   │   ├── borders.rs       # FocusRenderer (DWM border colors)
│   │   └── animation.rs     # Animation framework
│   ├── window/              # Window state management
│   │   ├── mod.rs           # Mapped/Unmapped/WindowRef/WindowSet
│   │   └── rules.rs         # Window rule resolution
│   ├── input/               # Input handling
│   │   ├── mod.rs           # Action enum, parse functions
│   │   ├── hotkey.rs        # HotkeyManager
│   │   ├── grab.rs          # MoveGrab, ResizeGrab
│   │   ├── mouse.rs         # MouseTracker
│   │   └── low_level_hook.rs # WH_MOUSE_LL global hook
│   ├── config/              # KDL configuration
│   │   ├── types.rs         # Config structs
│   │   ├── parse.rs         # KDL parser
│   │   ├── loader.rs        # File loading + watcher
│   │   └── error.rs         # Config errors
│   ├── hooks/               # System integration
│   │   ├── mod.rs           # SystemIntegration
│   │   ├── tray.rs          # Tray icon
│   │   ├── spawner.rs       # Process spawner
│   │   └── startup.rs       # Auto-start
│   ├── ipc/                 # Named pipe IPC
│   │   └── mod.rs           # IpcServer, IpcMessage
│   └── utils/               # Utilities
│       ├── mod.rs           # Re-exports
│       ├── id.rs            # OutputId, WindowId
│       └── rect.rs          # Point, Size, Rect
├── docs/
│   ├── SPEC.md              # Full specification
│   ├── PROGRESS.md          # Implementation progress
│   └── DOCUMENTATION.md     # This file
└── resources/               # Resources
```

### Building and Testing

```sh
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Run specific test
cargo test test_monitor_add_window

# Run with logging
RUST_LOG=debug cargo run

# Build with feature flags
cargo build --features "tray"    # tray enabled (default)
cargo build --no-default-features  # without tray support
```

### Adding a New Feature

1. **New hotkey action:**
   - Add variant to `Action` enum in `input/mod.rs`
   - Add parsing in `parse_action_name()`
   - Add dispatch case in `message_loop.rs`
   - Add default binding in `default_hotkeys()`
   - Implement the action in `TilingEngine` (engine.rs)

2. **New config option:**
   - Add field to relevant `*Config` struct in `config/types.rs`
   - Add parsing in `config/parse.rs` (`apply_*_property`)
   - Apply in `LayoutConfig::from_config()` if layout-related

3. **New window rule property:**
   - Add field to `WindowRule` in `config/types.rs`
   - Add parsing in `config/parse.rs`
   - Apply in `engine.rs` `add_window()` / rule resolution

4. **New backend event:**
   - Add variant to `BackendEvent` in `backend/mod.rs`
   - Register new event in `WinEventHook::new()` (hooks.rs)
   - Handle in `handle_backend_event()` (main.rs)

### ARM64 Compatibility Notes

- Do NOT use `MOD_NOREPEAT` (0x4000) — it causes `RegisterHotKey` to fail on ARM64 Windows
- Use software key-repeat debouncing instead (125ms interval in message_loop.rs)
- All Win32 API usage is through the `windows` crate, which handles ARM64 translation

### Code Style

- Follow Rust 2021 edition idioms
- All errors use `anyhow::Result` or typed error enums
- Thread safety via `Arc<RwLock<>>` for shared state
- Use `tracing` for logging (not `println`)
- Prefer `Windows::Win32::*` APIs through the `windows` crate
- Tests in `#[cfg(test)] mod tests {}` blocks within source files

---

## 11. Testing

### Test Organization

Tests are located in `#[cfg(test)] mod tests {}` blocks within the relevant source files:

| Module | Test Count | Focus |
|---|---|---|
| `layout/mod.rs` | ~14 | Monitor operations, focus navigation, workspace switching |
| `layout/animation.rs` | 8 | Animation math, easing functions, tick logic |
| `input/mod.rs` | ~20 | Action parsing, key name parsing, modifier parsing |
| `window/mod.rs` | ~12 | Window set operations, state transitions, HWND conversion |
| `utils/id.rs` | ~9 | OutputId/WindowId creation, equality, hashing |
| Total | ~59 | All passing |

### Running Tests

```sh
# All tests
cargo test

# With output
cargo test -- --nocapture

# Specific module
cargo test monitor_tests

# Specific test
cargo test test_monitor_add_window

# Release mode tests (slower compile, but more realistic)
cargo test --release
```

### Key Test Areas

**Monitor tests:**
- Creating monitors with/without scale factors
- Adding/removing windows, focus state tracking
- Workspace switching and window preservation
- Focus left/right/up/down boundary conditions
- Empty monitor edge cases (no panic on focus ops)

**Animation tests:**
- Easing function correctness
- Tick advancement and completion detection
- Linear, ease-in, ease-out, cubic-out curves
- Instant (zero-duration) animations

**Input parser tests:**
- All action name variants and aliases
- Key name parsing (letters, digits, arrows, special, function keys)
- Modifier parsing (individual, combined, case-insensitive)
- Missing argument handling

**Window set tests:**
- Mapped/unmapped window lifecycle
- Count tracking (mapped, unmapped, total)
- State checks (is_floating, is_fullscreen, etc.)
- Bounds updates

**ID tests:**
- Deterministic OutputId hashing
- WindowId equality and display formatting
- Hash set uniqueness

### Debugging

```sh
# Verbose mode
wiri -v

# Environment variable logging
RUST_LOG=wiri=debug wiri
RUST_LOG=wiri::layout=trace wiri
RUST_LOG=wiri::input=debug wiri

# Check log file (PowerShell)
Get-Content $env:LOCALAPPDATA\wiri\wiri.log -Tail 50
```

Common debugging areas:
- `SetWindowPos` failures (logged as debug messages)
- Window filtering (check `should_skip_window` / `should_tile_window`)
- Hotkey registration failures (logged as warnings)
- Config parse errors (logged as warnings, defaults used)

---

## Appendices

### A. Win32 API Reference

| API | Usage | Module |
|---|---|---|
| `EnumWindows` | Enumerate all top-level windows | `backend/mod.rs` |
| `EnumDisplayMonitors` | Enumerate monitors | `backend/mod.rs` |
| `GetMonitorInfoW` | Monitor bounds, work area | `backend/mod.rs` |
| `SetWindowPos` | Position/resize windows | `backend/mod.rs` |
| `MoveWindow` | Fallback window positioning | `backend/mod.rs` |
| `ShowWindow` | Show/hide windows | `backend/mod.rs` |
| `GetWindowTextW` | Window title | `backend/mod.rs` |
| `GetClassNameW` | Window class name | `backend/mod.rs` |
| `GetWindowRect` | Window bounds | `backend/mod.rs` |
| `DwmGetWindowAttribute` | Extended frame bounds | `backend/mod.rs` |
| `DwmSetWindowAttribute` | DWM border color | `layout/borders.rs` |
| `SetWinEventHook` | Real-time window events | `backend/hooks.rs` |
| `RegisterHotKey` | Global hotkeys | `input/hotkey.rs`, `backend/message_loop.rs` |
| `UnregisterHotKey` | Unregister hotkeys | `backend/message_loop.rs` |
| `SetWindowsHookExW(WH_MOUSE_LL)` | Global mouse hook | `input/low_level_hook.rs` |
| `GetCursorPos` | Cursor position | `input/mouse.rs` |
| `GetMessageW` / `DispatchMessageW` | Message pump | `backend/message_loop.rs` |
| `CreateWindowExW` | Hidden message-only window | `backend/message_loop.rs` |
| `Shell_NotifyIconW` | System tray icon | `hooks/tray.rs` |
| `PostThreadMessageW` | Cross-thread messages | `backend/message_loop.rs` |
| `SetProcessDpiAwarenessContext` | Per-monitor DPI | `main.rs` |
| `GetDpiForMonitor` | Monitor DPI | `backend/mod.rs` |
| `SetLayeredWindowAttributes` | Window opacity | `layout/engine.rs` |

### B. File Size Breakdown

| File | Lines | Description |
|---|---|---|
| `src/main.rs` | ~456 | Entry point, event loop |
| `src/backend/mod.rs` | ~340 | Backend + BackendHandle |
| `src/backend/message_loop.rs` | ~420 | Hotkey message pump |
| `src/backend/hooks.rs` | ~120 | WinEvent hook |
| `src/layout/engine.rs` | ~540 | TilingEngine core |
| `src/layout/mod.rs` | ~280 | Monitor + tests |
| `src/layout/workspace.rs` | ~180 | Workspace, Column, Tile |
| `src/layout/floating.rs` | ~120 | FloatManager |
| `src/layout/borders.rs` | ~140 | FocusRenderer |
| `src/layout/animation.rs` | ~240 | Animation framework |
| `src/window/mod.rs` | ~600 | Window state management |
| `src/window/rules.rs` | ~100 | Rule resolution |
| `src/input/mod.rs` | ~380 | Action enum + parsers |
| `src/input/hotkey.rs` | ~180 | HotkeyManager |
| `src/input/grab.rs` | ~200 | MoveGrab, ResizeGrab |
| `src/input/mouse.rs` | ~160 | MouseTracker |
| `src/input/low_level_hook.rs` | ~260 | WH_MOUSE_LL hook |
| `src/config/types.rs` | ~280 | Config structs |
| `src/config/parse.rs` | ~400 | KDL parser |
| `src/config/loader.rs` | ~200 | File watcher |
| `src/config/error.rs` | ~60 | Error types |
| `src/ipc/mod.rs` | ~350 | IPC server |
| `src/hooks/*.rs` | ~300 | System integration |
| `src/utils/*.rs` | ~200 | Geometry + IDs |
| Total | ~8,387 | 32 source files |