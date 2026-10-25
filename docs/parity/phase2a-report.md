# Phase 2A report: a real config parser, window rules that act, fullscreen, niri's IPC

Branch `wiri/shine-20261024`, draft PR #1.  Written 2026-10-25 after the last official cycle.  Every claim
below says whether it was **live-verified** (exercised by `scripts/live-test.ps1` on the test laptop, with real
key presses, `wiri-ctl`, window rectangles, event streams and screenshots) or **unit-tested only**.  Phase 1 is in
[phase1-report.md](phase1-report.md); the rows these items closed are in the two matrices (a row says "Phase 2A,
item N" when that item changed it).

## How it was verified

- **Test laptop (tridaris):** as in Phase 1: a spare x64 Windows 11 Home laptop (build 10.0.26200), one 1366x768
  display, Windows Terminal installed, shared with other tenants (about fifteen windows of theirs were on the desktop
  during the runs, which is why the live test uses relative checks, and why `h15_*` finds 14 to 16 windows at start).
- **The official cycle** (`schtasks /run /tn wiri-cycle`): fetch the pushed branch, `cargo build --release`, `cargo test`,
  `cargo check --all-targets` with `-D warnings`, then `scripts/live-test.ps1` in the interactive session, recorded with
  ffmpeg.  A scratch clone (`wiri-scratch`) gave faster iterations; every item went through an official cycle on its
  pushed commit.  Three scratch cycles on item 3 and two on item 4 found what is listed under "What the live gates found".
- **The live test now refuses to run twice at once** (a machine-wide named mutex in the harness; the idea and first
  version are Codex Sol's `ee41d27`).  The reason: during the first official cycle on item 3 a stale scheduled task started
  a second build and a second live run in the same clone; it overwrote `build.status` (`build_rc=101`, "access denied"
  replacing `wiri.exe` that the first run was executing) and then cleaned `C:\wiri-dev\live` before the first run's results
  were read.  The lead removed the stale tasks; item 3's official cycle was run again (below).
- **GitHub CI** (x86_64 and aarch64 `Build & test`) runs on every push.  The aarch64 binary was never run; no live run
  used the Snapdragon laptop wiri is meant for.
- **What no test here can do:** a second display.  Everything that needs one is unit-tested only (listed per item).

## At a glance

| # | Item | Commits | Live gates | Unit tests (all in `cargo test`) |
|---|---|---|---|---|
| 1 | A real KDL config parser with visible errors | `0a0bb75`, `b0e7149` | `h12_*` (27) | KDL reader, decoder, loader, `validate-config` |
| 2 | Window rules that act | `e4c5e68` | `h13_*` (33) | matching, `distribute_heights`, opacity, `rules.rs` |
| 3 | Fullscreen and maximize keep the window in the strip | `0bf3129` | `h14_*` (24) | `sizing.rs` decisions, engine states |
| 4 | IPC parity: niri's event stream and actions with niri's arguments | `b702890` | `h15_*` (30) | `ipc/state.rs`, `engine/ipcview.rs`, server, message forms |

Unit tests: 806 at item 2, 828 (+3 CLI) at item 3, 872 (+4 CLI) at item 4.  The Phase 0 and Phase 1 gates (194) stay in the same run.

## Items

### 1. A real KDL config parser with visible errors (`0a0bb75`, `b0e7149`)

The line scanner is gone.  `src/config/kdl.rs` reads KDL (nested blocks, `/-` slashdash, `/* */`, raw strings, escapes, number
forms, a BOM, UTF-16 files) with **no new dependency**; `src/config/decode.rs` maps it onto the config and reports every
problem as `file:line:column: error: message` (with a "did you mean" for keys, actions and key names).  niri's syntax loads as
written: `binds { Mod+T { spawn "wt.exe"; } }` with `repeat=false`, `cooldown-ms=`, `allow-when-locked=` and
`hotkey-overlay-title=`, `input`, `layout`, `output`, `workspace`, `window-rule`, `animations`, `spawn-at-startup`, `include`.
A bad config is never applied half: at startup wiri runs on its defaults and a balloon names the first error; on a reload the last
good config stays, the log has every error, a balloon names the file and line and `config_loaded {failed: true}` is sent; the
reload is a trailing-edge debounce (200 ms after the last change to the file or an include).  `wiri-ctl validate-config [path]`
runs the same parser.

Live gates: `h12_generated_default_config_has_no_issues`, `h12_niri_binds_block_registered`, `h12_slashdashed_bind_is_not_registered`,
`h12_niri_config_loads_without_issues`, `h12_bind_runs_its_action`, `h12_cooldown_ignores_a_press_inside_it`, `h12_cooldown_ends`,
`h12_repeat_false_fires_once`, `h12_binds_repeat_by_default`, `h12_validate_config_accepts_a_niri_file`,
`h12_validate_config_names_line_and_column`, `h12_bad_reload_gives_a_failed_config_loaded_event`, `h12_bad_reload_keeps_the_last_good_binds`,
`h12_failed_event_names_file_line_column`, `h12_bad_reload_is_logged_with_its_place`, `h12_error_balloon_could_be_shown`,
`h12_old_bind_still_fires_after_a_bad_reload`, `h12_half_written_save_is_applied_once_and_not_reported`, `h12_bind_of_the_new_config_runs`,
`h12_utf8_bom_config_loads`, `h12_utf16_config_loads`, `h12_include_binds_load`, `h12_editing_an_include_reloads`,
`h12_reload_config_command_says_why`, `h12_bad_startup_config_falls_back_to_defaults`, `h12_bad_startup_config_is_reported`,
`h12_bad_startup_balloon_was_shown`.  **Unit-tested only:** the grammar's corners (raw-string hash counts, escapes, number forms,
multi-line strings, slashdash on arguments and properties), the "did you mean" for actions and key names (one for a layout key is in
`h12_validate_config_names_line_and_column`), the include merge rules, `include optional=true`, `~` paths.
*Not done:* options niri has and wiri has no feature for are warnings, not features (the matrices list them).

### 2. Window rules that act (`e4c5e68`)

Several `match` lines are OR-ed and `exclude` lines veto; `app-id` and `title` are regular expressions (the process's file name,
with and without `.exe`, or the AppUserModelID; the title).  The `open-*` properties, parsed and never read before, now act:
`open-on-output` (the monitor's device name), `open-on-workspace` / `open-in-workspace`, `open-floating` (centred, at the size the
rule asks for), `open-fullscreen`, `open-maximized`, `open-focused false`.  `default-column-width`, `default-window-height`, the
limits `min-width`, `max-width`, `min-height`, `max-height` and `opacity` hold; opacity and the limits **follow the window**
(its title, the focus, floating, urgency, the config) with no timer.
Live gates: `h13_config_loads_without_issues`, `h13_monitor_name_is_reported`, `h13_default_column_width_fixed`,
`h13_default_column_width_proportion`, `h13_open_maximized_fills_the_view`, `h13_max_width_and_height_hold_the_window`,
`h13_limits_lift_when_the_title_stops_matching`, `h13_limits_return_when_the_title_matches_again`, `h13_app_id_regex_matches_the_process_name`,
`h13_app_id_regex_that_does_not_match_leaves_the_window_alone`, `h13_first_match_line_applies`, `h13_second_match_line_applies`,
`h13_window_matching_no_line_is_untouched`, `h13_exclude_line_vetoes`, `h13_opacity_follows_a_new_title`,
`h13_opacity_leaves_when_the_title_is_excluded`, `h13_focus_rule_dims_the_window_without_focus`, `h13_focus_rule_follows_the_focus`,
`h13_open_on_workspace_by_name`, `h13_open_on_workspace_window_is_hidden`, `h13_open_on_workspace_leaves_the_user_where_they_are`,
`h13_open_on_workspace_window_shows_when_visited`, `h13_open_in_workspace_by_position`, `h13_open_floating_floats_the_window`,
`h13_open_floating_default_size`, `h13_open_floating_is_centred`, `h13_open_fullscreen_covers_the_monitor`,
`h13_open_fullscreen_window_leaves_fullscreen`, `h13_open_focused_false_keeps_the_foreground`, `h13_open_focused_false_window_is_tiled`,
`h13_open_on_output_with_a_real_name`, `h13_open_on_output_with_an_unknown_name_is_ignored`, `h13_reload_changes_the_opacity_of_open_windows`.
**Unit-tested only:** the `is-active-in-column`, `is-floating` and `is-urgent` matchers (nothing in the live test makes a window
ask for attention), `open-on-output` with a second real monitor (one display), the height distribution corner cases.
*Not done* (section 12 of the layout matrix): `default-floating-position`, `default-column-display` (needs tabs), `scroll-factor`, `on-xdg-activate`,
the per-window `focus-ring` / `border` / `shadow` / `tab-indicator` overrides, `geometry-corner-radius`, `clip-to-geometry`, `background-effect`
(`at-startup` means "found when wiri started", not "in the first 60 seconds").

### 3. Fullscreen and maximize keep the window in the strip (`0bf3129`)

A fullscreen window used to hide every other tile of its monitor.  Now it keeps its column and its place in it, its column is made
as wide as the view meanwhile (and gets its width back), the layout pass draws it over the monitor in its slot (placed exactly,
shifted by whatever the strip is scrolled by), and nothing else is hidden or moved: the other windows stay in the taskbar and
Alt+Tab, and focusing a neighbour scrolls the strip by the least that shows it, so the fullscreen window goes away with its slot.
`ITaskbarList2::MarkFullscreenWindow` tells the shell, so the taskbar goes and comes back.  New niri action
`maximize-window-to-edges` (default bind `Mod+M`, rule `open-maximized-to-edges`): the window fills the work area with no gap and no
border and keeps its column (`maximize` is the same state now, not a native maximize the layout fought).  A window that goes
fullscreen on its own (a video player, F11) or whose maximize button is pressed is followed: the WinEvent hook reports a change of
its size class (not throttled), the engine adopts it without touching its frame, takes the native maximized state away, and tiles
the window again when the application leaves the state.
Live gates: `h14_config_loads_without_issues`, `h14_fullscreen_covers_the_monitor`, `h14_fullscreen_window_keeps_its_column`,
`h14_fullscreen_is_reported_by_ipc`, `h14_other_windows_are_not_hidden`, `h14_other_windows_stay_in_alt_tab`,
`h14_neighbours_are_out_of_the_way`, `h14_taskbar_is_hidden_during_fullscreen`, `h14_shell_was_told_about_the_fullscreen_window`,
`h14_focusing_a_neighbour_scrolls_the_fullscreen_window_with_its_slot`, `h14_fullscreen_window_comes_back_when_focused_again`,
`h14_leaving_fullscreen_gives_the_tile_back`, `h14_taskbar_is_back_after_fullscreen`, `h14_neighbours_are_back_after_fullscreen`,
`h14_maximize_to_edges_fills_the_work_area`, `h14_maximize_to_edges_keeps_the_column_and_is_reported`, `h14_maximize_to_edges_toggles_back`,
`h14_title_bar_maximize_is_followed`, `h14_title_bar_maximize_again_gives_the_tile_back`, `h14_open_maximized_to_edges_rule`,
`h14_app_fullscreen_is_followed`, `h14_app_fullscreen_hides_the_taskbar`, `h14_app_fullscreen_hides_nothing`,
`h14_app_leaving_fullscreen_is_followed` (the application case is a borderless window made as big as the monitor from its own
thread, which is what a video player does; no real player or browser was used).  **Unit-tested only:** the pure decisions
(`state_rect`, `size_reaction`), a window that opens already as big as its monitor, minimizing a fullscreen window.
*Not done:* games in exclusive fullscreen (no window geometry to see), floating windows that go fullscreen on their own (they keep what
their application does), several maximized windows in a tabbed column, a second display.

### 4. IPC parity: niri's event stream and actions with niri's arguments (`b702890`)

**The event stream is niri's: the state first, then what changes.**  Right after the ack, a subscriber gets `workspaces_changed`,
`windows_changed`, `overview_opened_or_closed` and `config_loaded` (every workspace and window with niri's fields, which window has the
focus, how the config loaded), then `window_opened_or_changed`, `window_closed`, `window_focus_changed`, `window_urgency_changed`,
`window_layouts_changed`, `workspace_activated`, `workspace_active_window_changed`, `workspace_urgency_changed`, `workspaces_changed` and
`overview_opened_or_closed` as things change, with niri's rules (a restructured workspace list is the whole story; a window sent whole is not
told again about its place or urgency).  The objects are niri's `Window` and `Workspace` (workspace ids are unique among monitors and below
2^53, window ids are window handles) plus wiri's `class_name`, `output`, `is_fullscreen`, `is_maximized`, `is_minimized`.
`workspaces_changed` and `workspace_activated`, declared and never sent before, are sent; `window_closed` is sent from the state (for
the windows wiri managed, not for every destroyed window) and has `id`; wiri's older events are untouched.

