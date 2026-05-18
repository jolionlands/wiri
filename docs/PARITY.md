# wiri ↔ niri parity

A frank stocktake of what wiri has shipped from
[niri](https://github.com/YaLTeR/niri)'s feature set, what's missing, and
what's intentionally out of scope.

Last refreshed: 2026-05-18 (commit `1392948`+, six opus polish passes).

## Legend

| Mark | Meaning |
| :---: | --- |
| ✅ | Shipped, default behaviour matches niri |
| 🟡 | Partial — primitive exists, polish pending |
| ❌ | Not shipped |
| 🚫 | Out of scope (Wayland-only / not applicable on Win32) |

## Layout

| Feature | Status | Notes |
| --- | :---: | --- |
| Scrollable-tiling columns on an infinite strip | ✅ | Core engine |
| Per-monitor independent workspace stack | ✅ | |
| Floating windows | ✅ | `Action::ToggleFloating` |
| Fullscreen toggle | ✅ | `Action::ToggleFullscreen` |
| Maximize column (full work-area height) | ✅ | `Action::MaximizeColumn`, `Ctrl+Alt+Shift+F` |
| Consume window into adjacent column | ✅ | `Ctrl+Alt+Comma` |
| Expel window from column into new column | ✅ | `Ctrl+Alt+Period` |
| Expand column to fill available width | ✅ | `Ctrl+Alt+E` |
| Column width presets (1/4, 1/3, 1/2, 2/3, 3/4, full) | ✅ | `set-column-width` |
| Cycle through column-width presets | ✅ | `set-column-width "cycle"` |
| Keyboard column resize | ✅ | `Ctrl+Alt+Shift+H` / `L` (5% steps) |
| Keyboard tile-height resize | ✅ | `Ctrl+Alt+Shift+K` / `J` (5% steps) |
| Move column to adjacent monitor | ✅ | `Ctrl+Alt+Shift+PgUp` / `PgDn` |
| Move column within workspace | ✅ | `Ctrl+Alt+Shift+Left` / `Right` |
| Center focused column on screen | ✅ | `Action::CenterWindow` |
| Workspace switch | ✅ | `Ctrl+Alt+1`–`9` |
| Dynamic workspace creation/removal | ✅ | |
| Named workspaces | ✅ | `wiri-ctl focus-workspace-named` |
| Window-rules (class / title / pid / regex) | ✅ | KDL `window-rules { … }` |
| `open-on-output` rule | ✅ | Resolved in `ResolvedWindowRules` |
| `open-in-workspace` rule | ✅ | |
| `open-fullscreen` rule | ✅ | |
| `open-floating` rule | ✅ | |
| `open-max-bounds` rule | ✅ | |
| Auto-engage overview when columns exceed threshold | ✅ | `auto-tile-threshold` |
| Tabbed columns within a column | ✅ | `Ctrl+Alt+\` toggles, `Ctrl+Alt+]` / `[` next/prev tab |
| Interactive snap-on-drag with snap guides | 🟡 | Agent H pass landing |
| Overview mode (zoom out, see all workspaces) | 🟡 | Agent G pass landing |
| Workspace slide animation | ✅ | Engine biases tile Y by `workspace_slide_offset(oid)` |
| Per-output `layout {}` overrides | ✅ | `TilingEngine.config_per_monitor` + `effective_config(oid)` accessor |
| Interactive resize mode (Mod+R cycle) | ✅ | `Mod+R` toggles; `Mod+Arrow` resize while engaged; Esc exits |
| Move column vertically between workspaces | ❌ | `Action::MoveToWorkspace { id }` exists but no "up/down by 1" shortcut |
| Stack column (vertical of same width) — niri's "consume to next" inverse | ❌ | |

## Input

| Feature | Status | Notes |
| --- | :---: | --- |
| Global hotkeys via `RegisterHotKey` | ✅ | 31 default binds |
| Software key-repeat debounce (ARM64-safe) | ✅ | 125 ms |
| Hot-reloadable keybindings | ✅ | `notify` watcher, 300 ms debounce |
| Configurable modifier prefix | ✅ | `input { mod-key "Ctrl+Alt" }` |
| Alt+left-drag interactive move | ✅ | `WH_MOUSE_LL` hook |
| Alt+right-drag interactive resize | ✅ | |
| Mouse-wheel workspace scroll (modifier held) | ✅ | Respects `natural_scroll` / `scroll_speed` |
| Two-finger pinch gesture | ✅ | `Gesture::PinchIn` / `PinchOut`, maps to overview toggle |
| Focus-follows-mouse | ✅ | `input { focus-follows-mouse true }` |
| `spawn-cmd` shorthand | ✅ | Parser shipped; dispatch wiring queued |
| Touch tap / swipe gestures | 🟡 | Single-touch ready, multi-touch pinch coalesced |
| Tablet / stylus support | ❌ | |

## Visual

| Feature | Status | Notes |
| --- | :---: | --- |
| Per-monitor DPI awareness (PER_MONITOR_AWARE_V2) | ✅ | |
| Coloured focused-window borders (DWM) | ✅ | Win11+ via `DWMWA_BORDER_COLOR` |
| Workspace switcher overlay (top-centre, fade in/out, multi-monitor) | ✅ | Win11 rounded corners |
| Tray icon + context menu | ✅ | Reload / About / Open config / Screenshot / Quit |
| Window opacity (per rule) | ✅ | `Option<f64>`, explicit `0` honoured |
| DWM drop-shadow toggle | ✅ | Once-per-HWND `DwmExtendFrameIntoClientArea` |
| Animation framework (6 easings, ~60 fps tick) | ✅ | |
| Focus ring (separate from border) | ✅ | `focus-ring-width` / `focus-ring-color` |
| Wallpaper integration | 🚫 | Win32 desktop manages wallpaper |
| Cursor theme | 🚫 | Windows handles globally |
| Blur backdrop (niri's `blur { … }`) | ❌ | DwmEnableBlurBehindWindow exists; not wired |

## Configuration

| Feature | Status | Notes |
| --- | :---: | --- |
| KDL format with hot-reload | ✅ | |
| `include "other.kdl"` directive (depth-capped, cycle-detected) | ✅ | |
| `wiri-ctl validate-config` | ✅ | line/col error reporting |
| Per-monitor `output { … }` config | ✅ | name + index match |
| `output { scale }` validation (0.5–4.0) | ✅ | warns + clamps |
| Default-config drop on first run | ✅ | `%APPDATA%\wiri\config.kdl` |
| Merge-with-defaults for `binds {}` | ✅ | Default `extend-defaults true`; config wins on collision, additions are appended |

## IPC

| Feature | Status | Notes |
| --- | :---: | --- |
| Named-pipe IPC at `\\.\pipe\wiri_control` | ✅ | ACL: Admins + owner only |
| 30+ `wiri-ctl` subcommands, human + `--json` output | ✅ | |
| Event streaming (`wiri-ctl events`) | ✅ | wall-clock timestamps |
| Diagnostic `list-monitors` | ✅ | |
| Diagnostic `test-bindings` (registered hotkeys + actions) | 🟡 | Agent G pass landing |
| Window capture (`capture-window`) | 🟡 | Agent H pass landing |
| Screenshot capture (`screenshot`) | ✅ | Default bind `Ctrl+Alt+P` |
| Workspace + window persistence across reloads | ✅ | `wiri-ctl save-snapshot / load-snapshot / list-snapshots / delete-snapshot` → JSON under `%APPDATA%\wiri\snapshots\` |

## Robustness

| Feature | Status | Notes |
| --- | :---: | --- |
| Position-failure circuit breaker (auto-float after 3 SetWindowPos rejections) | ✅ | UAC / anti-cheat windows |
| Single-instance lock via named-pipe probe | ✅ | |
| Urgent-window detection (`EVENT_SYSTEM_ALERT`) | ✅ | |
| Live `MatcherContext` flags (`is_floating`, `is_urgent`, `at_startup`) | ✅ | |
| Intel / AMD HotKey-Service conflict warning at startup | 🟡 | Agent G pass landing |
| Graceful degradation on hotkey-already-registered (logs WARN, keeps going) | ✅ | |
| Frame-stripping with shadow re-extension | ✅ | Windows Terminal repaint workaround |

## Out of scope (Wayland-only or platform-mismatched)

- Layer-shell protocol (`zwlr_layer_shell_v1`) — Win32 equivalent would be widget windows on a desktop layer; no demand yet.
- XWayland — N/A.
- DBus interface — Win32 has named pipes; we use them.
- Screen-cast portal (`org.freedesktop.portal.ScreenCast`) — Win32 equivalent is the Desktop Duplication API; not wired.
- `wlr-screencopy` — same as above.
- VRR / adaptive sync — Win32 has DWM that handles this implicitly.
- Cursor theme protocol — Windows global cursor settings cover this.
- Wayland clipboard managers — Win32 clipboard is global, no wiri scope.

## Niri features genuinely missing (good first issues)

These are concrete, scoped, and would tighten parity:

- **Move-column-vertically-between-workspaces** — `Action::MoveColumnToWorkspaceAbove` / `Below`.
- **Workspace move/swap** — `Action::MoveWorkspaceUp` / `Down` reorders the workspace stack.
- **Smart-borders** — only draw border when more than one window is visible.
- **Wallpaper-aware accent** — DWM `DWMWA_CAPTION_COLOR` from system accent.
- **Blur backdrop** for floating windows via `DwmEnableBlurBehindWindow`.
- **Win+L lock screen passthrough** — currently captured at the WM level; should fall through.
- **More-faithful overview rendering** — DWM thumbnail per window (via `DwmRegisterThumbnail`) instead of resizing the live HWND.

PRs welcome.
