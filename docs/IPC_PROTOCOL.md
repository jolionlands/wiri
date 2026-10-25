# wiri IPC Protocol — Integration Reference

**Schema version: 1.3**  
Last updated: 2026-10-25

This document is the canonical reference for third-party integrations with wiri.
Target audiences: aurora, crest, custom scripts, and any external tool that wants
to query or control wiri at runtime.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Security](#2-security)
3. [Message Format](#3-message-format)
4. [Request Types](#4-request-types)
5. [Response Shape](#5-response-shape)
6. [Event Types](#6-event-types)
7. [Code Examples](#7-code-examples)
8. [Versioning Policy](#8-versioning-policy)
9. [Common Patterns](#9-common-patterns)

---

## 1. Overview

wiri exposes a **named-pipe IPC server** at:

```
\\.\pipe\wiri_control_<user>_<session>
```

The name carries the user and the session the daemon runs in, because a pipe
name is machine-global on Windows: with one name for everybody, a second user
logged in on the same machine (fast user switching, an RDP session) could not
create its pipe, and its clients would have talked to the first user's daemon.
`<user>` is the account name (`GetUserNameW`), `<session>` the session id
(`ProcessIdToSessionId`); both are made safe for a pipe name (letters, digits,
`-`, `_` and `.` are kept, everything else becomes `_`).

`WIRI_PIPE` overrides the name: set it to the full pipe path
(`\\.\pipe\wiri_control_alice_1`) and both the daemon and the clients use it.
That is what the tests use, and what a client that was told where the daemon
listens uses.

The protocol accepts **one UTF-8 JSON value per line**, with one compact JSON reply
per line. Keep the connection open to send more requests, including after an error.
A connection switched to `EventStream` / `subscribe_events` is dedicated to events.

For existing clients, a complete first JSON value **without a newline** is still a
one-shot request: the daemon replies and closes, so old `read_to_end` clients work.
Write the newline together with the JSON value when using the persistent protocol;
without a delimiter a complete first value is indistinguishable from a legacy request.
Partial JSON and multiple lines in one write are supported. Each request line is
limited to 64 KiB; an oversized line is drained and answered with an error, after
which the next line is usable. Unknown variants and invalid fields return an error;
unknown future fields are ignored. No idle read timer is used.

`WIRI_PIPE` is exported to spawned children. The event stream acknowledges the
subscription, replays current state, then sends changes until disconnected. A silent
disconnect releases the subscription immediately.

---

## 2. Security

The named pipe is created with a DACL that limits access to:

| Principal              | Access  | SDDL  |
|------------------------|---------|-------|
| `BUILTIN\Administrators` | Full  | `(A;;GA;;;BA)` |
| Object owner (wiri process user) | Full | `(A;;GA;;;OW)` |

Other local users on a shared workstation **cannot connect** to
`\\.\pipe\wiri_control_<user>_<session>`. Connections from non-elevated
processes running as the same user succeed because ownership matches.

If wiri cannot build the custom DACL at startup it falls back to the default
pipe security (inherited from the process token), logging a warning.

---

## 3. Message Format

### 3.1 Encoding

- UTF-8, no BOM.
- No length prefix; newline-delimited UTF-8 values (CRLF is accepted).
- Maximum incoming request line: 64 KiB. Legacy undelimited first values still work.

### 3.2 Serde tagging

Wiri requests (`IpcMessage`) and events (`IpcEvent`) retain serde's
**adjacent tagging** scheme:

```json
{ "type": "<variant_name>", "data": { ...fields... } }
```

For unit variants (no fields), the `"data"` key is omitted.

Niri's externally tagged spelling is also accepted. It selects niri replies and
niri events on that connection, without changing wiri's existing envelopes:

```json
"Windows"
{"Ok":{"Windows":[]}}
{"Action":{"FocusColumnLeft":{}}}
{"Ok":"Handled"}
"ReturnError"
{"Err":"example error requested by ReturnError"}
```

`"EventStream"` acknowledges with `{"Ok":"Handled"}` then streams external events
such as `{"WindowFocusChanged":{"id":123}}`. Wiri-only events are omitted in that
dialect. Niri's unit requests and typed supported actions use their PascalCase names;
unsupported requests/actions/flags return a descriptive error rather than disconnecting.
Wiri replies include additive `schema_version: "1.3"`; niri envelopes keep one key.
`return_error` / `"ReturnError"` deliberately returns an error without altering state.

### 3.3 Example round-trip (one-shot)

**Client sends:**
```json
{"type":"get_state"}
```

**Server responds:**
```json
{"success":true,"result":{"windows":[...],"workspaces":[...],"active_workspace":"workspace-1"}}
```

**Pipe closes.**

### 3.4 Example round-trip (streaming)

**Client sends:**
```json
{"type":"subscribe_events","data":{"event_types":["workspace_switched","window_focused"]}}
```

**Server sends ack:**
```json
{"success":true,"subscription_id":1}
```

**Server then streams (one per newline):**
```json
{"type":"workspace_switched","data":{"workspace_id":2,"monitor_id":17629372411}}
{"type":"window_focused","data":{"window_hwnd":328756}}
```

Before any change the stream carries the **current state** as events (niri's event stream:
`workspaces_changed`, `windows_changed`, `overview_opened_or_closed`, `config_loaded`; see
[§6](#6-event-types)).  The filter applies to them too, so the example above, which names two
other types, sees none of them; a subscription without a filter starts with all four.

---

## 4. Request Types

### 4.1 Window Management

#### `tile_request`

Force a window into the tiling layout (un-floats it if floating).

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `window_hwnd` | `integer` | yes | Win32 HWND (decimal) |
| `target_workspace` | `string \| null` | no | Workspace name to move to |

```json
{"type":"tile_request","data":{"window_hwnd":328756,"target_workspace":"dev"}}
```

---

#### `window_move`

Move a window to an absolute screen position.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `window_hwnd` | `integer` | yes | Win32 HWND |
| `x` | `integer` | yes | Left edge in screen pixels |
| `y` | `integer` | yes | Top edge in screen pixels |

---

#### `window_resize`

Resize a window to explicit pixel dimensions.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `window_hwnd` | `integer` | yes | Win32 HWND |
| `width` | `integer (u32)` | yes | Width in pixels |
| `height` | `integer (u32)` | yes | Height in pixels |

---

#### `window_list`

Return all windows currently tracked by the engine. No fields.

Response `result`: array of window objects with `hwnd`, `title`, `class_name`,
`process_id`, `x`, `y`, `width`, `height`, `workspace` (the 1-based number; for a
floating window the workspace it floats on, for a minimized one the workspace it
returns to), `column` and `tile` (0-based position in the strip, `null` for a floating
or minimized window), `floating`, `minimized`, `fullscreen` (the window covers its
monitor; it keeps its column while it does) and `maximized` (maximized to the edges of
the work area, niri's `maximize-window-to-edges`).

---

#### `window_find`

Search tracked windows by a query string (title substring match).

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `query` | `string` | yes | Title substring |

---

#### `focus_window`

Set keyboard focus to a specific HWND.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `window_hwnd` | `integer` | yes | Win32 HWND |

---

#### `close_window`

Send `WM_CLOSE` to a window.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `window_hwnd` | `integer` | yes | Win32 HWND |

---

#### `window_float`

Take a window out of the tiling layout (floating mode).

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `window_hwnd` | `integer` | yes | Win32 HWND |

---

#### `window_unfloat`

Return a floating window to the tiling layout.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `window_hwnd` | `integer` | yes | Win32 HWND |

---

#### `window_move_to_workspace`

Move a specific window (by HWND) to a workspace.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `window_hwnd` | `integer` | yes | Win32 HWND |
| `workspace_id` | `integer (i32)` | yes | Destination workspace id |

---

#### `screenshot_window`

Capture a window to PNG and the clipboard. One-shot Windows.Graphics.Capture is tried first,
with `PrintWindow(PW_RENDERFULLCONTENT)` fallback. A forced fallback and protected/GPU-specific
surfaces have not been live-verified.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `hwnd` | `integer \| null` | no | HWND to capture; null = focused window |
| `path` | `string \| null` | no | Output PNG path; null = `screenshot-path` setting |

Response `result.path`: resolved file path, or null for clipboard-only (`screenshot-path null`).

---

#### `capture_window`

Capture a wiri-tracked window with the same PNG/clipboard pipeline as `screenshot_window`.
Only works for windows tracked by the engine (unlike `screenshot_window`).

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `window_hwnd` | `integer` | yes | Must be tracked by wiri |
| `path` | `string \| null` | no | Output PNG path; null = `screenshot-path` setting |

Response `result.path`: resolved file path, or null for clipboard-only (`screenshot-path null`).

---

#### `get_focus`

Return the currently focused window, workspace, and monitor. No fields.

Response `result`:

| Field | Type | Description |
|-------|------|-------------|
| `window_hwnd` | `integer \| null` | Focused HWND, or null |
| `workspace_id` | `integer` | Active workspace id on the focused monitor |
| `monitor_id` | `integer (u64)` | Engine-internal `OutputId` |

---

### 4.2 Workspace Management

#### `workspace_list`

List workspaces (one per active monitor). No fields.

#### `workspace_create`

Create a workspace.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | `integer \| null` | no | Requested id; null = auto-assign |

#### `workspace_delete`

Delete a workspace by id.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | `integer` | yes | Workspace id to delete |

#### `switch_workspace`

Switch the focused monitor to a workspace by id.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | `integer` | yes | Target workspace id (1–9 typical) |

#### `focus_workspace_next`

Switch to the next workspace (wraps). No fields.

#### `focus_workspace_previous`

Switch to the previous workspace (wraps). No fields.

#### `focus_workspace_named`

Switch to a workspace by name from config.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | `string` | yes | Workspace name as defined in config |

#### `get_workspace_list`

List all workspaces across all monitors with ids, names, and window counts.
Response `result.workspaces`: array of `{id, name, window_count}`.

#### `rename_workspace`

Rename a workspace by id.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `workspace_id` | `integer` | yes | Workspace to rename |
| `name` | `string` | yes | New name |

#### `set_workspace_layout`

Set the active workspace's layout mode.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `mode` | `string` | yes | One of: `"scrolling"` (default), `"bstack"`, `"spiral"` |

---

### 4.3 Column and Tile Operations

| Request type | Fields | Description |
|---|---|---|
| `center_column` | — | Center the focused column on screen |
| `set_column_width` | `preset: string` | Set width: `"1/3"`, `"1/2"`, `"2/3"`, `"full"`, `"cycle"` |
| `resize_column` | `delta_px: i32` | Resize focused column by ±N pixels |
| `grow_column` | — | Grow focused column width by 5% of work area |
| `shrink_column` | — | Shrink focused column width by 5% of work area |
| `expand_column` | — | Expand focused column to fill remaining work area |
| `maximize_column` | — | Toggle maximize (full height) for focused column |
| `grow_tile` | — | Grow focused tile height by 5% of its column |
| `shrink_tile` | — | Shrink focused tile height by 5% of its column |
| `consume_window` | — | Pull the tile from the column on the right into the focused column |
| `expel_window` | — | Eject the focused tile into a new column to the right |

---

### 4.4 Monitor and Multi-Monitor Operations

| Request type | Fields | Description |
|---|---|---|
| `move_window_to_monitor` | `direction: string` | Move focused window to monitor on `"left"` or `"right"` |
| `move_column_to_monitor` | `direction: string` | Move focused column to monitor on `"left"` or `"right"` |
| `move_window_to_workspace` | `workspace_id: i32` | Move focused window to workspace |
| `move_column_to_workspace_up` | — | Move focused column to the workspace above |
| `move_column_to_workspace_down` | — | Move focused column to the workspace below |
| `move_workspace_up` | — | Swap focused workspace with the one above |
| `move_workspace_down` | — | Swap focused workspace with the one below |
| `monitor_list` | — | Enumerate all monitors with bounds, DPI, workspace info |

`monitor_list` response `result`: array of `MonitorInfo`:

| Field | Type | Description |
|-------|------|-------------|
| `output_id` | `u64` | Deterministic 64-bit name hash |
| `name` | `string` | The Windows device name of the monitor (`\\.\DISPLAY1`): what `open-on-output` in a window rule and `output "name" {}` in the config call it (empty when the engine was not told) |
| `bounds_x/y` | `i32` | Top-left of physical screen rect |
| `bounds_width/height` | `u32` | Physical screen size in pixels |
| `work_area_x/y/width/height` | mixed | Work area excluding taskbar |
| `scale_factor` | `f64` | DPI scale (1.0 = 96 DPI) |
| `active_workspace` | `i32` | Currently displayed workspace id |
| `window_count` | `usize` | Total tiled windows on this monitor |
| `focused` | `bool` | True for the monitor receiving new windows |

---

### 4.5 Layout Presets and Snapshots

| Request type | Fields | Description |
|---|---|---|
| `preset_list` | — | List saved layout presets |
| `preset_save` | `name: string` | Save current layout as a named preset |
| `preset_load` | `name: string` | Apply a saved preset |
| `preset_delete` | `name: string` | Delete a saved preset |
| `layout_export` | — | Export the current layout as JSON |
| `save_snapshot` | `name: string` | Save full monitor/workspace/column/tile state to `%APPDATA%\wiri\snapshots\<name>.json` |
| `load_snapshot` | `name: string` | Restore a snapshot; missing HWNDs are skipped |
| `list_snapshots` | — | List all saved snapshot names |
| `delete_snapshot` | `name: string` | Delete a saved snapshot |

---

### 4.6 Focus and Navigation

| Request type | Fields | Description |
|---|---|---|
| `focus_previous` | — | Focus the previously-focused window (MRU history, like Alt+Tab) |
| `toggle_always_on_top` | — | Toggle always-on-top for the focused window |
| `toggle_sticky` | — | Toggle sticky (visible on all workspaces) for focused window |
| `toggle_resize_mode` | — | Enter/exit interactive resize mode |

---

### 4.7 Diagnostics and System

| Request type | Fields | Description |
|---|---|---|
| `get_state` | — | Full engine state: windows + workspaces |
| `subscribe_events` | `event_types: string[]` | Subscribe to live events (see §6) |
| `reload_config` | — | Hot-reload config.kdl |
| `quit` | — | Gracefully shut down the wiri daemon |
| `spawn_command` | `command: string`, `args?: string[]` | Spawn an external process; with `args` the arguments are passed intact, without it `command` is split like a Windows command line (quoted paths with spaces work) and the program is searched on PATH |
| `run_action` (also `action`) | `name: string` (also `action`), `args?: (string \| number \| boolean)[]`, `quoted?: boolean[]` | Run any action by its niri name as if its key were pressed (`wiri-ctl action focus-column-left`, `wiri-ctl action set-column-width +10%`); `args` are the words after the action name in a `binds {}` line; see [§4.8](#48-running-actions) |
| `list_bindings` | — | List all registered Win32 hotkey bindings (raw: id, modifiers, vk_code) |
| `get_bindings` | — | List bindings as human-readable `chord + action` table |
| `set_auto_tile_threshold` | `threshold: usize \| null` | Window count above which auto-tile activates; null = disabled |

---

### 4.8 Running actions

`run_action` is niri's `Request::Action`: any action that can be bound to a key can be run over
the pipe, with the arguments it takes in a `binds {}` line.  The message is also accepted as
`{"type":"action","data":{"action":"..."}}`.

```json
{"type":"run_action","data":{"name":"focus-column-left"}}
{"type":"run_action","data":{"name":"set-column-width","args":["+10%"]}}
{"type":"action","data":{"action":"focus-workspace","args":[2]}}
{"type":"run_action","data":{"name":"focus-workspace","args":["2"],"quoted":[true]}}
{"type":"run_action","data":{"name":"fullscreen-window","args":["--id",328756]}}
```

| Part | Meaning |
|---|---|
| `name` | The niri name of the action (`focus-column-left`, `consume-or-expel-window-left`, `move-column-to-workspace`, ...), or one of wiri's older spellings.  An unknown name is `{"success":false,"error":"unknown action or bad arguments: ..."}` |
| `args` | The words after the name in a `binds {}` line, in order.  A string is a word; a number or a boolean stands for its text (`2` is the same as `"2"`) so that a client in a typed language can send what niri's own arguments are |
| `quoted` | `quoted[i]` is true when `args[i]` was written in double quotes: `focus-workspace 2` is the second workspace, `focus-workspace "2"` the workspace named "2".  Omitted: none |
| `--id <hwnd>` (`--id=<hwnd>`; `--window-id` where niri spells it so) | niri's way to name the window to act on.  The window is the `id` of a window object (its HWND).  `focus-window --id <hwnd>` focuses it, `close-window --id <hwnd>` closes it and leaves the focus alone; **any other action focuses the window first** and then acts on it (niri leaves the focus where it is: an approximation).  An id that is not a managed window is an error |

`wiri-ctl action <name> [args...]` sends the same request.  A word in literal double quotes is a
`quoted` word (`wiri-ctl action focus-workspace '"2"'` in a shell that keeps the inner quotes).

Success is `{"success":true}`: the action ran as if its key had been pressed.  Whatever it changed
(the focus, the workspaces, the windows) reaches `subscribe_events` clients as events.

---

### 4.9 niri's request/reply forms

Five requests answer with **niri's** shapes, not wiri's own: the same objects the
event stream describes (niri's `Workspace` and `Window`), so that a client written
against niri reads them without knowing wiri.  They are answered from the engine's
current state, with the same code that builds the state a new subscriber of the
event stream is told first.

Each of them takes no fields.

| Request | Wire name | `result` |
|---|---|---|
| `Workspaces` | `workspaces` | `{"workspaces":[ ...WorkspaceIpc... ]}` |
| `Windows` | `windows` | `{"windows":[ ...WindowIpc... ]}` |
| `FocusedWindow` | `focused_window` | `{"focused_window": ...WindowIpc... \| null}` |
| `OverviewState` | `overview_state` | `{"is_open":true\|false}` |
| `Version` | `version` | `{"version":"0.1.0 (commit 1234567)"}` |

niri's own spelling of the request (`"Workspaces"`, `"Windows"`,
`"FocusedWindow"`, `"OverviewState"`, `"Version"`) is read as well.

The workspace objects are niri's `Workspace`:

| Field | Type | Description |
|---|---|---|
| `id` | `integer` | Unique among the workspaces of all monitors, small enough for a JavaScript number.  The same number the event stream uses |
| `idx` | `integer` | The number of the workspace on its monitor, counted from 1 at the top: what `focus-workspace 2` means |
| `name` | `string \| null` | The name from the config; **null** when the workspace has none |
| `output` | `string \| null` | The device name of the monitor (`\\.\DISPLAY1`); **null** when the engine was not told |
| `is_active` | `boolean` | The workspace is the active one of its monitor |
| `is_focused` | `boolean` | The workspace is the active one of the focused monitor |
| `active_window_id` | `integer \| null` | The window that is the active one of the workspace; **null** when it has none |

The window objects are niri's `Window`, plus what wiri adds:

| Field | Type | Description |
|---|---|---|
| `id` | `integer` | The window handle (HWND) as a number |
| `title` | `string \| null` | The title; **null** while the window has none |
| `app_id` | `string \| null` | The file name of the window's program (`WindowsTerminal.exe`): the closest thing Windows has to a Wayland `app_id`.  **null** when it cannot be read (an elevated program) |
| `pid` | `integer \| null` | The process id; **null** when it is unknown |
| `workspace_id` | `integer \| null` | The workspace the window is on; **null** for a floating window whose monitor is not connected |
| `is_focused` | `boolean` | The window has the keyboard focus |
| `is_floating` | `boolean` | The window floats instead of being tiled |
| `is_urgent` | `boolean` | The window asks for attention |
| `layout` | `object` | `{"pos_in_scrolling_layout":[column,tile]}` (both counted from 1), **null** for a floating or minimized window |
| `class_name` | `string` | The Win32 window class (wiri's addition) |
| `output` | `string \| null` | The device name of the monitor the window is on (wiri's addition) |
| `is_fullscreen` | `boolean` | The window covers its monitor (wiri's addition) |
| `is_maximized` | `boolean` | Maximized to the edges of the work area (wiri's addition) |
| `is_minimized` | `boolean` | The window is minimized (wiri's addition) |

Two niri fields have no Windows meaning and are therefore **not** reported:
`focus_timestamp` (when the window was focused — Windows has no such stamp) and
`focus_timestamp`-like timing of the overview.  Everything else that Windows
cannot tell is `null`, never missing.

**Examples**

```json
{"type":"workspaces"}
```
```json
{"success":true,"result":{"workspaces":[{"id":4294967297,"idx":1,"name":null,"output":"\\\\.\\DISPLAY1","is_urgent":false,"is_active":true,"is_focused":true,"active_window_id":328756}]}}
```

```json
{"type":"windows"}
```
```json
{"success":true,"result":{"windows":[{"id":328756,"title":"Terminal","app_id":"WindowsTerminal.exe","pid":4242,"workspace_id":4294967297,"is_focused":true,"is_floating":false,"is_urgent":false,"layout":{"pos_in_scrolling_layout":[1,1]},"class_name":"CASCADIA_HOSTING_WINDOW_CLASS","output":"\\\\.\\DISPLAY1","is_fullscreen":false,"is_maximized":false,"is_minimized":false}]}}
```

```json
{"type":"focused_window"}
```
```json
{"success":true,"result":{"focused_window":{"id":328756,"title":"Terminal","app_id":"WindowsTerminal.exe","pid":4242,"workspace_id":4294967297,"is_focused":true,"is_floating":false,"is_urgent":false,"layout":{"pos_in_scrolling_layout":[1,1]},"class_name":"CASCADIA_HOSTING_WINDOW_CLASS","output":"\\\\.\\DISPLAY1","is_fullscreen":false,"is_maximized":false,"is_minimized":false}}}
```

```json
{"type":"overview_state"}
```
```json
{"success":true,"result":{"is_open":false}}
```

```json
{"type":"version"}
```
```json
{"success":true,"result":{"version":"0.1.0 (commit 1234567)"}}
```

The `wiri-ctl` subcommands are `wiri-ctl workspaces`, `wiri-ctl windows`,
`wiri-ctl focused-window`, `wiri-ctl overview-state` and `wiri-ctl version`;
each of them prints the readable form, and `--json` prints the raw reply.

---

## 5. Response Shape

### 5.1 Success

```json
{
  "success": true,
  "result": { ... }
}
```

`result` is request-specific. Many action requests return `{}` or omit `result`
entirely on success.

### 5.2 Error

```json
{
  "success": false,
  "error": "human-readable description"
}
```

`wiri-ctl` exits with status 1 when `success` is `false`.

### 5.3 Subscription ack

Sent immediately after `subscribe_events` before any event data:

```json
{
  "success": true,
  "subscription_id": 1
}
```

`subscription_id` is currently always `1`; reserved for future multiplexed
subscriptions.  The state follows the ack ([§6.2](#62-the-state-first-niris-contract)).

---

## 6. Event Types

Events are emitted by the daemon whenever the relevant state changes. They are
broadcast to **all** connected `SubscribeEvents` subscribers simultaneously.

The daemon keeps a 64-event in-memory broadcast buffer per subscriber. A client
that falls 64 events behind is told the state again (the events of
[§6.2](#62-the-state-first-niris-contract), as when it subscribed) and goes on from
the current position, so what it knows is right again; it is not disconnected.

### 6.1 Filtering

Pass a non-empty `event_types` list to receive only named types.
An empty list (or `["*"]`) receives all events:

```json
{"type":"subscribe_events","data":{"event_types":[]}}
{"type":"subscribe_events","data":{"event_types":["workspace_switched","window_focused"]}}
```

The filter applies to the state events of §6.2 as well.

### 6.2 The state first (niri's contract)

wiri's event stream follows niri's: **a client that subscribes is told the whole state first and then
what changes, so it never has to ask.**  Right after the ack the stream carries, in this order,

1. `workspaces_changed` - every workspace of every monitor,
2. `windows_changed` - every window wiri manages (`is_focused` says which one has the keyboard focus),
3. `overview_opened_or_closed` - whether the overview is open,
4. `config_loaded` - how the config loaded at startup or the last time it was reloaded,

and then the deltas.  A client keeps its model by replacing the workspaces on `workspaces_changed`
and the windows on `windows_changed`, and applying the other events to them:

| Event | What the client does |
|---|---|
| `workspaces_changed` | replaces the list of workspaces (it carries the flags too) |
| `workspace_activated` `{id, focused}` | the workspace is the active one of its monitor (the one before it on that monitor is not); when `focused`, it is the only focused workspace |
| `workspace_active_window_changed` `{workspace_id, active_window_id}` | sets the workspace's `active_window_id` |
| `workspace_urgency_changed` `{id, urgent}` | sets the workspace's `is_urgent` |
| `windows_changed` | replaces the list of windows |
| `window_opened_or_changed` `{window}` | adds the window or replaces it by `id`; if `window.is_focused`, no other window is focused |
| `window_closed` `{id}` | removes the window |
| `window_focus_changed` `{id}` | `id` is the only focused window (`null`: none) |
| `window_urgency_changed` `{id, urgent}` | sets the window's `is_urgent` |
| `window_layouts_changed` `{changes}` | sets `layout` of the windows named |
| `overview_opened_or_closed` `{is_open}` | the overview is open or not |

The events are derived from the state of the layout engine, not from what happened to produce it:
whatever changes a window or workspace - a key, `wiri-ctl`, the mouse, an application, a config reload -
produces the same events, once, after the change is complete.  Nothing is computed while nobody is
subscribed.

The window object and the workspace object are niri's `Window` and `Workspace`, in snake_case, with
what wiri adds:

**Window**

| Field | Type | Description |
|---|---|---|
| `id` | `u64` | The window handle (HWND) as a number; what `--id` takes in `run_action`.  A handle can be reused by Windows once the window is gone |
| `title` | `string` or `null` | The title; `null` while the window has none |
| `app_id` | `string` or `null` | File name of the window's program (`WindowsTerminal.exe`): the closest thing Windows has to a Wayland `app_id`, and what the `app-id` of a window rule matches first.  `null` when it cannot be read (an elevated program) |
| `pid` | `u32` or `null` | Process id |
| `workspace_id` | `u64` or `null` | The workspace the window is on (`Workspace.id`); a floating or minimized window is on the workspace it floats on or comes back to |
| `is_focused` | `bool` | The window has the keyboard focus (wiri manages the window the OS says is in the foreground) |
| `is_floating` | `bool` | |
| `is_urgent` | `bool` | The window asked for attention |
| `layout.pos_in_scrolling_layout` | `[column, tile]` or `null` | Both counted from 1; `null` for a floating or minimized window.  niri's sizes and offsets are not reported |
| `class_name` | `string` | wiri: the Win32 window class |
| `output` | `string` or `null` | wiri: device name of the monitor the window is on (`\\.\DISPLAY1`) |
| `is_fullscreen`, `is_maximized`, `is_minimized` | `bool` | wiri: fullscreen; maximized to the edges of the work area (`maximize-window-to-edges`); minimized |

**Workspace**

| Field | Type | Description |
|---|---|---|
| `id` | `u64` | Unique among all workspaces of all monitors and the same for as long as the workspace exists (the workspaces of an unplugged monitor get new ids on the monitor that takes them).  Always below 2^53, so a JavaScript number holds it |
| `idx` | `u32` | The number of the workspace on its monitor, counted from 1 at the top: what `focus-workspace 2` means |
| `name` | `string` or `null` | |
| `output` | `string` or `null` | Device name of the monitor the workspace is on |
| `is_urgent` | `bool` | A window of the workspace is urgent |
| `is_active` | `bool` | The active workspace of its monitor |
| `is_focused` | `bool` | The active workspace of the focused monitor |
| `active_window_id` | `u64` or `null` | The window that is the active one of the workspace |

**Screenshot completion (Phase 2C Lane D):** after a successful clipboard transfer,
`{"type":"screenshot_captured","data":{"path":"C:\\Shots\\image.png"}}` reports the saved PNG,
or `path: null` for clipboard-only or a disk-save failure. It is a change event, not replayed
initial state, and can be selected with `events --stream --filter screenshot_captured`.
Hotkeys, tray and both window-capture requests share the event path.

Differences from niri: the envelope (`{"type","data"}`, not niri's externally tagged enum), the extra
fields marked wiri above, no `WindowFocusTimestampChanged`, keyboard layout, cast and touchpad
events (they have no Windows counterpart yet), and ids that are window handles.  The events of wiri's own
that predate this contract (`window_opened`, `window_focused`, `workspace_switched`, `column_focused`,
`window_moved`, ...) are kept as they were.

---

### `workspaces_changed`

niri's `WorkspacesChanged`: the workspaces were created, removed, renamed, moved to another monitor or
reordered.  The full list replaces what the client knew.  Also part of the state a new subscriber is told
first (§6.2).

```json
{
  "type": "workspaces_changed",
  "data": {
    "workspaces": [
      {"id": 4294967296, "idx": 1, "name": null,  "output": "\\\\.\\DISPLAY1", "is_urgent": false,
       "is_active": true,  "is_focused": true,  "active_window_id": 328756},
      {"id": 4294967297, "idx": 2, "name": "dev", "output": "\\\\.\\DISPLAY1", "is_urgent": false,
       "is_active": false, "is_focused": false, "active_window_id": null}
    ]
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `workspaces` | `Workspace[]` | Every workspace of every monitor, monitors in the order of their device names, the workspaces of each from the top |

**When:** a workspace is created, deleted (an empty one vanishes when it is left), renamed, moved or
reordered; a monitor comes or goes.  Before 1.1 this event was declared with a different payload and
never sent.

---

### `workspace_urgency_changed`

niri's `WorkspaceUrgencyChanged`: `{"id": 4294967296, "urgent": true}`.  **When:** a window of the
workspace asked for attention, or the last one that did stopped.

---

### `workspace_active_window_changed`

niri's `WorkspaceActiveWindowChanged`: `{"workspace_id": 4294967296, "active_window_id": 328756}`
(`null` when there is none).  **When:** the window that is the active one of a workspace changed.

---

### `windows_changed`

niri's `WindowsChanged`: `{"windows": [Window, ...]}`, every window wiri manages by ascending `id`.
Replaces what the client knew.  **When:** as part of the state a new subscriber is told first (§6.2).

---

### `window_opened_or_changed`

niri's `WindowOpenedOrChanged`: `{"window": Window}`.

**When:** a window appeared, or its `title`, `app_id`, `workspace_id`, `is_floating`, `is_fullscreen`,
`is_maximized`, `is_minimized` changed.  Focus, urgency and the place in the strip have events of their
own; the window object always shows their current value.  wiri's own `window_opened` is sent as well
when a window is added.

---

### `window_focus_changed`

niri's `WindowFocusChanged`: `{"id": 328756}`, or `{"id": null}` when no window that wiri manages has the
keyboard focus (the Start menu, an elevated program).  **When:** the window that has the focus changed.

---

### `window_urgency_changed`

niri's `WindowUrgencyChanged`: `{"id": 328756, "urgent": true}`.  **When:** the window's urgency changed.
wiri's `window_urgent` is sent as well when Windows reports the attention request.

---

### `window_layouts_changed`

niri's `WindowLayoutsChanged`: `{"changes": [[328756, {"pos_in_scrolling_layout": [2, 1]}], ...]}`.
**When:** windows changed their column or their place in it (a column moved, a window was consumed into
or expelled from a column, a column before them closed).

---

### `overview_opened_or_closed`

niri's `OverviewOpenedOrClosed`: `{"is_open": true}`.  **When:** the overview was opened or closed.
Also part of the state a new subscriber is told first (§6.2).

---

### `window_opened`

Emitted when a new window is tiled by the engine.

```json
{
  "type": "window_opened",
  "data": {
    "window": {
      "hwnd": 328756,
      "title": "Visual Studio Code",
      "class_name": "Chrome_WidgetWin_1",
      "process_id": 4982,
      "x": 0, "y": 0, "width": 960, "height": 1040
    }
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `window.hwnd` | `integer` | Win32 HWND |
| `window.title` | `string` | Window title at open time |
| `window.class_name` | `string` | Win32 class name |
| `window.process_id` | `u32` | PID of owning process |
| `window.x/y` | `i32` | Initial position in screen pixels |
| `window.width/height` | `u32` | Initial size in physical pixels |

**When:** a new window is added to the tiling engine layout.

---

### `window_closed`

niri's `WindowClosed`: a window that wiri managed is gone.

```json
{"type":"window_closed","data":{"window_hwnd":328756,"id":328756}}
```

| Field | Type | Description |
|-------|------|-------------|
| `window_hwnd` | `integer` | HWND of the closed window |
| `id` | `u64` | The same number, under niri's name (new in 1.1) |

**When:** the window is destroyed or taken out of the layout.  Sent for the windows wiri managed (before
1.1 it was sent for every window that was destroyed, managed or not).

---

### `workspace_activated`

niri's `WorkspaceActivated`: a workspace became the active one of its monitor.

```json
{"type":"workspace_activated","data":{"id":4294967297,"focused":true}}
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | `u64` | `Workspace.id` of the workspace |
| `focused` | `bool` | The workspace is now the focused one (the active workspace of the focused monitor): no other workspace is focused any more, but each stays active on its own monitor |

**When:** a workspace is activated on any monitor (a key, `wiri-ctl`, the mouse wheel, a window that was
asked for), and when the focus moves to another monitor (`focused: true` for the active workspace of that
monitor).  Before 1.1 this event was declared with a different payload and never sent.  wiri's
`workspace_switched` (below) goes on being sent for the focused monitor.

---

### `workspace_switched`

Emitted when the active workspace on a monitor changes.

```json
{"type":"workspace_switched","data":{"workspace_id":2,"monitor_id":17629372411}}
```

| Field | Type | Description |
|-------|------|-------------|
| `workspace_id` | `i32` | New active workspace id |
| `monitor_id` | `u64` | Engine `OutputId` of the monitor that switched |

**When:** any workspace switch occurs, including via hotkey, IPC, or scroll.

---

### `window_focused`

Emitted when keyboard focus moves to a different window.

```json
{"type":"window_focused","data":{"window_hwnd":328756}}
```

| Field | Type | Description |
|-------|------|-------------|
| `window_hwnd` | `integer` | HWND of the newly focused window |

**When:** `WM_SETFOCUS` / foreground-window change is detected by the engine.

---

### `config_loaded`

Emitted when the configuration file is reloaded (niri's `ConfigLoaded { failed }`).

```json
{"type":"config_loaded","data":{"config_path":"C:\\Users\\user\\AppData\\Roaming\\wiri\\config.kdl","failed":false,"error":null,"warnings":2}}
{"type":"config_loaded","data":{"config_path":"C:\\Users\\user\\AppData\\Roaming\\wiri\\config.kdl","failed":true,"error":"C:\\Users\\user\\AppData\\Roaming\\wiri\\config.kdl:12:5: error: unknown option 'gapz' in 'layout' (did you mean 'gaps'?)","warnings":0}}
```

| Field | Type | Description |
|-------|------|-------------|
| `config_path` | `string` | Absolute path of the config file |
| `failed` | `bool` | True when the file has errors: it was **not** applied and the last good config stays in use |
| `error` | `string` or `null` | When `failed`: `file:line:column: error: message` of the first error, with `(+N more)` when there are others; `file` is the included file when the error is in one |
| `warnings` | `number` | How many things in the file wiri ignores (niri options it lacks, binds that cannot work on Windows) |

**When:** the file watcher saw the config (or a file it includes) change and 200 ms of quiet passed,
the `reload_config` request was processed, or **Reload Config** was chosen in the tray menu.  A tray
balloon with the same text is shown when `failed` is true.  Also the last thing of the state a new
subscriber is told first (§6.2): how the config loaded at startup, or the last reload since.  After a
config with errors at startup it is `failed: true` and wiri runs on its defaults.

---

### `monitor_changed`

Emitted when the focused output (monitor) changes.

```json
{"type":"monitor_changed","data":{"monitor_name":"DELL P2723QE"}}
```

| Field | Type | Description |
|-------|------|-------------|
| `monitor_name` | `string` | OS display name of the newly focused monitor |

**When:** focus moves to a window on a different monitor.

---

### `window_moved`

Emitted when a tiled window is repositioned by the layout engine.

```json
{
  "type": "window_moved",
  "data": {"window_hwnd":328756,"x":960,"y":0,"width":960,"height":1040}
}
```

| Field | Type | Description |
|-------|------|-------------|
| `window_hwnd` | `integer` | HWND of the moved window |
| `x` | `i32` | New left edge in screen pixels |
| `y` | `i32` | New top edge in screen pixels |
| `width` | `u32` | New client width in physical pixels |
| `height` | `u32` | New client height in physical pixels |

**When:** a layout pass repositions a tiled window (scroll, column resize, workspace switch, etc.).

---

### `window_resized`

Emitted when a tiled window's dimensions change.

```json
{"type":"window_resized","data":{"window_hwnd":328756,"width":640,"height":1040}}
```

| Field | Type | Description |
|-------|------|-------------|
| `window_hwnd` | `integer` | HWND |
| `width` | `u32` | New width in physical pixels |
| `height` | `u32` | New height in physical pixels |

**When:** width or height differs from the previous layout pass for this window.

---

### `window_move_resize_start`

Emitted when the user starts an interactive move/resize drag.

```json
{"type":"window_move_resize_start","data":{"window_hwnd":328756}}
```

**When:** `WM_ENTERSIZEMOVE` is received for a tracked window.

---

### `window_move_resize_end`

Emitted when the interactive drag finishes.

```json
{"type":"window_move_resize_end","data":{"window_hwnd":328756}}
```

**When:** `WM_EXITSIZEMOVE` is received.

---

### `window_urgent`

Emitted when a window's urgency / attention-request state changes.

```json
{"type":"window_urgent","data":{"window_hwnd":328756,"urgent":true}}
```

| Field | Type | Description |
|-------|------|-------------|
| `window_hwnd` | `integer` | HWND |
| `urgent` | `boolean` | `true` = urgent; `false` = urgency cleared |

**When:** the application requests user attention (flashing taskbar entry or equivalent).

---

### `column_focused`

Emitted when the focused column changes within the layout.

```json
{"type":"column_focused","data":{"window_hwnd":328756,"column_index":2,"workspace_id":1}}
```

| Field | Type | Description |
|-------|------|-------------|
| `window_hwnd` | `integer` | HWND of the window that is now focused |
| `column_index` | `usize` | Zero-based index of the focused column within the workspace |
| `workspace_id` | `i32` | Workspace the column belongs to |

**When:** the foreground window changes and the new window is in a different column.

---

## 7. Code Examples

### 7.1 Rust — one-shot query

```rust
use std::io::{Read, Write};
use std::fs::OpenOptions;

fn main() {
    // The pipe of this user in this session; `WIRI_PIPE` overrides the name.
    let path = wiri::ipc::pipe_path();
    let mut pipe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("wiri not running");

    let req = r#"{"type":"get_state"}"#;
    pipe.write_all(req.as_bytes()).unwrap();
    pipe.flush().unwrap();

    let mut buf = Vec::new();
    pipe.read_to_end(&mut buf).unwrap();

    let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
    println!("{}", serde_json::to_string_pretty(&v).unwrap());
}
```

---

### 7.2 Rust — event subscriber

```rust
use std::io::Write;
use std::fs::OpenOptions;

fn main() {
    let path = wiri::ipc::pipe_path();
    let mut pipe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("wiri not running");

    let sub = r#"{"type":"subscribe_events","data":{"event_types":[]}}"#;
    pipe.write_all(sub.as_bytes()).unwrap();
    pipe.flush().unwrap();

    // Stream JSON objects back-to-back; serde_json handles framing.
    let de = serde_json::Deserializer::from_reader(pipe).into_iter::<serde_json::Value>();
    for item in de {
        match item {
            Ok(v) => {
                // Skip the subscription ack.
                if v.get("subscription_id").is_some() { continue; }
                println!("{}", v);
            }
            Err(e) => { eprintln!("stream ended: {}", e); break; }
        }
    }
}
```

---

### 7.3 Python — one-shot query (pywin32)

```python
import ctypes
import getpass
import json
import os
import re
import win32file
from ctypes import wintypes


def session_id():
    """The session this process runs in (1 is the console session)."""
    kernel32 = ctypes.WinDLL('kernel32', use_last_error=True)
    sid = wintypes.DWORD()
    if not kernel32.ProcessIdToSessionId(kernel32.GetCurrentProcessId(), ctypes.byref(sid)):
        raise ctypes.WinError(ctypes.get_last_error())
    return sid.value


# The pipe of this user in this session; WIRI_PIPE overrides the name.
user = re.sub(r'[^A-Za-z0-9._-]', '_', getpass.getuser())
PIPE = os.environ.get('WIRI_PIPE') or r'\\.\pipe\wiri_control_%s_%d' % (user, session_id())

handle = win32file.CreateFile(
    PIPE,
    win32file.GENERIC_READ | win32file.GENERIC_WRITE,
    0, None, win32file.OPEN_EXISTING, 0, None
)

win32file.WriteFile(handle, json.dumps({"type": "get_state"}).encode())

buf = b''
while True:
    try:
        _, chunk = win32file.ReadFile(handle, 4096)
        if not chunk:
            break
        buf += chunk
    except Exception:
        break

print(json.dumps(json.loads(buf), indent=2))
win32file.CloseHandle(handle)
```

---

### 7.4 Python — event subscriber (pywin32)

```python
import win32file
import json
import os

# The same name as in 7.3 (WIRI_PIPE, or wiri_control_<user>_<session>).
PIPE = os.environ.get('WIRI_PIPE') or r'\\.\pipe\wiri_control_alice_1'

handle = win32file.CreateFile(
    PIPE,
    win32file.GENERIC_READ | win32file.GENERIC_WRITE,
    0, None, win32file.OPEN_EXISTING, 0, None
)

req = {"type": "subscribe_events", "data": {"event_types": []}}
win32file.WriteFile(handle, json.dumps(req).encode())

buf = b''
while True:
    try:
        _, chunk = win32file.ReadFile(handle, 4096)
        buf += chunk
        # Events arrive back-to-back; split on newlines the daemon may insert,
        # or use the serde streaming decoder approach in Rust for robustness.
        for line in buf.split(b'\n'):
            line = line.strip()
            if not line:
                continue
            try:
                event = json.loads(line)
                if 'subscription_id' in event:
                    continue  # skip ack
                print(json.dumps(event))
            except json.JSONDecodeError:
                pass  # partial read; wait for next chunk
        buf = b''
    except KeyboardInterrupt:
        break
    except Exception as e:
        print(f"stream ended: {e}")
        break

win32file.CloseHandle(handle)
```

---

### 7.5 PowerShell — one-shot query

```powershell
# The pipe of this user in this session; WIRI_PIPE overrides the name.
$user = ($env:USERNAME -replace '[^A-Za-z0-9._-]', '_')
$session = (Get-Process -Id $PID).SessionId
$pipeName = if ($env:WIRI_PIPE) {
    $env:WIRI_PIPE -replace '^\\\\\.\\pipe\\', ''
} else {
    "wiri_control_${user}_${session}"
}

$pipe = [System.IO.Pipes.NamedPipeClientStream]::new(
    '.', $pipeName,
    [System.IO.Pipes.PipeDirection]::InOut
)
$pipe.Connect(5000)   # 5-second timeout

$writer = [System.IO.StreamWriter]::new($pipe)
$writer.AutoFlush = $true
$writer.Write('{"type":"get_state"}')

$reader = [System.IO.StreamReader]::new($pipe)
$response = $reader.ReadToEnd()

$pipe.Dispose()
$response | ConvertFrom-Json | ConvertTo-Json -Depth 10
```

---

### 7.6 PowerShell — event subscriber

```powershell
# The same name as in 7.5.
$user = ($env:USERNAME -replace '[^A-Za-z0-9._-]', '_')
$session = (Get-Process -Id $PID).SessionId
$pipeName = if ($env:WIRI_PIPE) {
    $env:WIRI_PIPE -replace '^\\\\\.\\pipe\\', ''
} else {
    "wiri_control_${user}_${session}"
}

$pipe = [System.IO.Pipes.NamedPipeClientStream]::new(
    '.', $pipeName,
    [System.IO.Pipes.PipeDirection]::InOut
)
$pipe.Connect(5000)

$writer = [System.IO.StreamWriter]::new($pipe)
$writer.AutoFlush = $true
$writer.Write('{"type":"subscribe_events","data":{"event_types":[]}}')

$reader = [System.IO.StreamReader]::new($pipe)
Write-Host "Subscribed. Press Ctrl+C to exit." -ForegroundColor Cyan

try {
    while ($true) {
        $line = $reader.ReadLine()
        if ($null -eq $line) { break }
        $obj = $line | ConvertFrom-Json -ErrorAction SilentlyContinue
        if ($obj -and -not $obj.subscription_id) {
            Write-Host ($obj | ConvertTo-Json -Compress)
        }
    }
} finally {
    $pipe.Dispose()
}
```

---

### 7.7 `wiri-ctl` — streaming from the command line

```
# Pretty-print events (default human format):
wiri-ctl events

# Raw newline-delimited JSON (pipe-friendly):
wiri-ctl events --stream

# Filter to specific types:
wiri-ctl events --stream --filter workspace_switched,window_focused

# Pipe to jq:
wiri-ctl events --stream | jq -r '.type + " " + (.data | tostring)'
```

---

## 8. Versioning Policy

The schema is currently at **version 1.2**.  1.2 (2026-10-25) adds niri's request/reply forms
(§4.9: `workspaces`, `windows`, `focused_window`, `overview_state` and `version`, each answering with
niri's own shapes) and the pipe name of the user and the session
(`\\.\pipe\wiri_control_<user>_<session>`, `WIRI_PIPE` overrides it).

1.1 (2026-10-25) adds niri's event stream (§6.2: the
state first, then `windows_changed`, `window_opened_or_changed`, `window_focus_changed`,
`window_urgency_changed`, `window_layouts_changed`, `workspace_urgency_changed`,
`workspace_active_window_changed`, `overview_opened_or_closed`), the `id` of `window_closed`, the
`action` spelling, typed arguments, `quoted` and `--id` of `run_action`, and the `fullscreen` and
`maximized` fields of `window_list`.  The payloads of `workspaces_changed` and `workspace_activated`
were replaced; both were declared before and never sent, so no client could have depended on them.

Version strings follow `MAJOR.MINOR`:

| Change type | Version bump | Example |
|---|---|---|
| New request type | Minor (1.x) | Adding `set_brightness` |
| New optional field on existing request/response | Minor (1.x) | Adding `result.pid` |
| New event type | Minor (1.x) | Adding `workspace_renamed` |
| Removing a field | Major (2.x) | Dropping `window_hwnd` from `window_closed` |
| Renaming a type or field | Major (2.x) | `"window_hwnd"` → `"hwnd"` |
| Changing a field type | Major (2.x) | `string` → `integer` |

Third-party clients should ignore unknown fields and unknown event types to
remain forward-compatible with minor version bumps.

The schema version is reported in `docs/IPC_SCHEMA.json` under `"version"` and
embedded in this document's header.

---

## 9. Common Patterns

### React to workspace changes

Subscribe and watch for `workspace_switched`:

```python
# On each workspace_switched event, run a custom hook.
if event.get("type") == "workspace_switched":
    ws_id = event["data"]["workspace_id"]
    subprocess.run(["my-script.exe", "--workspace", str(ws_id)])
```

### Track the state from the event stream

Subscribe with an empty filter and keep a model: the stream starts with the whole state
(`workspaces_changed`, `windows_changed`, `overview_opened_or_closed`, `config_loaded`) and goes on with
the changes (§6.2), so there is nothing to query first and nothing to poll.

```python
workspaces, windows = {}, {}
for event in events():                      # one JSON object per line
    kind, data = event["type"], event["data"]
    if kind == "workspaces_changed":
        workspaces = {w["id"]: w for w in data["workspaces"]}
    elif kind == "windows_changed":
        windows = {w["id"]: w for w in data["windows"]}
    elif kind == "window_opened_or_changed":
        w = data["window"]
        if w["is_focused"]:
            for other in windows.values():
                other["is_focused"] = False
        windows[w["id"]] = w
    elif kind == "window_closed":
        windows.pop(data["id"], None)
    elif kind == "window_focus_changed":
        for w in windows.values():
            w["is_focused"] = (w["id"] == data["id"])
    elif kind == "workspace_activated":
        active = workspaces[data["id"]]
        for w in workspaces.values():
            if w["output"] == active["output"]:
                w["is_active"] = (w["id"] == active["id"])
            if data["focused"]:
                w["is_focused"] = (w["id"] == active["id"])
```

### Query current state at startup, then track deltas

1. Send `get_state` to get the full current layout.
2. Open a second connection with `subscribe_events` to track changes.
3. Merge events into your local model.

(With wiri 1.1 the first step is not needed: the subscription itself starts with the state.)

### Detect when a specific app opens

```python
if event.get("type") == "window_opened":
    w = event["data"]["window"]
    if "Visual Studio Code" in w["title"]:
        # Move VS Code to workspace 2
        send_ipc({"type": "window_move_to_workspace",
                  "data": {"window_hwnd": w["hwnd"], "workspace_id": 2}})
```

### Trigger a layout snapshot before switching contexts

```bash
wiri-ctl save-snapshot work-session
wiri-ctl switch-workspace 5
```

### Poll-free focus tracking

```powershell
# Print the title of every window that receives focus.
wiri-ctl events --stream --filter window_focused | ForEach-Object {
    $ev  = $_ | ConvertFrom-Json
    $hwnd = $ev.data.window_hwnd
    # Use wiri-ctl or Win32 API to look up the title from HWND.
    Write-Host "Focused: hwnd=$hwnd"
}
```

### `reserve_area` — Reserve screen-edge space for external bars

Requests that wiri subtract the given pixel count from each monitor's tiling
work area on the specified edge.  Useful for status bars (e.g. crest), pinned
panels, or other persistent overlays that need dedicated space that tiles must
not overlap.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `side` | `string` | yes | Which edge: `"top"` \| `"bottom"` \| `"left"` \| `"right"` |
| `pixels` | `integer` | yes | Pixels to reserve on that edge. `0` = release. |
| `monitor_id` | `integer \| null` | no | Monitor ID (currently ignored; applies to all monitors). |

**Request:**
```json
{"type":"reserve_area","data":{"side":"top","pixels":32}}
```

**Response (success):**
```json
{"success":true}
```

**Response (invalid side string):**
```json
{"success":false,"error":"invalid side \"foo\": expected top | bottom | left | right"}
```

**Behaviour notes:**

- The reservation is **in-memory only** — it is not written to `config.kdl` and
  resets when wiri restarts.
- External bars should re-issue `reserve_area` on every wiri reconnect (e.g.
  subscribe to `config_loaded` to detect daemon restarts and reapply).
- Pass `pixels: 0` to release a previous reservation.
- The reserved area is applied **before** the `outer-gaps` inset, so the two
  compose: a 32 px top reservation plus 8 px outer-gaps gives 40 px of clearance
  from the monitor edge to the first tile.
- `monitor_id` is accepted but ignored in v1 — the reservation is global across
  all monitors.  Per-monitor reservations are planned for a future release.

**`wiri-ctl` equivalent:**
```
wiri-ctl reserve-area top 32
wiri-ctl reserve-area top 0     # release
```

---

### aurora / crest integration checklist

1. Open `\\.\pipe\wiri_control_<user>_<session>` with `GENERIC_READ | GENERIC_WRITE`
   (the pipe of this user in this session; `WIRI_PIPE` names another one).
2. Send `subscribe_events` with the event types you care about.
3. Parse the ack (skip if `subscription_id` is present).
4. Read events in a background thread/task; dispatch by `"type"`.
5. For actions (move window, switch workspace, etc.), open a **separate**
   pipe connection per request — do not reuse the subscription pipe.
6. Handle `"success": false` responses gracefully (the daemon may not yet have
   an engine or backend wired up during early startup).
