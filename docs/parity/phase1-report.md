# Phase 1 report: safe to daily-drive

Branch `wiri/shine-20261024`, draft PR #1.  Written 2026-10-24 after the last official cycle.  Every
claim below says whether it was **live-verified** (exercised by `scripts/live-test.ps1` on the test
laptop, with real key presses, `wiri-ctl`, window rectangles and screenshots) or **unit-tested only**.

## How it was verified

- **Test laptop (tridaris):** a spare x64 Windows 11 Home laptop (build 10.0.26200), one 1366x768
  display, Windows Terminal installed, rustc 1.95.  It is shared: a Neva job, a Meylin runner, another
  session's builds and about thirty windows of other tenants live on the same desktop, so the live test
  uses relative checks (columns, order, overlap) instead of absolute strip positions, and presses the
  shipped default chords (`Ctrl+Alt+F8` / `F11` are taken by another tenant).
- **The official cycle** (`schtasks /run /tn wiri-cycle`): `git pull`, `cargo build --release`, `cargo test`,
  `cargo check --all-targets` with `-D warnings`, then `scripts/live-test.ps1` in the interactive session,
  recorded with ffmpeg (`scripts/record-live.ps1`, 6 fps, 960 px, hidden).  The recording did not change any gate.
  A scratch clone and `scratch-cycle.cmd` gave faster iterations; every item went through an official cycle
  before the next one, and the last commit went through one as well (below).
- **GitHub CI** (x86_64 and aarch64 `Build & test`) runs on every push.  The aarch64 build was never run; the
  x86_64 binary was the one exercised live.
- **What no test here can do:** a second display.  Monitor unplug (item 10) is covered by engine unit tests
  only.  The aarch64 laptop that wiri is meant for (Snapdragon X) was not used for any live run.

## At a glance

| # | Item | Commit | Live gates | Unit tests |
|---|---|---|---|---|
| 1 | Unknown modifiers are rejected, niri `Mod` | `02e8aab` | `h1_*` (5) | input, config parse, hotkey rows |
| 2 | `spawn` / `spawn-at-startup` keep every argument and search PATH | `b1af9bf` | `h2_*` (7) | command-line rules, spawner |
| 3 | Actions act on the real foreground window, floats included | `b62a0c8` | `h3_*` (22) | target resolution, float geometry |
| 4 | A minimized window leaves no gap and returns to its place | `1ae1698` | `h4_*` (14) | slots, every restore order |
| 5 | A floating window belongs to its workspace and stays on top | `b64be61`, `7055463` | `h5_*` (14) | float-only workspaces, one tile per window |
| 6 | niri's column keymap | `91ba170` | `h6_*` (25) | list operations, engine operations, names |
| 7 | niri's action names and argument grammar | `e1ca86e` | `h7_*` (24) | grammar, resolve, aliases, size actions |
| 8 | The workspace model | `ed3ce93` | `h8_*` (12) | order API, previous, auto back-and-forth, named workspaces |
| 9 | Phase 0 leftovers | `0febee8` | `h9_*` (15) | width resolution, hidden.json originals, default chords |
| 10 | Monitor unplug keeps the windows | `de78b63` | none: **unit-tested only** (one display) | seven engine tests |
| 11 | `hide-method "cloak"` (optional) | `c51ced6`, `6ea45e4` | `h11_*` (13) | config, flags, foreground rule, vtable layout |

The phase-0 and earlier gates (43: loop, IPC latency, crash safety, key presses, console) stay in the same run.

## Items

### 1. Unknown modifiers are rejected (`02e8aab`)

`Hyper+T`, `Mod5+T` or a typo such as `Crtl+T` used to be dropped silently, which registered a bare global
hotkey and took that key from every application.  Such a bind is ignored at parse time with a warning, and
`wiri-ctl validate-config` reports it.  niri's `Mod` resolves from `input { mod-key }` (Ctrl+Alt unless set).
Live gates: `h1_unknown_modifier_warnings`, `h1_unsupported_modifier_warnings`, `h1_rejected_binds_registered`, `h1_mod_bind_is_ctrl_alt`, `h1_f9_f8_reach_application`.

### 2. `spawn` and `spawn-at-startup` (`b1af9bf`)