How: `TilingEngine::ipc_snapshot` describes the engine (no layout pass, no write lock), `ipc/state.rs` is the pure `diff` / `replay`, the
`IpcServer` keeps what the clients were last told.  `notify_state_changed` (two atomic operations, callable from any thread and under any
lock) wakes one task after the backend event handler, `perform_action`, an IPC request that is not a query, a config load, a mouse-wheel
workspace switch or a window drag; the task describes the engine once per burst of changes and **only while somebody is subscribed**: no
timer, no polling.  A client that falls 64 events behind is told the state again.

**Actions:** the `run_action` request (also accepted as `action`, name under `action`) takes numbers and booleans as arguments, `quoted`
flags (`focus-workspace 2` is an index, `focus-workspace "2"` a name; `wiri-ctl action` treats a word in literal double quotes so), and niri's
`--id <hwnd>` / `--id=` / `--window-id`: `close-window --id` closes that window and leaves the focus alone, any other action focuses it first
(**an approximation**: niri leaves the focus where it is) and acts on that window whatever the OS reports (`perform_action_on`); only a
window wiri manages can be named; what is wrong is said.  Schema 1.1: `docs/IPC_PROTOCOL.md` (section 6.2), `docs/IPC_SCHEMA.json`.

Live gates: `h15_config_loads_without_issues`, `h15_the_stream_starts_with_the_state`, `h15_windows_changed_describes_the_windows`,
`h15_exactly_one_window_is_focused_the_foreground_one`, `h15_workspaces_changed_describes_the_workspaces`,
`h15_a_window_names_the_workspace_it_is_on`, `h15_the_config_state_is_replayed`, `h15_an_idle_desktop_sends_no_events`,
`h15_a_new_title_is_a_changed_window`, `h15_focus_is_told_once_with_the_window_id`, `h15_floating_is_a_changed_window`,
`h15_windows_that_moved_are_told_as_layouts`, `h15_tiling_again_is_a_changed_window`, `h15_fullscreen_is_a_changed_window`,
`h15_leaving_fullscreen_is_a_changed_window`, `h15_moving_a_window_to_workspace_2_is_told_in_order`,
`h15_activating_a_workspace_is_told_once_as_the_focused_one`, `h15_activating_the_first_workspace_again_is_told`,
`h15_a_quoted_workspace_word_is_a_name_not_an_index`, `h15_a_typed_json_action_is_accepted`, `h15_an_action_can_name_its_window_with_id`,
`h15_close_window_by_id_leaves_the_focus_alone`, `h15_bad_actions_say_what_is_wrong`, `h15_the_overview_opening_is_told`,
`h15_the_overview_closing_is_told`, `h15_a_config_reload_is_told`, `h15_a_window_that_opens_is_told`,
`h15_a_window_that_closes_is_told_by_id_and_handle`, `h15_a_late_subscriber_is_told_the_state_as_it_is`,
`h15_the_filter_applies_to_the_state_and_the_changes`.
**Unit-tested only:** `window_urgency_changed` and `workspace_urgency_changed` (nothing in the live test can make a window ask for
attention), a second monitor (`workspace_activated` when the focus moves between outputs; the test laptop has one display), the resync of a
client that lags (it needs a client that stops reading for 64 events), the pure `diff` rules (19 tests in `ipc/state.rs`), the engine's
description (9 tests in `engine/ipcview.rs`), the server's subscribe, sync and `--id` paths (12 tests in `ipc/server.rs`).
*Not done:* request/reply versions of niri's `Workspaces`, `Windows`, `FocusedWindow`, `OverviewState` (the replay of the stream has the same
data), `Version`, `Outputs` details, the `output` subcommand; `window_focus_timestamp_changed`, keyboard-layout, screenshot and cast events;
tile sizes in `window_layouts_changed`; niri's actions that wiri lacks (see the action rows).

