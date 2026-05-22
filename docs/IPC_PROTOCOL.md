# wiri IPC Protocol — Integration Reference

**Schema version: 1.0**  
Last updated: 2026-05-21

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
\\.\pipe\wiri_control
```

The protocol is simple:

1. Open the pipe with read+write access.
2. Write a single UTF-8 JSON object (the request) — no length prefix, no framing.
3. Read the response: either a single JSON object (for one-shot requests) or a
   stream of newline-delimited JSON objects (for `subscribe_events`).
4. Close the pipe when done.

For one-shot requests the server closes its end of the pipe after sending the
response, so a `read_to_end` / `ReadFile` loop will see EOF naturally.

For `subscribe_events` the server keeps the pipe open and writes one event JSON
object per line until the client disconnects or the daemon exits.

### Wire summary

| Mode               | Client writes | Server writes            | Pipe lifetime        |
|--------------------|--------------|--------------------------|----------------------|
| One-shot request   | 1 JSON object | 1 JSON object then EOF   | Single round-trip    |
| `subscribe_events` | 1 JSON object | Ack + N newline-delimited JSON events | Until client disconnects |

---

## 2. Security

The named pipe is created with a DACL that limits access to:

| Principal              | Access  | SDDL  |
|------------------------|---------|-------|
| `BUILTIN\Administrators` | Full  | `(A;;GA;;;BA)` |
| Object owner (wiri process user) | Full | `(A;;GA;;;OW)` |

Other local users on a shared workstation **cannot connect** to
`\\.\pipe\wiri_control`. Connections from non-elevated processes running as the
same user succeed because ownership matches.

If wiri cannot build the custom DACL at startup it falls back to the default
pipe security (inherited from the process token), logging a warning.

---

## 3. Message Format

### 3.1 Encoding

- UTF-8, no BOM.
- No length prefix. The server reads until it can parse a complete JSON object.
- Maximum incoming message size: 64 KiB (server-side read buffer).

### 3.2 Serde tagging

Both requests (`IpcMessage`) and events (`IpcEvent`) use serde's
**adjacent tagging** scheme:

```json
{ "type": "<variant_name>", "data": { ...fields... } }
```

For unit variants (no fields), the `"data"` key is omitted.

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

Response `result`: array of `WindowInfo` objects.

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

Capture a window to a BMP file.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `hwnd` | `integer \| null` | no | HWND to capture; null = focused window |
| `path` | `string \| null` | no | Output path; null = Pictures folder default |

Response `result.path`: resolved file path.

---

#### `capture_window`

Capture a wiri-tracked window using `PrintWindow(PW_RENDERFULLCONTENT)`.
Only works for windows tracked by the engine (unlike `screenshot_window`).

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `window_hwnd` | `integer` | yes | Must be tracked by wiri |
| `path` | `string \| null` | no | Output BMP path |

Response `result.path`: resolved file path.

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
| `spawn_command` | `command: string` | Spawn an external process |
| `list_bindings` | — | List all registered Win32 hotkey bindings (raw: id, modifiers, vk_code) |
| `get_bindings` | — | List bindings as human-readable `chord + action` table |
| `set_auto_tile_threshold` | `threshold: usize \| null` | Window count above which auto-tile activates; null = disabled |

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
subscriptions.

---

## 6. Event Types

Events are emitted by the daemon whenever the relevant state changes. They are
broadcast to **all** connected `SubscribeEvents` subscribers simultaneously.

The daemon keeps a 64-event in-memory broadcast buffer per subscriber. If a slow
client falls 64 events behind, missed events are logged and the subscriber
continues from the current position (no disconnect).

### 6.1 Filtering

Pass a non-empty `event_types` list to receive only named types.
An empty list (or `["*"]`) receives all events:

```json
{"type":"subscribe_events","data":{"event_types":[]}}
{"type":"subscribe_events","data":{"event_types":["workspace_switched","window_focused"]}}
```

---

### `workspaces_changed`

Emitted when the set of workspaces changes (create, delete, or rename).

```json
{
  "type": "workspaces_changed",
  "data": {
    "workspaces": [
      {"name": "workspace-1", "id": 1, "window_count": 3},
      {"name": "dev",         "id": 2, "window_count": 1}
    ]
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `workspaces` | `WorkspaceInfo[]` | Full current list of workspaces |

**When:** workspace is created, deleted, or renamed.

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

Emitted when a tiled window is removed.

```json
{"type":"window_closed","data":{"window_hwnd":328756}}
```

| Field | Type | Description |
|-------|------|-------------|
| `window_hwnd` | `integer` | HWND of the closed window |

**When:** the window is destroyed or explicitly removed from the layout.

---

### `workspace_activated`

Emitted when a named workspace becomes active on any monitor.

```json
{"type":"workspace_activated","data":{"workspace":"dev"}}
```

| Field | Type | Description |
|-------|------|-------------|
| `workspace` | `string` | Name of the activated workspace |

**When:** a named workspace is brought to the foreground on any monitor.

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

Emitted when the configuration file is loaded or reloaded.

```json
{"type":"config_loaded","data":{"config_path":"C:\\Users\\user\\AppData\\Roaming\\wiri\\config.kdl"}}
```

| Field | Type | Description |
|-------|------|-------------|
| `config_path` | `string` | Absolute path of the loaded config file |

**When:** wiri starts or `reload_config` IPC is processed.

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
    let mut pipe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(r"\\.\pipe\wiri_control")
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
    let mut pipe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(r"\\.\pipe\wiri_control")
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
import win32file
import json

PIPE = r'\\.\pipe\wiri_control'

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

PIPE = r'\\.\pipe\wiri_control'

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
$pipe = [System.IO.Pipes.NamedPipeClientStream]::new(
    '.', 'wiri_control',
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
$pipe = [System.IO.Pipes.NamedPipeClientStream]::new(
    '.', 'wiri_control',
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

The schema is currently at **version 1.0**.

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

### Query current state at startup, then track deltas

1. Send `get_state` to get the full current layout.
2. Open a second connection with `subscribe_events` to track changes.
3. Merge events into your local model.

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

### aurora / crest integration checklist

1. Open `\\.\pipe\wiri_control` with `GENERIC_READ | GENERIC_WRITE`.
2. Send `subscribe_events` with the event types you care about.
3. Parse the ack (skip if `subscription_id` is present).
4. Read events in a background thread/task; dispatch by `"type"`.
5. For actions (move window, switch workspace, etc.), open a **separate**
   pipe connection per request — do not reuse the subscription pipe.
6. Handle `"success": false` responses gracefully (the daemon may not yet have
   an engine or backend wired up during early startup).