One spawner for binds, the startup list and the IPC request: every argument kept, Windows command-line quoting,
programs found on PATH (so the default config's `wt.exe` works), `.cmd` / `.bat` through PATHEXT.
Live gates: `h2_default_terminal_opens`, `h2_startup_args_ok`, `h2_bind_args_ok`, `h2_shell_ok`, `h2_ipc_args_ok`, `h2_ipc_command_line_with_spaced_program_ok`, `h2_wt_exe_found_on_path`.

### 3. Actions act on the real foreground window (`b62a0c8`)

The target comes from `GetForegroundWindow()` (owner chain for dialogs), the engine focus second.  A floating
window is a real target; `close-window` refuses to guess when an unmanaged window has the focus; new IPC
`run_action` / `wiri-ctl action`.  Live gates: `h3_float_keeps_foreground`, `h3_float_reported_floating`, `h3_second_press_tiles_the_same_window`, `h3_second_press_floats_nothing_else`, `h3_tiled_again_has_column_height`, `h3_float_geometry_remembered`, `h3_float_keeps_foreground_after_retoggle`, `h3_float_keeps_own_size`, `h3_float_is_centered`, `h3_move_nudges_float_50px`, `h3_fullscreen_covers_monitor`, `h3_fullscreen_only_the_float`, `h3_fullscreen_exit_restores_float`, `h3_ipc_focus_window_tile`, `h3_ipc_focus_window_float`, `h3_ipc_action_centers_float`, `h3_close_closes_the_float`, `h3_tiles_survive_float_close`, `h3_dialog_was_foreground`, `h3_close_hits_dialog_not_owner`, `h3_tool_window_was_foreground`, `h3_close_refuses_unmanaged_foreground`.

### 4. Minimized windows (`1ae1698`)

`EVENT_SYSTEM_MINIMIZESTART` / `END`: the window leaves its column, the others close up, and it returns to its
old place (found again by neighbour, not index; a minimized desktop comes back in its order).
Live gates: `h4_three_windows_in_order`, `h4_minimized_window_is_iconic`, `h4_neighbours_close_the_gap`, `h4_ipc_reports_minimized`, `h4_restored_between_its_neighbours`, `h4_restored_keeps_its_size`, `h4_ipc_reports_restored`, `h4_all_minimized`, `h4_all_restored`, `h4_old_order_kept_after_restoring_in_another_order`, `h4_focus_window_restores_a_minimized_window`, `h4_closed_while_minimized_is_gone`, `h4_closed_while_minimized_is_forgotten`, `h4_neighbours_stay_after_closing_a_minimized_window`.

### 5. Floating windows (`b64be61`, `7055463`)

A float is hidden and shown with its workspace, stays above the tiles, and keeps a workspace alive that holds
only floats.  The live gates found a real race: two events could add one new window twice (two tiles, a ghost
column); the check is now repeated under the write lock.  Live gates: `h5_float_on_top_when_it_floats`, `h5_ipc_reports_float_on_workspace_1`, `h5_float_hidden_on_another_workspace`, `h5_float_shown_again_at_its_place`, `h5_float_on_top_after_returning`, `h5_overlapping_tile_got_the_focus`, `h5_float_stays_above_the_focused_tile`, `h5_float_stays_above_after_workspace_round_trip`, `h5_move_to_workspace_hides_the_float_here`, `h5_ipc_reports_float_on_workspace_2`, `h5_float_shown_on_its_new_workspace_at_the_same_place`, `h5_float_hidden_again_on_workspace_1`, `h5_focus_window_switches_to_the_float_workspace`, `h5_float_closed`.

### 6. niri's column keymap (`91ba170`)

`consume-or-expel-window-left/right`, the corrected `consume-window-into-column` / `expel-window-from-column`,
`move-window-up/down`, `focus-column-first/last`, `move-column-to-first/last`, and a `maximize-column` that
really makes the column as wide as the view, also for one window.  Live gates: `h6_niri_keymap_binds_registered`, `h6_three_single_window_columns`, `h6_consume_or_expel_left_joins_the_left_column`, `h6_emptied_column_vanishes`, `h6_focus_stays_on_the_moved_window`, `h6_consume_or_expel_left_expels_to_the_left`, `h6_consume_or_expel_right_joins_the_right_column`, `h6_consume_or_expel_right_expels_to_the_right`, `h6_consume_window_into_column_takes_the_right_neighbour`, `h6_consume_keeps_the_focus`, `h6_expel_takes_the_bottom_window`, `h6_expel_keeps_the_focus`, `h6_move_window_up`, `h6_move_window_keeps_the_focus`, `h6_move_window_down`, `h6_focus_window_up_name`, `h6_focus_window_down_name`, `h6_focus_column_first`, `h6_focus_column_last`, `h6_move_column_right_swaps_the_columns`, `h6_move_column_left_swaps_them_back`, `h6_move_column_to_last`, `h6_move_column_to_first`, `h6_maximize_column_full_width_for_a_one_window_column`, `h6_maximize_column_restores_the_width`.

### 7. niri's action names and argument grammar (`e1ca86e`)

`set-column-width` / `set-window-height` take niri's size argument (`"50%"`, `"+10%"`, `"1200"`, `"+100"`),
`focus-workspace 2` is an index and `"2"` a name, and the niri action names are accepted for what wiri already
does.  Proportions are niri's (two `"50%"` columns and the gap fill the view).  Live gates: `h7_config_bind_warnings`, `h7_niri_binds_registered`, `h7_set_column_width_percent`, `h7_set_column_width_plus_percent`, `h7_set_column_width_minus_percent`, `h7_set_column_width_pixels`, `h7_set_column_width_plus_pixels`, `h7_two_50_percent_columns_fill_the_view`, `h7_switch_preset_column_width_cycles_and_goes_back`, `h7_focus_workspace_number_is_an_index`, `h7_focus_workspace_index_back`, `h7_focus_workspace_quoted_number_is_a_name`, `h7_fullscreen_window`, `h7_fullscreen_window_off`, `h7_toggle_window_floating`, `h7_move_window_to_floating_is_no_toggle`, `h7_move_window_to_tiling`, `h7_move_window_to_tiling_is_no_toggle`, `h7_focus_monitor_left_at_the_edge_keeps_the_focus`, `h7_set_window_height_percent`, `h7_set_window_height_minus_percent`, `h7_reset_window_height`, `h7_wiri_ctl_set_column_width`, `h7_wiri_ctl_set_column_width_minus_percent`.

### 8. The workspace model (`ed3ce93`)

Workspaces are addressed by position; `focus-workspace-previous`; `workspace-auto-back-and-forth`; up and down do
not wrap; named workspaces from the config always exist; an empty middle workspace vanishes when left.
Live gates: `h8_config_bind_warnings`, `h8_named_workspaces_exist_from_the_start`, `h8_focus_workspace_name_of_an_empty_workspace`, `h8_focus_workspace_previous_goes_back`, `h8_focus_workspace_3_is_the_empty_one_below`, `h8_auto_back_and_forth`, `h8_a_number_past_the_last_is_the_last`, `h8_focus_workspace_down_does_not_wrap`, `h8_back_on_the_first_workspace`, `h8_focus_workspace_up_does_not_wrap`, `h8_focus_window_switches_to_its_workspace`, `h8_number_counts_positions_after_a_workspace_went`.

### 9. Phase 0 leftovers (`0febee8`)

- **Minimum width.**  Every window is asked `WM_GETMINMAXINFO` (through `SendMessageTimeoutW`, 120 ms,
  `SMTO_ABORTIFHUNG`, outside the engine lock, once when added and twice more at most when it took the focus without
  having answered); a column is never narrower than its windows.  Three Windows Terminal windows on 1366 px used
  to overlap.  *Not done:* maximum sizes, minimum heights, a window that changes its minimum later.
- **Fullscreen** is exactly the monitor (it was 7 px larger: the invisible border measured while it had a frame was added
  to a frameless window).
- **`Ctrl+Alt+Tab`** (Windows' task switcher) is no longer a default; `focus-previous` is ``Ctrl+Alt+` ``.
- **`hidden.json`** also keeps every window's original rectangle, so `quit` after a kill puts windows back where they
  were (windows with an unknown place are cascaded, no longer stacked).

Live gates: `h9_failed_default_hotkeys`, `h9_default_hotkeys_registered`, `h9_focus_window_focused_the_first_form`, `h9_ctrl_alt_grave_goes_back_to_the_previous_window`, `h9_fullscreen_covers_exactly_the_monitor`, `h9_fullscreen_off_gives_the_tile_back`, `h9_tile_after_fullscreen_is_aligned_with_its_neighbour`, `h9_three_terminals_opened`, `h9_overlapping_terminal_pairs`, `h9_a_window_was_parked_when_wiri_died`, `h9_originals_recorded_before_the_crash`, `h9_wiri_had_moved_the_windows_before_the_crash`, `h9_originals_survive_the_restart`, `h9_every_window_back_where_it_was_after_crash_and_quit`, `h9_record_gone_after_a_graceful_quit`.

### 10. Monitor unplug keeps the windows (`de78b63`)

**Unit-tested only, said plainly: the test laptop has one display and no monitor has been unplugged on a real
desktop.**  `unregister_monitor` moves the workspaces that hold something to the monitor that is left (named ones join
the workspace of the same name, floats and minimized windows with them) and `register_monitor` gives the windows back
that are still where they were put; a monitor that goes when none is left waits for the next one.  The
`WM_DISPLAYCHANGE` handler (`sync_monitors_from_backend`) is exactly the part only a second display can exercise.

### 11. `hide-method "cloak"` (`c51ced6`, `6ea45e4`; optional, off by default)

`layout { hide-method "cloak" }` hides the windows of other workspaces with the shell's cloak (komorebi's
`IApplicationView::SetCloak`) instead of `SW_HIDE`, so they stay in the taskbar and Alt+Tab.  The shell interfaces are
private: the view is asked for by its IID before its vtable is used, and a refusal makes wiri hide the plain way.
`hidden.json` marks cloaked windows, so a restart, a quit and a panic uncloak them; Alt+Tab to a cloaked window
brings its workspace along.

Verified live on tridaris (Windows 11 build 26200): the windows of the other workspace are cloaked by the shell
(`DWMWA_CLOAKED` = 2) yet `IsWindowVisible`, not one pixel of their colour is drawn, the shell itself reports them in
Alt+Tab and on the taskbar (`IApplicationView::GetShowInSwitchers`), the real `Alt+Tab` keys bring the workspace back,
a killed wiri leaves them cloaked and the restarted one uncloaks them, and a quit leaves none cloaked.  The phase found
two real defects (a layout pass after the restore hid the windows again, in the default method too; a window hidden by
leaving its workspace was not time-stamped), fixed in `6ea45e4`.  *Not verified:* other Windows builds, the ARM64 laptop,
a hung shell (the COM calls have no timeout; `hide` stays the default for that reason).

Live gates: `h11_config_accepted_without_warnings`, `h11_forms_drawn_before_hiding`, `h11_shell_lists_the_forms_in_alt_tab_before`, `h11_other_workspace_windows_are_cloaked_not_hidden`, `h11_cloaked_windows_are_not_drawn`, `h11_cloaked_windows_stay_in_alt_tab_and_the_taskbar`, `h11_hidden_json_marks_them_cloaked`, `h11_alt_tab_changed_the_foreground_window`, `h11_alt_tab_to_a_cloaked_window_brings_its_workspace`, `h11_windows_stay_cloaked_when_wiri_dies`, `h11_restart_uncloaks_what_the_dead_wiri_hid`, `h11_windows_are_cloaked_again_before_the_quit`, `h11_quit_uncloaks_everything`.

## Final gate results

Official cycle (`schtasks /run /tn wiri-cycle`) on `6ea45e4`, round `round-official-6ea45e4`:

    start Sat 10/24/2026 20:33:42.33
    6ea45e4 cloak: a quit leaves nothing hidden, Alt+Tab to a cloaked window brings its workspace; live phase H11 completed
    build_rc=0
    test_rc=0
    check_rc=0
    done Sat 10/24/2026 20:35:14.57

- Gates in the run: **194**; not as expected: **0**.
- Steps with a non-zero return code: none.

## What the live gates found along the way

- A window found by two events at once was added twice (two tiles, a ghost column): `h5_ipc_reports_float_on_workspace_2` and
  `h5_focus_window_switches_to_the_float_workspace` failed in one run of two for identical code (`7055463`).
- Two default-keymap chords (`Ctrl+Alt+F8`, `Ctrl+Alt+F11`) are taken by other programs on the shared laptop, so a test
  that bound them saw no key arrive (`os error 1409` in the log); the H6 gates press the shipped default chords.
- The Phase C gate `hidden_json_present_after_restart` now means "the file still lists hidden windows": `hidden.json`
  stays after a restart because it also holds the original rectangles (`0febee8`).
- The cloak phase found three product defects the unit tests could not: a layout pass between `restore_all` and the end of the
  process hid the windows of the other workspaces again, so a quit could leave windows hidden (cloaked, with the default method
  it is the same race; `h11_quit_uncloaks_everything`, fixed by `shut_down` in the engine); a window hidden by leaving its
  workspace was not stamped with the time, so a later foreground report for it was ignored (`h11_alt_tab_to_a_cloaked_window_brings_its_workspace`,
  fixed by one `note_hidden` for every place that hides); and a restart enumerated the desktop before DWM had dropped the cloak
  of the windows it had just uncloaked, and an enumeration skips a cloaked window (it now waits).
- Test-side findings for the same phase: `SwitchToThisWindow` and the shell's own `IApplicationView::SwitchTo` do not activate a
  cloaked window (the real `Alt+Tab` keys do); UI Automation sees only the taskbar's groups on Windows 11, not its buttons, so
  "stays in Alt+Tab and on the taskbar" is read from the shell itself (`IApplicationView::GetShowInSwitchers`, gate
  `h11_cloaked_windows_stay_in_alt_tab_and_the_taskbar`) instead.
- `hooks::spawner::tests::test_capture_screenshot_to_file_bmp_signature` needs the interactive desktop (`BitBlt`): it
  passes in the official cycle and fails in a plain ssh session.  Items 10 and 11 were first built and tested over ssh while
  tridaris was on battery power (its scheduled tasks did not start on batteries until the lead changed their power
  settings); every other test passed there.

## Timers and threads Phase 4 should remove (noticed, none added)

Nothing in items 6 to 11 adds a timer, a thread or a per-event full layout pass.  What exists (file and line as of this commit):

| Where | What | Wakeups |
|---|---|---|
| `main.rs` main loop (`sleep(8 ms)`, ~line 789) | polls the config reload channel, tray actions and animation state | ~125 per second, forever |
| `main.rs` event drain (`sleep(50 ms)`, ~line 520) | `try_recv` on the backend event channel instead of `recv().await` | 20 per second |
| `main.rs` reconciler (`interval(750 ms)`, ~line 531) | `EnumWindows` + `WindowInfo::new` for every visible window and an `apply_all` | ~1.3 per second |
| `main.rs` mouse tracker (`sleep(500 ms)`, ~line 596) | takes the tracker write lock and reads the cursor even when `focus-follows-mouse` is off | 2 per second |
| `main.rs` status bar updater (`sleep(1 s)`) and `overlay/status_bar.rs` `SetTimer(1000)` | only with `status-bar true` | 1 per second each |
| `overlay/workspace_indicator.rs` `SetTimer(16 ms)` | fade timer | ~60 per second while the indicator is up |
| `main.rs` animation tick (`ANIMATION_TICK_MS = 33`) | already skips the engine lock when nothing is animating | - |

Threads started at launch whether or not the feature is used: the WinEvent hook thread (`backend/hooks.rs`), two hotkey / keyboard-action
threads (`backend/message_loop.rs`), the config watcher (`config/loader.rs`), the tray (`hooks/tray.rs`), the low-level keyboard and mouse
hook threads and two workers (`input/low_level_hook.rs`), and one thread each for the snap guide, the cheatsheet, the overview banner, the
workspace indicator and the status bar overlays; the overview host window gets its own thread on first use.  `tokio` is built with `full` on
a multi-thread runtime.  The overlays can be created on first use.

Per-pass work worth a look: `hide_inactive_workspace_windows_for_monitor` posts `SW_HIDE` for every window of every inactive workspace on
every layout pass (the 750 ms reconciler makes that a steady stream); a fullscreen window is repositioned on every pass (no cache entry);
`hidden::note_hidden` scans the hidden list for each of those hides; in cloak mode every inactive window costs a `DwmGetWindowAttribute`
per pass; `sync_min_widths` walks every tile when a window is added or moved (cheap, but it is O(tiles) per event); learning a
window's minimum width runs one layout pass for its monitor (at most three times per window, and only when the minimum changes a column).

## Left to do, in priority order

1. **A second display.**  Item 10 has never run on a real unplug: one `wiri-ctl monitors` / unplug / replug session on a laptop with an
   external monitor, watching `sync_monitors_from_backend`, would settle it.
2. **A real KDL parser with visible errors** (Phase 2): the config reader is line-oriented and ignores what it does not know, so a niri
   `binds { Mod+T { spawn "x"; } }` block with nested nodes still does not load as written.
3. **Window rules that act**: `open-on-output`, `open-on-workspace`, `open-fullscreen`, `open-floating` are parsed and never read; maximum and
   minimum heights and sizes (item 9 did the minimum width).
4. **The fullscreen / maximize model** (a fullscreen window still hides the other tiles of its workspace instead of staying in the strip),
   app-initiated fullscreen and maximize.
5. **Phase 4**: remove the polling above, create the overlays on first use, count `SetWindowPos` calls per action as a gate.
6. **Cloak on more machines, and its failure mode**: item 11 was written against komorebi's interface definitions and checked on one
   Windows build; the first refusal falls back to `SW_HIDE`, but a build that answers differently would be new information, and the
   owner's ARM64 laptop has not run it.  The COM calls into the shell have no timeout and run under the engine lock, so a hung
   explorer would stall wiri (one reason `hide` stays the default): they belong on a worker with a deadline before cloak is made the default.
7. **Phase 3**: elevated windows, per-user pipe names, the quoted autostart path, ARM64 release builds.