## Final gate results

Official cycle (`schtasks /run /tn wiri-cycle`) on `b702890` (the last code commit; the commits after it change documents only), round
`round-official-b702890`:

    start Sun 10/25/2026  0:56:17.20
    b702890 IPC parity: niri's event stream (the state first, then every change) and actions with niri's arguments (Phase 2A, item 4)
    build_rc=0
    test_rc=0
    check_rc=0
    done Sun 10/25/2026  0:58:12.32

- Unit tests: **872 passed, 0 failed** (library), 4 passed (CLI), the one documentation example is ignored.  `check_rc` is `cargo check --release
  --all-targets` with `RUSTFLAGS=-D warnings`.
- Live gates in the run, judged by the rules of `scripts/live-report.py`: **277 pass, 0 fail** (31 further entries are information: sizes, counts, names);
  the 30 new `h15_*` gates, the 24 `h14_*`, the 33 `h13_*`, the 27 `h12_*`, and every gate of Phase 0 and Phase 1.  One of them,
  `hidden_json_present_after_restart`, is **good when False** (the crash record is deleted once the windows are back).
- Steps with a non-zero return code: none.  GitHub CI (x86_64 and aarch64 `Build & test`): success on `b702890`.

The official cycles of the other items, each on its pushed commit: `b0e7149` (item 1: 176 boolean gates True, the one expected False),
`e4c5e68` (item 2: 209 True), `0bf3129` (item 3: 233 True, 247 pass and 0 fail by the `live-report.py` rules; its first official cycle was spoiled by
the stale task described above and was run again: `start 0:14:32`, `build_rc=0`, `test_rc=0`, `check_rc=0`).  All four had `build_rc=test_rc=check_rc=0`.

