# Lane H — IPC and CLI completeness (2026-10-25)

Branch: `meylin/sa-4b8f-6b719`, based on `df229cd`. Reference: niri main
`e19df12cf8`. External source and wiki fetches were denied in this task; they
were not retried or bypassed. No other worker's worktree was modified.

## Scope and commits

The lane closes config/input/IPC matrix sections 8 (transport and CLI), 9
(requests) and 10 (events) where Windows and the existing executors allow it.
Commits, oldest first:

- `377d32b` persistent JSON lines, bounded framing, niri's externally tagged
  requests/Ok/Err, recoverable request errors, WIRI_PIPE for children.
- `908386d` reusable newline client and CLI commands, `--json`,
  `--print-request`, raw JSON argument/stdin, schema hint.
- `d481bac` foreground-thread keyboard layouts and shell/input notifications.
- `e25e2bc` interactive Window/Color picks on the existing hooks.
- `de65c07` Outputs/FocusedOutput and validated temporary output requests.
- `e3e749f` preserve connector aliases in the temporary planner.
- `4da25e7` automated H35-H39 live checks.
- `bd3e23b` language notifications on the existing hotkey window; inject a
  temporary layout immediately.
- `323c552` conservative output snapshots and target-only display changes.

The final integration commit contains schema/docs/matrix updates, the H35-H39
harness corrections, the performance collector endpoint fix, and this report.

## Behaviour

**Transport.** A connection may carry repeated and pipelined JSON lines; each
request gets one line. Errors (unknown variant, invalid field, ReturnError,
oversized line) are answered on the same connection and the next line remains
usable. `Framer` is incremental, caps a request at 65,536 bytes and drains an
oversized line to its delimiter rather than growing memory. Old clients that
write one complete undelimited wiri value and read to EOF still work; external
niri clients are JSON-line only. A silent disconnected subscriber is noticed by
the pipe read, not a timer. Wiri replies carry `schema_version: "1.3"`; niri
requests receive exactly one `{"Ok": <externally tagged variant>}` or
`{"Err": "..."}`, and niri events use niri's names/fields. Unknown fields are
ignored, unknown variants are not fatal. `Action` is adapted to the existing
validated action parser; unsupported flags/actions fail explicitly. `Output`
answers `{"Ok":{"OutputConfigChanged":"Applied"}}` after a successful change.

**CLI and library client.** `IpcClient` keeps one connection and exposes
connect, typed request, raw request and `read_events`; legacy helpers remain.
`wiri-ctl` accepts niri's `msg` prefix, per-request commands, `--json`,
`--print-request` (no connection), `raw-request <json>` and raw stdin. Stdin is
read as one JSON value **to EOF**, so a pretty-printed request works; it is not
line-oriented. A missing/mismatched schema prints update/restart guidance on
stderr without altering `--json` or raw replies. `windows` uses niri's shape;
`window-list` keeps wiri's geometry list.

**Outputs and keyboard.** `Outputs` and `FocusedOutput` are read-only views:
case-insensitive connector or EDID make/model/serial identity, physical size,
DXGI rational-rate modes with the current mode index, logical geometry/scale/
transform from the engine, and `null` for VRR/max-bpc that Windows does not
expose reliably. Temporary `Output` mode/position/rotation/on/off requests reuse
the Lane D conservative CCD planner for exactly one connector: validate first,
apply only explicit physical differences, never write config, never change
Windows DPI or choose a primary display. Scale changes the wiri layout only.
Inactive-output enabling, flipped transforms, custom timings/modeline, VRR,
max-bpc and turning the last display off fail closed. Reload discards the
in-memory override. Mutating paths are **synthetic unit tests only**; no display
setting was changed on this laptop. `KeyboardLayouts` enumerates loaded HKLs,
selects the foreground window thread's exact HKL (never the IPC caller thread),
resolves localized registry names with bounds, and caches names until the loaded
list changes.

**Picks.** `PickWindow` and SDR `PickColor` consume the next left click on the
existing low-level input hooks and dispatch thread. The request is asynchronous
outside the normal IPC reply budget; Escape returns null, disconnect cancels,
only one request is accepted, and physical/injected origins have independent
down state so the matching release is consumed without stealing an already-held
button or letting an injected release clear a physical guard. Window picks use
`WindowFromPoint`/`GetAncestor` and the managed snapshot; color picks use a
request-local screen DC and `GetPixel`, returning normalized sRGB. HDR color is
not converted.

