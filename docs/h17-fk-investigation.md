# FK / H17 investigation (dcf036a)

## Status

This change is diagnostic and makes the setup's focus semantics explicit; it is
**not a demonstrated fix for the official H17 failure**. No desktop harness was
run. The existing vertical-motion gate and K's H54–H57 are unchanged.

## Action trace

* `move-to-workspace 2` parses as the legacy `Action::MoveToWorkspace(2)`,
  **not** `WorkspaceAction::MoveWindow` (`src/input/layout_actions.rs:406–411`).
  Dispatch resolves the visible number to a stable key
  (`src/backend/message_loop.rs:1249–1257`). The compatibility engine method
  passes `focus=false` (`src/layout/engine/workspace_actions.rs:103–121`).
  It does not deliberately switch the view or call `switch_workspace_composited`.
* The different, niri-style `move-window-to-workspace` parses as
  `WorkspaceAction::MoveWindow`, defaults to `focus=true`, and accepts
  `--focus false` (`src/input/layout_actions.rs:407–411,591–607`). Its transfer
  completion calls `focus_action_window` only when focus is requested
  (`src/layout/engine/workspace_actions.rs:411–448`). H17 now explicitly uses
  the no-follow spelling. This is semantically equivalent to its old setup,
  not evidence that F2's default caused the regression.
* `focus-workspace 2` dispatches to `focus_workspace_number`
  (`src/backend/message_loop.rs:1137–1141`), resolves numbering and optional
  auto-back-and-forth (`src/layout/engine.rs:7811–7830`), and calls
  `switch_workspace`, which tries the compositor first (`:3912–3915`).
  H17 does not enable auto-back-and-forth. With the intended setup, this is
  the first workspace switch, not a second transition. If already on the
  target, compositor admission returns false (`transitions.rs:296–298`);
  the ordinary path still reconciles visibility/layout, but does not start
  a workspace slide (`engine.rs:3940–3949`).

## Layer ownership and the ambiguous evidence

`switch_workspace_composited` cancels old transitions before building the new
scene (`src/layout/engine/transitions.rs:301–356`). The pump has one thread-local
`Option<Host>` (`src/overlay/compositor/win.rs:33`). Starts reuse that host; End
removes a scene, and an empty host is dropped in the same pump turn
(`:77–114`). `Host::drop` destroys the HWND (`:545–553`). Multiple effects may
share the host, but this path cannot create two simultaneous hosts on its pump.
It does not leave an empty finished host visible. This does not rule out a host
from another process: H17's enumeration is desktop-wide, without PID filtering.

**The supplied `thumbnail_y=0` does not establish that the visible-layer count
was other than one.** `Host::paint` also initializes incoming Y to zero and
updates it only for admitted incoming workspace tiles (`win.rs:417–436`), then
publishes it (`:523–528`). Engine collection excludes missing/offscreen geometry
(`transitions.rs:236–260`); native admission excludes failed captures and
registrations (`win.rs:339–385`). An outgoing-only scene can therefore move
while IncomingY is zero. A screenshot of motion alone cannot distinguish that
from a missing/foreign/duplicate host. The changed probe retains layer count,
HWND, PID, effect, both Y properties and thumbnail count at the sample. It still
requires exactly one visible layer; no gate was relaxed.

F2 invalidates transferred applied geometry (`workspace_actions.rs:424–425`).
The preserved tile throttle can suppress immediate replacement geometry
(`engine.rs:2938–2946`), which is a possible admission hazard, but its usual
16-ms duration does not by itself explain H17's 1800-ms setup wait. No speculative
product change or retry mechanism is included. The exact live root cause remains
unresolved without distinguishing the cases above.

## Verification

Pure semantic contract test:
`layout::engine::lane_f_integration_tests::h17_workspace_setup_stays_put_until_explicit_focus`.
It covers both legacy and explicit structured setup, and verifies that the view
stays on the source until explicit focus. It is not a fails-before product
regression test.

Passed with `RUSTFLAGS=-D warnings` and `-j 2`:

* `cargo build --release`
* `cargo check --release --all-targets`
* `cargo test --release`: 1481 library + 14 ctl unit tests, no failures.

The changed PowerShell file passed
`[System.Management.Automation.Language.Parser]::ParseFile`.
`cargo fmt --check` was unavailable because rustfmt is not installed.
No live-test or desktop harness was run.