## What the tests found along the way

- **The IPC `focus-window` never scrolled the strip to its window** (`h14_focusing_a_neighbour_scrolls_the_fullscreen_window_with_its_slot`,
  first scratch run: 23 of 24 gates): the request set the engine's focus and activated the window, and the OS focus report that followed found
  the engine's focus already there and scrolled nothing, so a window off screen stayed off screen.  `focus_window_by_hwnd` reveals the column now
  (`reveal_focused`, in `0bf3129`).  The older gate `h7_two_50_percent_columns_fill_the_view` had relied on that bug (its scenario moved the focus
  to a 700 px column through IPC, then shrank it, and counted on the strip not having scrolled to show it); with niri's behaviour (a column that
  shrinks keeps its left edge) its scenario makes the column a half first.  The gate asserts what it did.
- The first H15 run had every state-dependent gate False: a **harness** bug (`@($list)` over a `List[object]` of parsed events throws "Argument
  types do not match" in Windows PowerShell 5.1), not a product defect: the recorded stream already had the right events in the right order.
  The harness result was only trusted after the second run, with the parsing fixed and the recorded stream read by the same function offline.
- Unit-test expectations that were wrong, found by the first run of the new tests, and corrected to what niri does: a new window on the active
  workspace tells the workspace's active window first (niri's order), and a restructured workspace list is sent alone (niri clears the flag events).