**Events.** Window objects now include full logical `layout` data computed
before offscreen parking (`pos_in_scrolling_layout`, tile/window size,
workspace-view position and offset) and a daemon-relative monotonic
`focus_timestamp`; duplicate foreground reports do not restamp, closed handles
are removed, and timestamp/layout deltas are deduplicated. Subscribers receive
`windows_changed`/`workspaces_changed`/overview, then keyboard layouts, then
config. `KeyboardLayoutsChanged` and idx-only `KeyboardLayoutSwitched` are
event-driven from shell language, `WM_INPUTLANGCHANGE` and `WM_SETTINGCHANGE`
messages on the existing hidden hotkey window. No new timer, thread or window
was added.

## Verification

On TRIDARIS (Windows 11 x64, 1366x768, rustc 1.95) all required commands passed
with `RUSTFLAGS=-D warnings` and `-j 2`:

- `cargo check --release --all-targets -j 2`
- `cargo test --release -j 2`: **1037 library + 14 CLI tests**, no failures; one
  ignored documentation example.
- `cargo build --release -j 2`
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\live-test.ps1 -Bin target\release -Out .live -LockWaitMinutes 120`

The shipped schema and every documented example were checked with the
preinstalled `jsonschema` package (no dependency added); schema `1.3` and the
wire contract tests agree. The collector self-test validates the per-user/session
pipe endpoint without starting wiri.

The most recent completed full run reported `366` pass, `0` fail and `75`
information entries; `352` of `353` booleans true. The exception is the
pre-existing `hidden_json_present_after_restart`, expected false. All **29**
H35-H39 checks are accounted for: 28 booleans true, plus
`h37_keyboard_layout_event = null` with
`h37_keyboard_layout_event_skipped = true` (the laptop has one loaded layout and
none was installed system-wide). H9's known terminal overlap is informational
in `live-report.py`: one 11 px pair, rectangles `(892,12,466,704)`,
`(437,12,466,704)`, `(-53,12,466,704)`; the existing H9 booleans pass and no
harness threshold was changed.

A later required invocation with `-Out .live` was attempted but its harness
refused to start because another worker's `wiri.exe` was running; after that
lock/daemon wait expired, the machine was still busy. No foreign process was
stopped and no successful `.live` directory exists for that attempt. The counts
above are from this lane's preserved completed run, not from that refused
attempt. Raw evidence is local/git-ignored.

An earlier completed run had six failures: console-startup detection, one H5
z-order check, two H14 maximize checks and the two H15 replay-order checks. The
H15 failures were real: inserting `keyboard_layouts_changed` made the replay five
events while the assertion still expected four. The harness was corrected to the
five-event order. Console/H5/H14 did not recur in the immediate rerun or the
most recent completed run; their cause was not proven, so they are not claimed
flake-free.

### Live versus unit-tested only

Live on the real desktop: repeated/legacy/niri line connections, ReturnError
recovery, CLI raw argument/stdin and print-request, output/keyboard query
shapes, outputs read-only before/after, focus timestamps and layout deltas,
shell keyboard-layout notification, injected click/Escape consumption by both
picks, pick disconnect cancellation, pixel match, and both CLI window shapes.
Injected input was guarded by owned foreground/target checks.

Unit-tested only: all physical output mutations (mode/position/rotation/off),
multi-output ambiguity/focus and last-display refusal, DPI/parking geometry with
negative origins, HDR color, a real second keyboard layout install/switch, and
a client slower than the existing 1,500 ms response budget.

## Known limits

- niri's full action vocabulary includes layout actions outside this lane;
  unsupported actions/flags return an error instead of a silent no-op.
- Output capabilities/state that Windows does not expose are `null`; inactive
  output enabling, VRR, max-bpc, custom timings and flipped transforms remain
  unsupported.
- `PickColor` is SDR-only. A real keyboard-layout switch was not live-verified
  because installing a system layout would alter the shared machine.
- Request/reply waits are bounded by the existing IPC response budget; the pick
  lifetime itself has no idle timer or polling.

## Added idle cost

No idle timers, polling loops, always-on threads or windows. Picks share the
existing input hooks and dispatch thread; keyboard notifications reuse the
existing hotkey window; queries and temporary output plans are created on
demand. The only new persistent objects are the existing client/subscription
structures while a client is connected.
