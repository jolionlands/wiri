# Partial parity rows, classified (2026-10-30)

This file replaces nothing: it is the working record behind the status cells of
[niri-gaps-layout-behaviour.md](niri-gaps-layout-behaviour.md) and
[niri-gaps-config-input-ipc.md](niri-gaps-config-input-ipc.md) at the `0c2581d` base. It exists
because the two matrices carried 92 `partial` rows at that base and a status cell cannot say
*which* kind of gap each one is, so the same sentence was being re-derived on every lane. The
classification and the row table below are that 2026-10-30 snapshot; the counts and the
promotion list that follow them are current to the 2026-10-31 official `dab9a88` cycle.

**Method.** Each row was read in its own matrix (status, evidence and Windows-approach cells),
then checked against `src/` and `scripts/` for the implementation it claims, and against
`scripts/live-test.ps1` / `scripts/live-phases/*.ps1` for the gate it names. A row is placed in
exactly one primary category; several rows have secondary limits too, which are kept in the
matrix text, not dropped.

**What this pass did not do.** It did not promote a single row. Promotion needs a live gate that
checks exactly the row's niri behaviour to have PASSED in a complete official run, with the run
directory and the test names cited. The official runs live under `C:\wiri-dev\evidence\<run>\`,
and this worktree could not read that directory (the runner refused the path, and the access
request was still pending when the pass ended); the matrices' own "live pending" sentences are
not evidence of a pass. So every `implemented, evidence not reconciled` row below still carries
either a named `h95_*` gate that has been written and parse-checked but not yet run, a gate that
already exists in another lane and only needs a complete cycle (h81, h85, h89, h90, h91), or the
one exact check that would settle it. The 2026-10-31 `dab9a88` cycle has since run the `h89`,
`h90`, `h91` and `h95` gates, which is why six rows below are marked promoted and fourteen more
now cite a passed gate next to the condition that still keeps them `partial`.

**Cross-checks against the previous survey.** [next-lanes.md](next-lanes.md) recorded 32 / 13 / 9 /
39 at 93 rows on 2026-10-29. The movement since then is the four rows that changed on
2026-10-30 (the focus core, the floating layer under a fullscreen tile, the DPI decoration
projection and the overview/Pango/CFG gates), plus the recount below; the categories are not
directly comparable because this pass assigns one primary category per row against the current
text rather than the older survey's per-row prose.

## Summary

| Primary condition | Layout | Config/input/IPC | Total |
| --- | ---: | ---: | ---: |
| Missing or incorrect software | 2 | 3 | 5 |
| Implemented; evidence not reconciled | 5 | 5 | 10 |
| Hardware evidence unavailable | 20 | 15 | 35 |
| Windows backend difference | 11 | 21 | 32 |
| **Total partial rows** | **38** | **44** | **82** |

The 2026-10-30 counts in this table were 2/8/21/14 and 3/7/16/21, **45** and **47**, **92** in
total. The movement is the five pointer rows promoted by the official `dba4606` cycle (one
tablet-pen, three drag-and-drop and one tablet-stylus row) plus the "Live zoomed-out view"
overview row that this snapshot did not list, and the six rows promoted by the official
`dab9a88` cycle described below.

Matrix totals after the 2026-10-31 `dab9a88` cycle: **171 done / 38 partial / 17 N/A** (layout,
226 rows) and **144 done / 44 partial / 71 N/A** (config/input/IPC, 259 rows), i.e. **315 done /
82 partial / 88 N/A** across 485 rows, **315 of 397 applicable rows (79.3 %)**. The 2026-10-30
pass itself promoted nothing.

## Rows promoted since this snapshot

The official `dab9a88` cycle of 2026-10-31 (run directory
`C:\wiri-dev\evidence\dab9a88-live`, 911/915 boolean gates) passed the gates these rows were
waiting for. Six rows left `partial` because the gate was their only remaining condition:

| Row | Matrix | Gate(s) that passed |
| --- | --- | --- |
| Mouse move/resize of floats via the Mod key (L11) | layout | `h95_mod_drag_moves_a_floating_window`, `h95_mod_drag_resizes_a_floating_window`, `h95_mod_drag_escape_cancels_to_the_pre_drag_rect` |
| Tab indicator (L14) | layout | `h95_tab_indicator_urgent_colour_overrides_the_selection` |
| Live zoomed-out view of every workspace (not listed below) | layout | all eight `h97_*` gates |
| Gesture kinetics: spring decay, rubber-band (L45) | layout | `h90_no_animation_wakeups_at_rest` |
| What a reload re-applies (C2) | config/input/IPC | all four `h91_*` gates |
| `hotkey-overlay-title` Pango markup (C23) | config/input/IPC | all six `h89_*` gates |

Fourteen rows had their named gates pass in the same cycle but keep `partial`, because their
text also records a condition a gate cannot clear: L8 (`center-column`: differing physical
outputs), L9/row 131 (`FloatingActive::NoButRaised`), L16 (multi-display overview keyboard), L20
and L29 (opaque-host alpha over the backdrop and the workspace shadow), L23 (the xdg-activation
token path and the strict cross-workspace detail), L38 (other refresh rates, multi-output), L39
(a real Precision Touchpad), C1 (non-animation config bounds), C29 (the census's six
executor-evidence partials), C38 (`background-color`'s per-output backdrop) and C43 (no
per-client inhibition protocol on Windows); L5/row 61 (self-resize) and C47 keep their recorded
missing maximum-size/minimum-height consumers and focus-policy gaps.

## Every partial row

`L` is [niri-gaps-layout-behaviour.md](niri-gaps-layout-behaviour.md), `C` is
[niri-gaps-config-input-ipc.md](niri-gaps-config-input-ipc.md); the ordinal is the row's position
among the partial rows of that matrix in file order, because the matrices have no stable ids.

| # | Matrix / section | Row | Primary condition | Exact remaining condition |
| --- | --- | --- | --- | --- |
| L1 | Layout model and `layout {}` | Scrolling strip: workspace = horizontal strip of columns, column = vertical stack of tiles | hardware evidence unavailable | Cross-output clipping and partly visible windows need a second physical output; H40-H44 pin the single-output strip and the multi-display geometry is unit-tested only. |
| L2 | Layout model and `layout {}` | `preset-window-heights` | hardware evidence unavailable | Physical mixed-DPI outputs and the remaining tiled-height lifecycle routes are unmeasured; the 2,104-test source qualification and the `381ce11` native run stop at 96 DPI. |
| L3 | Layout model and `layout {}` | Per-output `layout {}` overrides | hardware evidence unavailable | Distinct physical outputs and scaled metrics still need qualification; H70 covers global/output/workspace precedence and reload on one 96-DPI output. |
| L4 | Layout model and `layout {}` | New window opens as a column right of the active one, becomes active, view animates to it | implemented, evidence not reconciled | The insert/focus half is live (`h41_inserts_right_of_active_middle_end_empty`); the reveal animation for an admitted window is not observed live, because H40-H44 run with animations off and the H17 camera samples cover explicit column moves, resize and workspace switches. |
| L5 | Layout model and `layout {}` | Windows keep the final say over their size (min/max, self-resize); layout adapts | missing/incorrect software | Only the minimum width is read (WM_GETMINMAXINFO at admission and focus); maximum sizes, minimum heights, a window that changes its minimum later and a window that resizes itself are not consumed. |
| L6 | Layout model and `layout {}` | Resize transactions: windows in a column resize and present together | Windows backend difference | Windows cannot gate another application's repaint: a column moves in one `DeferWindowPos` batch, but the applications' content is not presented together. |
| L7 | Column and tile sizing | `switch-preset-window-height` / `-back` | hardware evidence unavailable | As L2: physical mixed-DPI and the remaining lifecycle routes; the preset ladder itself passed 21 native checks at 96 DPI. |
| L8 | Column and tile sizing | `center-column` | implemented, evidence not reconciled | `h95_center_column_centres_variable_width_edge_columns` passed in the official dab9a88 cycle; the differing-physical-outputs half still needs hardware. |
| L9 | Floating windows | Floating layer: per workspace, always above tiles, does not scroll, has its own focus | Windows backend difference | niri's `FloatingActive::NoButRaised` is not modelled because wiri's hover focus is a native activation; the six `h96_*` gates that cover the rest passed in the official dab9a88 cycle. |
| L10 | Floating windows | `move-floating-window` (x/y: set, adjust, proportion) | hardware evidence unavailable | Typed positioning and persistence are qualified at 96 DPI only; distinct physical output scales are unmeasured. |
| L11 | Floating windows | Mouse move/resize of floats via the Mod key | promoted to done (official dab9a88 cycle) | Settled: `h95_mod_drag_moves_a_floating_window`, `h95_mod_drag_resizes_a_floating_window` and `h95_mod_drag_escape_cancels_to_the_pre_drag_rect` all passed in the official 2026-10-31 cycle. |
| L12 | Workspaces | Workspaces survive monitor unplug and return when it reconnects | hardware evidence unavailable | No physical display is unplugged, and an `original-output` setting and the returning output's old monitor-order position are still missing, so the return path is only synthesised in engine tests. |
| L13 | Workspaces | Hiding the windows of non-active workspaces | Windows backend difference | The default `hide-method` is `SW_HIDE`, which removes windows from the taskbar and Alt+Tab; the closer niri behaviour is only the opt-in `hide-method "cloak"`, whose shell interfaces are private and version-dependent. |
| L14 | Tabs | Tab indicator (colours by active/inactive/urgent, click to select, position/size/gaps/radius) | promoted to done (official dab9a88 cycle) | Settled: `h95_tab_indicator_urgent_colour_overrides_the_selection` passed in the official 2026-10-31 cycle; everything else in the row is covered by H65 plus pure hit/pixel/config tests. |
| L15 | Overview | Mouse in overview: click, drag between/to new workspaces, wheel, right-drag | hardware evidence unavailable | Multi-display overview pointer behaviour is unmeasured, and dragging a real third-party application window is not exercised; H63 covers single-output click/drag/drop/wheel/right-drag. |
| L16 | Overview | Keyboard in overview: all normal binds keep working | hardware evidence unavailable | Multi-display overview keyboard behaviour is unmeasured; H19/H63 cover single-output binds and source lifecycle. |
| L17 | Overview | Hot corner opens overview (top-left default, per-corner and per-output config) | hardware evidence unavailable | Per-output hot corners need a second output; H64 covers the single-output corners, the 120 ms dwell and the inheritance rules. |
| L18 | Overview | Four-finger vertical swipe opens/closes overview | hardware evidence unavailable | No Precision Touchpad was used; only recorded-frame and contact-assembly tests exist and H61 does not exercise the swipe. |
| L19 | Overview | Touch overview (one-finger scroll, long press to move) | hardware evidence unavailable | No touchscreen was used, and Windows reports only the primary contact, so no finger pan or long press is observed. |
| L20 | Overview | `overview { zoom, backdrop-color, workspace-shadow }` | Windows backend difference | The overview host is opaque because foreign DWM thumbnails require it, so a shadow's alpha composites against the backdrop colour instead of a wallpaper layer; H62 covers the options themselves. |
| L21 | Overview | Overview open/close zoom animation (spring) | Windows backend difference | The zoom is a critical spring on wiri's own painted host, not a compositor zoom of live surfaces. |
| L22 | Window rules: match criteria | `at-startup` (first 60 s after launch) | implemented, evidence not reconciled | The ten `h85_*` checks passed in the frozen 861 checkpoint and in R3, whose complete desktop cycles failed; no complete official run is cited for them yet. |
| L23 | Window rules: properties | New-window focus policy and `strict-new-window-focus-policy` | missing/incorrect software | Under strict, niri also leaves a window that opened on another workspace or output unselected within its workspace, which wiri does not do, and the token-granted path has no deterministic desktop trigger. |
| L24 | Window rules: properties | `focus-ring {}` / `border {}` overrides | hardware evidence unavailable | Physical mixed-DPI evidence: the projection is pure-tested at 100-200 % and the live gates run at 96 DPI, and this lab has no display scaled above 100 %. |
| L25 | Window rules: properties | `shadow {}` override | hardware evidence unavailable | As L24. |
| L26 | Window effects | Focus ring | hardware evidence unavailable | As L24. |
| L27 | Window effects | Border | hardware evidence unavailable | As L24. |
| L28 | Window effects | Shadow | hardware evidence unavailable | As L24. |
| L29 | Window effects | Workspace shadow in the overview | implemented, evidence not reconciled | `h95_overview_workspace_shadow_paints_and_off_removes_it` passed in the official dab9a88 cycle; the row stays partial because the opaque host composites the shadow's alpha over the backdrop instead of a wallpaper layer. |
| L30 | Animations | Easing curve set: linear, ease-out-*, cubic-bezier | implemented, evidence not reconciled | The configured Bezier/linear/camera and off controls of `h81_*` passed in the frozen 861 checkpoint and in R3, whose complete desktop cycles failed; no complete official run is cited for them yet. |
| L31 | Animations | Spring animations, velocity hand-off from gestures | hardware evidence unavailable | Gesture velocity hand-off needs a real touchpad to produce a gesture velocity; frame-rate independence, retarget continuity and finite termination are unit-tested. |
| L32 | Animations | `horizontal-view-movement` | hardware evidence unavailable | Gesture hand-off and cross-monitor camera motion are unmeasured; the H17 gates observe resize-driven camera motion on one 96-DPI output. |
| L33 | Animations | `window-resize` (also cross-fade of old and new content) | Windows backend difference | The resize layer cross-fades a live DWM thumbnail: pixel-exact blend fidelity is not reachable, and protected or empty captures silently fall back to plain motion, which no gate exercises. |
| L34 | Animations | `window-open` (scale/fade in) | Windows backend difference | The reveal is a DWM thumbnail overlay; there is no universal pre-show/no-flash guarantee for a foreign window. |
| L35 | Animations | `window-close` (snapshot fades/shrinks) | Windows backend difference | The fade needs a DWM snapshot taken before the HWND dies; an application that destroys itself first, or a protected window, skips the effect. |
| L36 | Animations | `workspace-switch` (vertical slide) | Windows backend difference | The slide moves window sets, not composited shell wallpaper surfaces, and multiple outputs remain unverified. |
| L37 | Animations | `overview-open-close` | Windows backend difference | The camera spring runs on wiri's opaque painted host; the compositor limits recorded at L21 apply. |
| L38 | Animations | Vsync-paced animation clock | Windows backend difference | Windows exposes no foreign-window vblank, so the clock is `DCompositionWaitForCompositorClock` plus `DwmFlush`; cadence and idle CPU under the official cycle are now measured by the two `h90_*` gates that passed in the official dab9a88 cycle, while other refresh rates and multi-output stay unmeasured. |
| L39 | Gestures | Touchpad 3-finger vertical/horizontal | hardware evidence unavailable | No real Precision Touchpad descriptor or swipe was used; only recorded-frame, kinetics and opt-in/off tests exist. |
| L40 | Gestures | Touchscreen move/scroll, second-finger tap | hardware evidence unavailable | No physical touchscreen or complete contact stream was used, and Windows often promotes only the primary contact. |
| L41 | Gestures | Tablet pen: Mod+drag a window | promoted to done (official dba4606 cycle) | Settled: `h93_promoted_pen_carries_the_mi_wp_signature` and `h93_pen_mod_drag_moves_a_floating_window` passed (synthetic pointer device, no physical digitizer). |
| L42 | Gestures | Drag-and-drop edge view scroll | promoted to done (official dba4606 cycle) | Settled: `h93_dnd_edge_view_scroll_moves_the_strip` and `h93_dnd_edge_view_scroll_honours_its_knobs` passed against a real OLE drag source. |
| L43 | Gestures | Drag-and-drop edge workspace switch | promoted to done (official dba4606 cycle) | Settled: `h93_dnd_edge_workspace_switch_steps_once` passed against a real OLE drag source. |
| L44 | Gestures | Drag-and-drop hold-to-activate | promoted to done (official dba4606 cycle) | Settled: `h93_dnd_hold_to_activate_focuses_the_hovered_window` passed against a real OLE drag source. |
| L45 | Gestures | Gesture kinetics: spring decay, rubber-band | promoted to done (official dab9a88 cycle) | Settled: `h90_no_animation_wakeups_at_rest` passed in the official 2026-10-31 cycle, so the reused Phase 2B clock is measured to arm nothing at rest. |
| C1 | Config system | Range-checked values (scale 0..10, accel-speed -1..1, percentages) | implemented, evidence not reconciled | The `h17_*` family passed 48/48 in the official dab9a88 cycle, feeding niri's exact numeric limits and values one past them, but only for the `animations` block: no live gate yet feeds an out-of-range `input { accel-speed }`, `output { scale }` or percentage value and asserts its load diagnostic, and the other bounds items recorded in the matrix remain. |
| C2 | Config system | What a reload re-applies | promoted to done (official dab9a88 cycle) | Settled: `h91_gap_reload_moves_the_window`, `h91_window_rule_reload_affects_a_new_window`, `h91_bind_reload_registers_the_new_chord` and `h91_animation_reload_changes_motion` all passed in the official 2026-10-31 cycle. |
| C3 | Keyboard input | Pick the active layout by name from config | hardware evidence unavailable | Selecting another keyboard layout changes a machine-wide setting that is never written on the shared laptop, so only H68's dry-run log exists. |
| C4 | Keyboard input | `repeat-delay` | hardware evidence unavailable | As C3: `SPI_SETKEYBOARDDELAY` is never written on the shared laptop; only the dry-run and lifecycle tests exist. |
| C5 | Keyboard input | `repeat-rate` | hardware evidence unavailable | As C3: `SPI_SETKEYBOARDSPEED` is never written on the shared laptop. |
| C6 | Keyboard input | `numlock` | Windows backend difference | Windows gives no trustworthy background-thread baseline for Num Lock (`GetKeyState` is queue-relative), so wiri warns and refuses instead of injecting state. |
| C7 | Pointing devices | Per-device-type sections | Windows backend difference | Windows has one system-wide pointer setup, so the four pointer sections share one set of fields and only the OS levers listed in the rows below are written. |
| C8 | Pointing devices | `off` for a device type | hardware evidence unavailable | A Windows mouse or trackpoint cannot be disabled without an admin PnP transaction; only the raw-input touchpad/tablet/touch switches are unit-tested. |
| C9 | Pointing devices | `natural-scroll` | Windows backend difference | The flag inverts wiri's own wheel bindings; applications keep the system scroll direction. |
| C10 | Pointing devices | `accel-speed` | Windows backend difference | Windows exposes no per-device acceleration/sensitivity lever, so the option is parsed, range-checked and logged only. |
| C11 | Pointing devices | `accel-profile "flat"/"adaptive"` | hardware evidence unavailable | `SPI_SETMOUSE` is written for real, but the shared laptop never applies pointer settings, so only the dry-run audit and the unchanged-values gate exist. |
| C12 | Pointing devices | `scroll-factor` | missing/incorrect software | Device and system scroll scaling are not implemented; the legacy `input.mouse.scroll-speed` is parsed and configured binds use 120-unit ticks. |
| C13 | Pointing devices | `scroll-method`, `scroll-button`, `scroll-button-lock` | Windows backend difference | Windows has no lever for them, so the validated values are reported at load and never reach the OS. |
| C14 | Pointing devices | `left-handed` | hardware evidence unavailable | `SwapMouseButton` is system-wide and is not flipped on the shared laptop; only unit tests and the dry-run audit exist. |
| C15 | Pointing devices | `middle-emulation` | Windows backend difference | Windows has no middle-button emulation setting, so the value is reported at load and never written. |
| C16 | Pointing devices | touchpad tap | Windows backend difference | There is no OS lever for tap-to-click through this adapter, so the value is parsed and logged only. |
| C17 | Pointing devices | `touchpad dwt` / `dwtp` | Windows backend difference | `dwt` maps to the Precision Touchpad typing-sensitivity value, but `dwtp` has no Windows equivalent and is only listed as unapplied. |
| C18 | Pointing devices | touchpad `disabled-on-external-mouse` | hardware evidence unavailable | The inverted Windows switch is written only when configured, and the shared laptop never applies pointer settings, so the live gate is a dry-run audit. |
| C19 | Key bindings | `ISO_Level3Shift`/`Mod5` (AltGr), `ISO_Level5Shift`/`Mod3` | Windows backend difference | AltGr is the Windows Ctrl+Alt approximation with no level-5 equivalent, and the hook-only bit is never passed to `RegisterHotKey`. |
| C20 | Key bindings | Key names (case-insensitive XKB keysyms) | Windows backend difference | Only virtual keys exist: shifted punctuation shares one US OEM key, keypad Enter collapses to Return, and layout-dependent Unicode/dead keys and Linux-only keysyms are unsupported. |
| C21 | Key bindings | Touchpad scroll triggers | Windows backend difference | The names alias the shared Windows wheel stream; there are no device-specific thresholds and a touchpad cannot be told apart from a wheel. |
| C22 | Key bindings | Tablet stylus triggers | promoted to done (official dba4606 cycle) | Settled: `h93_promoted_pen_carries_the_mi_wp_signature` and `h93_pen_barrel_runs_its_tablet_stylus_bind` passed (synthetic pointer device, no physical digitizer), on top of the `h78_tablet_stylus_bind_decode_and_registration` decode gate. |
| C23 | Hotkey overlay | `hotkey-overlay-title` Pango markup | promoted to done (official dab9a88 cycle) | Settled: the six `h89_*` gates in `lane-pango.ps1` passed in the official 2026-10-31 cycle. |
| C24 | Hotkey overlay | Rendered per output at output centre, scaled by output scale | hardware evidence unavailable | Only the focused output gets a host, and 125/150/200 % scale plus negative-origin geometry were never observed because this machine has one 96-DPI output. |
| C25 | Screenshots | OS conflicts: Print Screen / Win+Shift+S | Windows backend difference | The Snipping Tool owns Print and Win+Shift+S; wiri binds Ctrl+Alt+P and cannot take over the OS capture button without writing a per-user setting. |
| C26 | IPC transport | `niri msg` structure: one subcommand per request | missing/incorrect software | `--print-request` prints wiri's adjacent-tagged request instead of niri's JSON, the `output` custom-mode/modeline words are absent and niri's top-level `startup` command has no counterpart. |
| C27 | IPC requests | Outputs (name, make, model, serial, physical size, modes, vrr, max-bpc) | Windows backend difference | Identity, EDID, modes and geometry are reported, but Windows offers no per-output VRR or max-bpc control, so those fields stay null instead of being fabricated. |
| C28 | IPC requests | `PickColor` | Windows backend difference | The pick is a normalised SDR RGB of the clicked pixel; HDR colour conversion is unsupported. |
| C29 | IPC requests | `Action(Action)` (every action, typed) | implemented, evidence not reconciled | Six executor-evidence gaps remain; the nine focus-neutral `*ById` actions (`h92_by_id_*`) passed in the official dab9a88 cycle and no longer await a run; nine compositor-protocol actions stay N/A. |
| C30 | IPC requests | `Output{output, action}` (temporary display mutations) | hardware evidence unavailable | Every temporary display mutation is synthetic-unit-tested only; nothing is applied to real hardware. |
| C31 | Outputs | Hot-plug: connect/disconnect re-evaluates outputs | hardware evidence unavailable | One physical output means no live plug/unplug, and mode/position/rotation requests are applied at startup and reload only. |
| C32 | Outputs | `off` | hardware evidence unavailable | Live runs use an unmatched `off` block and never detach hardware; only planner/preparation tests cover the transaction. |
| C33 | Outputs | `mode "WxH@R"` | hardware evidence unavailable | Live H29 requests the current dimensions only, and niri's auto-pick on an invalid mode is intentionally not implemented. |
| C34 | Outputs | `scale` | Windows backend difference | An explicit positive scale overrides wiri's stored layout scale only; Windows DPI is untouched, so logical-pixel sizing and real Windows scale changes stay outside the row. |
| C35 | Outputs | `transform` | hardware evidence unavailable | No physical rotation was executed; only synthetic CCD planning is tested. |
| C36 | Outputs | `position x= y=` | hardware evidence unavailable | No physical repositioning is tested, and coordinates are Windows desktop pixels rather than a Wayland logical-output space. |
| C37 | Outputs | `variable-refresh-rate` | Windows backend difference | There is no per-output VRR switch to write, so the option is parsed and reported only. |
| C38 | Outputs | `background-color` (deprecated, moved to `layout`) | Windows backend difference | The deprecated per-output key maps to the layout-level value, but wiri paints no per-output backdrop because Windows owns the desktop behind tiled windows, so it only reaches the overview; `h95_deprecated_output_background_colour_is_the_overview_backdrop` passed in the official dab9a88 cycle and pins that half, while the per-output backdrop itself stays a Windows limit. |
| C39 | Outputs | `backdrop-color` (also `overview { backdrop-color }`) | Windows backend difference | Output-specific backdrop overrides are outside the adapter and the opaque DWM host composites alpha over black. |
| C40 | Outputs | `hot-corners {}` per output | hardware evidence unavailable | Per-output hot corners need a second output; H64 covers inheritance, `off`, default and named corners on one display. |
| C41 | Outputs | `layout {}` overrides per output | implemented, evidence not reconciled | H70 covers precedence and reload, but the output-name alias and inheritance matching is not observed live; one owned output block written under the reported name would settle it. |
| C42 | Actions (non-layout) | `spawn` (no shell, `~` expansion, `allow-when-locked`) | Windows backend difference | Secure-desktop locked behaviour is a Windows qualification; quoting/PATH lookup, `ChildEnvironment` and program-only home expansion are covered by H49/H69. |
| C43 | Actions (non-layout) | `toggle-keyboard-shortcuts-inhibit` and `allow-inhibiting=false` | implemented, evidence not reconciled | `h95_shortcuts_inhibit_blocks_inhibitable_binds_only` passed in the official dab9a88 cycle; the row stays partial because Windows has no per-client inhibition protocol for the automatic path. |
| C44 | Misc top-level config | `cursor { hide-when-typing }` | Windows backend difference | `SPI_SETMOUSEVANISH` is a per-user Windows setting shared with every other application, not a compositor cursor surface. |
| C45 | Misc top-level config | `cursor { hide-after-inactive-ms }` | Windows backend difference | The pointer is hidden by swapping the system cursor scheme with a transparent mask, not by a compositor surface. |
| C46 | Misc top-level config | Layer-shell components: bars reserve space, launchers overlay, wallpaper tools | missing/incorrect software | The optional built-in status bar does not reserve space itself, so third-party bars work only through the `reserve_*` config and the `ReserveArea` IPC path. |
| C47 | Debug options | `strict-new-window-focus-policy` | implemented, evidence not reconciled | The flag is parsed, honoured and unit-tested; the four `h92_new_window_*` strict/default gates passed in the official dab9a88 cycle, and the shared strict cross-workspace and token gaps tracked at L23 keep the row partial. |

## What each open evidence row needs

| Row | Gate to look up in a complete official run |
| --- | --- |
| L4 | none yet — needs a camera-motion sample for an admitted window, not for an explicit move (design note in the row) |
| L8 | `h95_center_column_centres_variable_width_edge_columns` — passed in the official dab9a88 cycle; the row stays partial for differing physical outputs |
| L11 | `h95_mod_drag_moves_a_floating_window`, `h95_mod_drag_resizes_a_floating_window`, `h95_mod_drag_escape_cancels_to_the_pre_drag_rect` — all passed in the official dab9a88 cycle (row settled) |
| L14 | `h95_tab_indicator_urgent_colour_overrides_the_selection` — passed in the official dab9a88 cycle (row settled) |
| L22 | `h85_initial_scan_startup_rules`, `h85_new_admission_startup_rules`, `h85_title_change_keeps_startup_rules`, `h85_reload_keeps_startup_rules`, `h85_real_deadline_expires_without_reload_reset`, `h85_expiry_updates_existing_limits_and_opacity`, `h85_expiry_does_not_replay_open_properties`, `h85_expiry_remains_consumed`, `h85_post_expiry_admission_uses_runtime_rules`, `h85_cleanup` (lane `lane-startup-rules.ps1`) |
| L29 | `h95_overview_workspace_shadow_paints_and_off_removes_it` — passed in the official dab9a88 cycle; the row stays partial for the opaque host's shadow alpha |
| L30 | `h81_*` (lane `lane-a-camera-motion.ps1`) |
| L45 | `h90_no_animation_wakeups_at_rest` — passed in the official dab9a88 cycle (row settled) |
| C1 | the `h17_*` numeric-limit gates passed in the official dab9a88 cycle, but they cover the `animations` block only; an out-of-range `input { accel-speed }`, `output { scale }` or percentage value plus its load diagnostic still has no gate, and the other bounds items above remain |
| C2 | `h91_gap_reload_moves_the_window`, `h91_window_rule_reload_affects_a_new_window`, `h91_bind_reload_registers_the_new_chord`, `h91_animation_reload_changes_motion` — all passed in the official dab9a88 cycle (row settled) |
| C23 | `h89_*` (lane `lane-pango.ps1`) — all six passed in the official dab9a88 cycle (row settled) |
| C29 | `h92_by_id_*` (lane `lane-focus.ps1`) — passed in the official dab9a88 cycle; the row stays partial for the census's six executor-evidence partials |
| C41 | none yet — needs one owned output block written under the reported output name |
| C43 | `h95_shortcuts_inhibit_blocks_inhibitable_binds_only` — passed in the official dab9a88 cycle; the row stays partial for Windows' absent per-client inhibition protocol |
| C47 | `h92_new_window_default_heuristic_focuses_a_window_without_the_foreground`, `h92_new_window_default_heuristic_keeps_a_fullscreen_window_focused`, `h92_new_window_strict_policy_does_not_focus_a_window_without_the_foreground`, `h92_new_window_strict_policy_still_honours_an_explicit_focus_request` (lane `lane-focus.ps1`) — all passed in the official dab9a88 cycle; the row stays partial for the focus-policy gaps tracked in the layout row |

## The eight `h95_*` gates added by this lane

`scripts/live-phases/lane-evidence.ps1`, dot-sourced from `scripts/live-test.ps1` next to the
other lanes inside `try { . lane } catch { LanePhaseError 'lane_evidence' $_ }`, with its gate
defaults declared in the summary block. Both files are parse-checked with the PowerShell Parser
API. **All eight ran in the official `dab9a88` cycle of 2026-10-31 and passed**, so the rows whose
only remaining condition was one of these gates are now `done`; the rows that also record a
hardware, mixed-DPI, multi-output or Windows-backend condition stayed `partial` with the passed
gate named next to it.

| Gate | Settles | Evidence it takes |
| --- | --- | --- |
| `h95_overview_workspace_shadow_paints_and_off_removes_it` | L29 | A real screen pixel in the offset-10/softness-0 shadow band against the configured backdrop, then the same point after `workspace-shadow { off; }`. |
| `h95_deprecated_output_background_colour_is_the_overview_backdrop` | C38 (overview half) | The host's published backdrop and a screen pixel with the deprecated per-output `background-color` and no `overview { backdrop-color }`. |
| `h95_center_column_centres_variable_width_edge_columns` | L8 | 320 px and 640 px first/last columns centred on the work-area centre, with the narrow column's earlier minimal-reveal position as the control. |
| `h95_tab_indicator_urgent_colour_overrides_the_selection` | L14 | Configured active/inactive pixels, then `SetWindowUrgent` on the hidden tab, then `UnsetWindowUrgent` restoring inactive, with the tab selection unchanged. |
| `h95_mod_drag_moves_a_floating_window` | L11 | A real `SendInput` Mod(Alt)+left grab: held preview offset, persisted rectangle, size unchanged, still floating on its workspace. |
| `h95_mod_drag_resizes_a_floating_window` | L11 | A real Mod(Alt)+right grab: the persisted width/height delta with the origin held. |
| `h95_mod_drag_escape_cancels_to_the_pre_drag_rect` | L11 | Escape during the grab returns the window to the pre-drag rectangle, and it stays floating. |
| `h95_shortcuts_inhibit_blocks_inhibitable_binds_only` | C43 | Ctrl+Alt+F8 toggles floating twice, `toggle-keyboard-shortcuts-inhibit` then blocks it, an `allow-inhibiting=false` bind still fires, and releasing the inhibitor restores it; every observation is the engine's own `window-list` field. |

Each group has its own `try`/`catch`, owned fixtures only, polls observable conditions instead of
racing fixed sleeps, and its `finally` releases any held synthetic input before quitting its
private daemon. The lane starts its own daemon with `input { mod-key "Alt" }` and
`animations { off; }`; it never writes a display, pointer or keyboard setting and never touches a
window it does not own.