- Two builds and two live runs in the same clone during one official cycle (see "How it was verified"): the live test now holds a lock, and a
  second run refuses to start instead of spoiling the first.

## Dependency changes

None.  No crate was added or upgraded (`Cargo.lock` is unchanged since Phase 1).  The `windows` crate gained the feature
`Win32_UI_Shell_PropertiesSystem` (item 2: the AppUserModelID of a window for `app-id`); `ITaskbarList2` (item 3) is in `Win32_UI_Shell`,
already enabled.  Phase 4 can drop that feature together with the AppUserModelID lookup if `app-id` stays process-name only.

## Timers and threads Phase 4 should remove (noticed; none added in Phase 2A)

Nothing in items 1 to 4 adds a timer, a polling loop or a thread that wakes without an event.  What the work leans on, and what is still
there from before (file and line as of `b702890`):

| Where | What | Wakeups |
|---|---|---|
| `main.rs` main loop (`sleep(8 ms)`, line ~837) | polls the reload channel, tray actions, `LAST_WS` (workspace switch for the overlay and the older `workspace_switched` event) | ~125 per second, forever |
| `main.rs` event drain (`sleep(50 ms)`, line ~559) | `try_recv` on the backend event channel instead of `recv().await`; also **adds up to 50 ms to every event the IPC stream is told about**, because the stream is computed after the handler ran | 20 per second |
| `main.rs` reconciler (`interval(750 ms)`, line ~570) | `EnumWindows` + `WindowInfo::new` for every visible window and an `apply_all` | ~1.3 per second |
| `main.rs` mouse tracker (`sleep(500 ms)`, line ~636) | takes the tracker write lock and reads the cursor even when `focus-follows-mouse` is off | 2 per second |
| `main.rs` status bar updater (`sleep(1 s)`) and `overlay/status_bar.rs` `SetTimer(1000)` | only with `status-bar true` | 1 per second each |
| `overlay/workspace_indicator.rs` `SetTimer(16 ms)` | fade timer | ~60 per second while the indicator is up |
| `backend/hooks.rs` `detect_size_class_change` (item 3) | for every location change of a tracked window that wiri did not cause: `IsZoomed`, `DwmGetWindowAttribute(EXTENDED_FRAME_BOUNDS)` and a monitor lookup, before the 200 ms throttle (a window being dragged or animating itself produces a stream of them) | per external `LOCATIONCHANGE` |

Event-driven and cheap by construction (kept that way on purpose): the config watcher and its debounce (`recv` until a file changes), the
window rules (re-evaluated on title, focus, float, urgency and config events only, gated by `RuleFlags`), the IPC state task (`Notify`, sleeps
until `notify_state_changed`, does nothing without subscribers: two atomic operations per event or action are the whole cost), the size-class
reaction.  Per-event work worth a look: the IPC description is O(windows + workspaces) with string copies (only while subscribed, once per
burst); `window_title_changed` takes the engine write lock and clones the title for every title event of a tracked window; fullscreen and
maximized windows are placed on every layout pass that finds their rectangle changed (the applied-state cache holds them since item 3).

## Left to do, in priority order

1. **A second display.**  Monitor unplug (Phase 1 item 10), `open-on-output` with a real second monitor, `workspace_activated` when the focus
   crosses monitors and `monitor_changed` have only unit tests.
2. **The rest of Phase 2 (niri feel):** the low-level keyboard bind engine (Win-key chords, key repeat), mouse and wheel binds, one overlay
   layer for focus ring, border, shadow, tab indicator and insert hint, the spring animation pipeline, tabs, the overview and the recent-windows
   switcher, the hotkey overlay, PNG and clipboard screenshots.
3. **IPC rows still open:** the request/reply forms in niri's shapes (`Workspaces`, `Windows`, `FocusedWindow`, `OverviewState`), `Version`,
   `Outputs` details and the `output` subcommand, `window_focus_timestamp_changed`, keyboard-layout and screenshot events, tile sizes in
   `window_layouts_changed`, newline framing with several requests per connection, niri's `--id` without moving the focus.
4. **niri actions wiri lacks** (the `move-window-to-workspace N or name` and `move-column-to-workspace N or name` family, the focus flags, and
   the others the layout matrix lists as missing).
5. **Phase 4:** remove the polling above, create the overlays on first use, count `SetWindowPos` calls per action as a gate.
6. **Phase 3:** elevated windows, per-user pipe names, the quoted autostart path, ARM64 release builds and a live run on the Snapdragon laptop.
7. **Real-application checks** for item 3's "an application goes fullscreen on its own": a video player and a browser's F11, not only a borderless
   test window.
