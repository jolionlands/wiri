# Next lanes: surveying the partial parity rows (2026-10-27)

> **Historical survey, superseded for assignment.** This file records the
> `0821983` snapshot. Following the integrated, PR #2 and qualified R4 follow-ups,
> current matrices contain **305 done / 92 partial / 88 N/A** (397 applicable
> rows), including five appearance/DPI and two floating-preset corrections, not the 138
> partial rows below. The active
> [full-parity plan](../superpowers/plans/2026-10-28-full-parity.md) checks the
> pinned source and current implementation before assigning work. In particular,
> center-column, floating Mod grabs, gesture spring velocity/rubber-band, spawn
> home expansion and h60/h79 coverage already exist. The proposed fullscreen
> refusal gates below require source-semantic review, not blind implementation.

## Current partial-row audit, 2026-10-29

The initial 94 partial rows were rechecked against the implementation and pinned
niri source. The action census exposed two further preset-height rows previously
marked done and confirmed software defects in both broad action/CLI rows.
The qualified R4 source/native follow-ups close the `gaps`, `struts` and curated
hotkey-list software gaps. Primary categories now sum to 93; a row can have
secondary limits too. See the [R4 evidence report](2026-10-29-tridaris-r4.md#qualified-native-spacing-and-panel-follow-ups).

| Primary remaining condition | Layout | Config/input/IPC | Total |
| --- | ---: | ---: | ---: |
| Missing or incorrect software | 15 | 17 | 32 |
| Implemented; evidence needs reconciliation | 10 | 3 | 13 |
| Hardware evidence unavailable | 7 | 2 | 9 |
| Windows-specific backend differences | 14 | 25 | 39 |
| **Total partial rows** | **46** | **47** | **93** |

These are prioritization categories, not completion claims or 32 independent
coding tasks. Several metric rows share one implementation. A Windows-specific
category describes the current adapter's approximation or rejected capability;
it does not establish that every possible Windows implementation is impossible.
Evidence-only rows retain partial status until their exact remaining condition
is reconciled with source and native proof.

R4 qualified fractional spacing, signed struts and hotkey curation through pure
and native checks; those three rows are no longer remaining software work.
Signed per-axis scroll factors also passed their R4 source/native checks, with
physical device routing still separate. The following queue covers appearance scaling,
general viewport anchoring and clipping, application self-resize reconciliation,
floating fullscreen transitions, admission focus policy, and presentation-aware
pacing. Native constraint sampling alone does not finish self-resize behavior.
Canonical typed action fields, automatic output values, output-position CLI
grammar and floating preset heights also have source-confirmed software gaps;
matching action names alone does not establish their field or effect contracts.

Several earlier gap descriptions below are stale: center-column, floating Mod
grabs and positioning, typed workspace IDs and sizes, cubic Bezier, startup-rule
timing, scalar wheel scaling, and environment-aware spawn now have implementations.
Returning-output order and a literal `original-output` option are not pinned gaps:
niri appends returning outputs and uses repeated `open-on-output` preferences.
Per-device routing, global input ownership, multi-output acceptance and full Pango
markup remain distinct questions. A small markup subset cannot close that row.

Use the [active implementation plan](../superpowers/plans/2026-10-28-full-parity.md)
for assignments. The survey below remains historical evidence.

Read-only survey of every row still marked `partial` in both parity matrices, written at
`0821983` on branch `wiri/shine-20261024`. Nothing was built, run, edited or reconfigured:
no live cycle, no wiri instance, no system setting. The classifications below rest on (a) the
status and evidence text already in the matrices, and (b) targeted greps of `src/` and
`scripts/` to check whether a row's remaining delta is a missing live gate, a missing piece of
code, or a Windows limit. Where a grep could not settle a row it is marked **verify** and is
kept out of the ranked list.

Current evidence base: the **2026-10-27 official cycle at `cc3920d`** — **761/761 enforced
gates passed, 0 failed**, 88 information entries (recorded in [PERF.md](../PERF.md) and
[README.md](README.md)). Gate counts below assume that population; a gate that exists and ran
there is called "gated" even when its row text still says otherwise.

At this survey's snapshot, the README recount was **layout 141 done / 68 partial / 17 N/A** and
**config-input-IPC 117 done / 70 partial / 72 N/A**, 0 missing in both. The "68 partial rows"
of the task is the layout matrix; this survey covers **all 138 partial rows** (68 + 70).
Row numbers below are this document's own ordinal over each matrix's partial rows in file
order — the matrices have no stable row ids.

## 1. Classification summary

| Class | Layout (68) | Config/input/IPC (70) | Total |
| --- | ---: | ---: | ---: |
| **(a) live-gateable now** | 32 | 28 | **60** |
| — (a-1) gate exists and ran at `cc3920d`, row text is stale | 24 | 22 | 46 |
| — (a-2) implemented + unit-tested, no live gate (or a vacuous one) | 8 | 6 | 14 |
| **(b) needs real hardware** | 11 | 18 | **29** |
| **(c) Windows approximation** | 16 | 19 | **35** |
| **(d) still needs code first** (added; see note) | 9 | 5 | **14** |
| total | 68 | 70 | 138 |

**Why a fourth class.** The three requested classes assume the implementation exists. For 14
rows it does not: the matrix's own "Windows approach" column is still a plan ("add Bezier
solver", "parse PositionChange", "register wiri's own bar as an AppBar"). Those rows are
neither live-gateable now nor blocked by hardware, and calling them "approximations" would
hide real work. They are listed separately in section 6 so the ranking stays honest. This is
an addition to the requested taxonomy, not a substitution.

**The finding that matters.** 46 of the 60 class-(a) rows are not missing a gate at all. Their
gate exists, is wired into `scripts/live-test.ps1`, and passed in the 761-gate cycle, but the
row still carries an earlier "unit-tested only" or "live pending lead cycle" tag. That is a
records problem, and it is the cheapest parity win available — see Batch 0 in section 7.

## 2. Class (a-1): gate exists and ran at `cc3920d` — 46 rows

Grouped by the gate(s) that already cover them. Nothing to implement; the row text and, where
relevant, the matrix counts need to cite the cycle.

### Layout (24)

| Row (abbreviated) | Existing gate(s) |
| --- | --- |
| `empty-workspace-above-first` | `h76_empty_workspace_above_first` |
| `preset-window-heights` | `h76_preset_window_height_and_back` |
| `fullscreen-backdrop-color` | `h76_fullscreen_backdrop_colour` |
| `switch-preset-window-width` / `-back` | `h76_preset_window_width_and_back` |
| `switch-preset-window-height` / `-back` | `h76_preset_window_height_and_back` |
| `center-visible-columns` | `h76_center_visible_columns` |
| `swap-window-left` / `swap-window-right` | `h76_swap_window_left`, `h76_swap_window_right` |
| `move-window-down-or-to-workspace-down` / `-up` | `h76_move_window_down_or_to_workspace`, `h76_move_window_up_or_to_workspace` |
| `move-column-left/right-or-to-monitor-*` | `h76_move_column_or_to_monitor` (single display only) |
| `move-column-to-index` | `h76_move_column_to_index` |
| `toggle/set/unset-window-urgent` | `h48_urgency_set_unset_toggle_nonfocusing`, `h48_urgency_events_and_focus_clear` |
| `toggle-window-rule-opacity` | `h76_toggle_window_rule_opacity` |
| Auto-float dialogs (transient parent, fixed size) | `h43_owned_dialog_auto_floats_over_owner`, `h43_normal_toplevels_stay_tiled` |
| `focus-floating` / `focus-tiling` / `switch-focus-between-*` | `h76_focus_floating_and_tiling`, `h76_switch_focus_between_layers` |
| Named workspaces exist when empty | `h8_named_workspaces_exist_from_the_start`, `h33_named_startup_order` |
| Tabbed column holds fullscreen/maximized windows | `h65_fullscreen_suppresses_across_tab_switches` |
| `horizontal-view-movement` | `lane-a-animations.ps1` decodes and runs these blocks live |
| Interactive move (Mod+left-drag) | `h59_move_drops_into_actual_destination_column` |
| Interactive resize (Mod+right-drag) | `h59_resize_persists_after_drop` |
| Double-click top/side edge | `h59_top_edge_double_resets_height`, `h59_side_edge_double_toggles_full_width` |
| Mod+middle-drag view movement | `h58_horizontal_follows_pointer_while_held` |
| Mod+middle-drag vertical workspace switch | `h58_vertical_switches_workspace` |
| Pointer gestures follow the configured mod-key | lane T runs `input { mod-key "Alt" }`; the whole `h58_*`/`h59_*` set passes under it |
| Wheel binds (Mod+wheel) | `h23_wheel_up/down/left/right_bind_fires_and_consumes`, `h23_high_resolution_wheel_accumulates` |

Note the cross-lane coverage: the layout matrix's Lane T rows for wheel binds and the Mod
prefix are gated by **lane B's** `h23_*` set, and the auto-float row by **lane G's** `h43_*`
gate. Neither the row text nor the README points at that evidence.

### Config/input/IPC (22)

| Row (abbreviated) | Existing gate(s) |
| --- | --- |
| `load-config-file [--path]` (IPC and CLI) | `h49_load_config_file_and_reload_current` |
| `mod-key` shared with the mouse hook | lane T's `mod-key "Alt"` runs with `h58_*`/`h59_*` |
| `quit [skip-confirmation]` | `h49_quit_confirmation_cancel`, `h49_quit_skip_confirmation_exits` |
| `power-off-monitors`, `power-on-monitors`, `suspend` | `h49_power_actions_dry_run_only` |
| `show-hotkey-overlay` | `h75_hotkey_overlay_shown_at_startup` |
| `toggle-touchpad` | `h75_toggle_touchpad_event_dry_run` |
| `switch-layout next/prev/index` | `h48_keyboard_layout_next_prev_index` |
| Urgency over IPC (`toggle/set/unset-window-urgent`) | `h48_urgency_set_unset_toggle_nonfocusing`, `h48_urgency_events_and_focus_clear` |
| `toggle-overview`, `open-overview`, `close-overview` | `h48_overview_open_close_idempotent` |
| `workspace "name"` declarations (single output) | `h8_named_workspaces_exist_from_the_start`, `h33_named_startup_order` |
| Screen-reader tree, workspace-switch announcement, exit/Alt-Tab/screenshot texts | `h77_*` in `lane-a11y.ps1` |
| `block-out-from "screencast"` / `"screen-capture"` | `h77_block_out_from_layer_rule_excludes_overlay`, `h77_block_out_from_window_rule_excludes_overview_host`, `h77_block_out_from_absent_leaves_affinity_none` |
| `focus-follows-mouse` (windows under the pointer) | `h25_hover_targets_nested_client_child`, `h25_focus_does_not_change_before_delay`, `h25_focus_changes_after_delay`, `h25_leaving_target_cancels_pending_focus`, `h25_zero_delay_hover_focuses_without_reentry`, `h25_disabled_warp_keeps_cursor` |

## 3. Class (a-2): implemented + unit-tested, live gate missing or vacuous — 14 rows

These are the real candidates for a new delegation batch. "Verify" means the survey's grep
found the implementation but the row's remaining delta should be re-read before writing a gate.

| # | Matrix row | Implementation found | Gap |
| --- | --- | --- | --- |
| L2 | Window height in a column: auto (weighted), fixed, proportion | `Tile::height_proportion`, `preferred_height`, resolved in `engine.rs` and `presets.rs`; `h76_preset_window_height_and_back` covers only preset cycling | No gate asserts the three height modes, min/max clamping, or survival across admission/removal |
| L20 | Fullscreen and maximized windows cannot be floating | `float_stack.rs` excludes `fullscreen_windows` from the float layer | **Verify** the action-time guard, then gate `toggle-window-floating` on a fullscreen window |
| L40 | `recent-windows-close` fade-out animation | `overlay/recent_windows*.rs`; gate exists | `h76_recent_windows_close_fade` only asserts "no warnings, still alive" — it does not assert a fade |
| L41 | `recent-windows { off }` and `focus_timestamp` in IPC `Window` | `enabled` honoured in `engine/recent.rs:99,146` and `input/recent.rs:412,447`; `focus_timestamp` gated by `h37_focus_timestamp_event` | `off` is never exercised live |
| L53 | `recent-windows-close` animation kind | same as L40 | same as L40 |
| L63 | Drag-and-drop edge view scroll (`dnd-edge-view-scroll`) | `input/dnd.rs` (`edge_speed`, zone, one-shot frame timer) + `engine/dnd.rs` | `h60_*` only checks the drag host exists; no gate drags to an edge |
| L64 | Drag-and-drop edge workspace switch (`dnd-edge-workspace-switch`) | same modules | no gate holds at the edge past the delay |
| L65 | Drag-and-drop hold-to-activate | same modules (dwell) | no gate |
| C1 | Range-checked values (`scale`, `accel-speed`, percentages) | typed bounds in `config/validation.rs` | no gate feeds out-of-range values and expects diagnostics |
| C2 | What a reload re-applies (binds, outputs, rules, layout) | event-driven reload; `h12_*`, `h13_*` cover binds and rules | layout/output half of the reload scope is ungated |
| C24 | niri action names accepted in binds | action vocabulary in `config/decode.rs` + `input/mod.rs` | no completeness gate — a new action can ship with no name test |
| C27 | Curated Important Hotkeys list (~15 core actions) | `overlay/hotkey_rows.rs::curated_rows` with unit tests; `h75_*` covers startup/skip/hide-not-bound | curated content not asserted live |
| C28 | `hotkey-overlay-title` per bind, markup, `null` hides | `hotkey_rows.rs`; null path gated by `h75_hotkey_overlay_hide_not_bound` | Lane PANGO parsed, measured and painted the markup (`pango_markup.rs`, `markup_text.rs`) with `h89_*` gates added but not yet run; row text/status pending the lead cycle |
| C51 | `spawn` home expansion | executable-only `~` expansion plus `ShellExecuteExW` fallback | no gate (low value; the row may belong in class (c) instead) |

## 4. Class (b): needs real hardware — 29 rows

**Layout (11):** scrolling-strip cross-output clipping and multi-monitor spill; per-output
`layout {}` overrides; workspaces surviving monitor unplug; overview pointer drag of foreign
windows and multi-display; overview keyboard with several outputs; hot corners per output;
four-finger vertical swipe (Precision Touchpad); touch overview; three-finger touchpad
gestures; touchscreen move/scroll; tablet pen drag.

**Config/input/IPC (18):** per-device-type `off` (PnP disable); touchpad tap, `dwt`/`dwtp`,
`disabled-on-external-mouse`; touchpad-scroll bind triggers; tablet stylus buttons; output
`off`/`mode`/`position`/`transform`/`scale`; variable-refresh-rate; hot-plug re-evaluation;
outputs in IPC (make/model/serial/VRR/max-bpc); `Output{output, action}`; temporary output
actions; per-output hot corners; per-output layout overrides; hotkey overlay placed at each
output's centre.

Every one of these needs a second display, a real PTP touchpad, a pen/touch digitizer, or a
display topology transaction this laptop cannot perform. They are **not** delegation
candidates for this runner: a gate would either be skipped (and count as a false pass) or
change machine state.

## 5. Class (c): Windows approximation — 35 rows

**Layout (16):** fractional/logical `gaps` and `struts` at non-100% DPI (rounding to physical
pixels); animation ownership of new-window reveal; floating-layer stacking repair instead of
compositor-atomic stacking; `hide-method "cloak"` / `SW_HIDE`; tab indicator host vs. drawn
tabs; config `overview { zoom, backdrop-color, workspace-shadow }` (opaque host); overview
zoom and workspace shadow; resize transactions (no cross-app repaint gating); the Lane K
transitions (`window-resize`, `window-open`, `window-close`, `workspace-switch`,
`overview-open-close`); vsync-paced clock.

**Config/input/IPC (19):** pointer settings that are system-wide (`accel-speed`,
`accel-profile`, `scroll-factor`, `natural-scroll`, `left-handed`); keyboard repeat and Num
Lock (`SPI_SETKEYBOARDDELAY`/`SPEED`, safe refusal); per-device-type sections; `ISO_Level3/5`
(AltGr approximation); XKB keysym names mapped to virtual keys; `cursor { hide-when-typing }`
(`SPI_SETMOUSEVANISH`) and `hide-after-inactive-ms` (`SetSystemCursor` transparency);
`background-color` and `backdrop-color` (the shell owns the wallpaper); `PickColor` SDR only;
Print Screen / Win+Shift+S reservation; layer-shell components (no bar protocol);
`switch-layout` HKL posting.

These stay `partial` by design. The right close-out is a one-line "approximation, accepted"
status, not a lane.

## 6. Class (d): still needs code — 14 rows

Listed so they are not mistaken for gate work:

| Matrix row | Missing piece |
| --- | --- |
| Windows keep the final say over their size (min height, max size) | same `WM_GETMINMAXINFO` answer as the width, plus `LOCATIONCHANGE` / `EVENT_SYSTEM_MINIMIZESTART/END` invalidation |
| `center-column` | explicit centring outside the reveal policy (**verify** — negative offsets exist since Lane G) |
| `move-floating-window` (x/y set/adjust/proportion) | no `move-floating-window` entry in the action vocabulary at all |
| Mouse move/resize of floats via the Mod key | the LL-hook grab is not extended to the float layer |
| `at-startup` (first 60 s after launch) | only startup-enumerated windows get `at_startup`; no 60 s clock |
| New-window focus policy (heuristic auto-focus) | done in the focus-core lane (heuristic, `debug { strict-new-window-focus-policy }`, h92_new_window_* gates); remaining gaps are listed in the layout-behaviour matrix row |
| Easing curve set (linear, ease-out-*, Bezier grammar) | no Bezier solver |
| Spring animations: typed kinds and gesture velocity | kinds exist in config; the velocity wiring to the driver does not |
| Gesture kinetics (spring decay, rubber band) | needs the above |
| `scroll-method` / `scroll-button` / `scroll-button-lock` | LL-hook emulation not written |
| `middle-emulation` | near-simultaneous L+R detection not written |
| `Action(Action)` for every action, typed | some variants have no typed arm |
| `toggle-keyboard-shortcuts-inhibit` per-client | only the manual global inhibitor exists |
| Layer-shell components / bar reserving space | no `SHAppBarMessage` use anywhere in `src/` |

## 7. Ranked next lanes

### Batch 0 (no code, do this first): one cycle that names H75-H77

46 rows above already have passing gates. The blocker is that lanes L2, P and A11Y have never
had their own documented official cycle, so their rows keep saying "live pending". One
locked `scripts/live-test.ps1` run at the next head, with `scripts/live-report.py` output
recorded the way `cc3920d` was, plus a recount commit, converts most of those 46 rows to
`done` without touching `src/`. Acceptance: `summary.json` shows 0 failed enforced gates and
the record names `h75_*`, `h76_*` and `h77_*` explicitly; the recount then moves a stated
number of rows, one row per named gate family.

### Lane 1 — column height modes (layout row `Window height in a column`)

*Files:* `scripts/live-phases/lane-l2.ps1` (gates), `src/layout/engine/sizing.rs` and
`src/layout/engine/presets.rs` (only if a defect shows up), then the layout matrix row.
*Gates:* `h76_column_height_proportion_and_fixed`, `h76_column_height_survives_admission`,
`h76_column_height_reset_returns_to_auto`, added to the existing `h76_*` family so the L2
phase stays one lane.
*Acceptance:* with `preset-window-heights { fixed 240; proportion 0.5; }` and three tiles in
one column, each visible height matches the resolved spec within 1 px; opening a fourth
window leaves the first three heights unchanged; `set-window-height "30%"` then
`reset-window-height` returns that tile to weighted auto and the column fills the view.
This is the highest-value class-(a-2) row: it is the layout knob niri users reach for first,
the implementation is already in the tree, and the risk is a rectangle assertion, not timing.

### Lane 2 — drag-and-drop edges (layout rows `dnd-edge-view-scroll`, `dnd-edge-workspace-switch`, `hold-to-activate`)

*Files:* `scripts/live-phases/lane-t.ps1` (three new `TGate`s and a drag fixture that holds
the left button with `SendInput`), `src/input/dnd.rs` (a debug-log line per edge frame so the
gate has evidence beyond rectangle deltas), the drop-target model in `src/layout/engine/dnd.rs`.
*Gates:* `h60_dnd_edge_scroll_moves_the_strip`, `h60_dnd_edge_switch_fires_once_after_the_delay`,
`h60_dnd_no_frame_timer_when_idle`.
*Acceptance:* dragging an owned window into the configured `trigger-width` of the right edge
shifts the column rectangles left while the drag is held and stops within one frame of leaving
the zone; holding past `delay-ms` activates the next workspace exactly once (no repeat while
the pointer stays); with `gestures {}` removed, the idle `p_idle_context_switches_per_s`
information entry is unchanged from the 9.16/s baseline — i.e. the edge timer only exists
during a drag. Medium risk: dwell timing is the flaky part, so assert direction and
once-only-ness, not frame cadence.

### Lane 3 — niri action-name completeness (config row `Niri action names accepted in binds`)

*Files:* `scripts/live-phases/lane-c2-binds.ps1`, the action table in `src/config/decode.rs`,
`src/input/mod.rs`, and the IPC action vocabulary in `src/ipc/`.
*Gates:* `h79_action_names_all_parse`, `h79_action_names_all_run_over_ipc`.
*Acceptance:* the gate enumerates the same action list the decoder ships (one source of truth,
so a new action cannot land untested), writes a fixture config that binds one key per name,
and requires `validate-config` to report zero diagnostics and the run log to contain no
`unknown action` line; then `wiri-ctl action <name>` must succeed for every name (with
required arguments). Highest value per hour for anyone porting a niri config, lowest risk of
the five: it is text in, diagnostics out.

### Lane 4 — `recent-windows { off }` (layout row `recent-windows { off } and focus_timestamp`)

*Files:* `scripts/live-phases/lane-a-recent.ps1`, `src/layout/engine/recent.rs`,
`src/input/recent.rs`, `src/config/recent_windows_config.rs`.
*Gates:* `h20_recent_windows_off_creates_no_host`, `h20_recent_windows_off_still_timestamps_focus`.
*Acceptance:* with `recent-windows { off; }` no `WiriRecentWindowsHost` window appears while
switching between two owned windows, the recent-windows bind passes through to the
application, and `window_focus_timestamp_changed` still fires exactly once per confirmed
focus; flipping to `{ on; }` in a reload later in the same run makes the host appear within the
existing H20 timeout. The `focus_timestamp` half is already covered by
`h37_focus_timestamp_event`, so this lane closes the row rather than half of it.

### Lane 5 — fullscreen/maximized windows cannot float (layout row of that name)

*Files:* `scripts/live-phases/lane-l2.ps1`, `src/layout/engine/float_stack.rs` (the guard at
line 126), `src/input/mod.rs` (float action dispatch). **Verify the guard runs at action time
before writing the gate** — the survey only found the float-layer exclusion.
*Gates:* `h76_fullscreen_window_refuses_floating`, `h76_maximized_window_refuses_floating`,
`h76_unfullscreen_then_float_works`.
*Acceptance:* a fullscreen window stays fullscreen and reports `floating = false` in
`wiri-ctl windows` after `toggle-window-floating` and `move-window-to-floating`; leaving
fullscreen and repeating the action floats it into the strip without a second layout pass;
the neighbours never move during the refused action (rectangle snapshot before/after).

## 8. Watch items

- **`h9_overlapping_terminal_pairs` moved 1 → 0 without a dedicated fix.** The `af3eb30`
  baseline recorded 1 overlapping terminal pair with an 11 px overlap; the `cc3920d` cycle
  recorded 0. The likely cause is the event-armed reconcile (`2c53631`), but nothing in either
  record proves that, and the `cc3920d` record does not carry the `h9_*` numeric entry at all
  (PERF.md lists it among the entries that run did not record). Treat it as unproven: the next
  official cycle should record the `h9_*` values and, if the pair is still 0, say so
  explicitly rather than letting a silently fixed defect become folklore.
- **Stale row text is now the largest single source of "partial".** 46 rows describe
  themselves as unverified while a passing gate exists. Any future recount should be
  generated from the gate list, not hand-edited, or the same drift will reappear.
- **Lanes L2, P and A11Y still lack their own documented cycle.** Batch 0 exists because of
  this; until it runs, their rows rest on unit tests in the README's own words.
- **Row-to-gate mapping is cross-lane.** Lane T's wheel and Mod-prefix rows are gated by lane
  B, the auto-float row by lane G, `focus_timestamp` by lane H. Whoever writes the next lane
  should grep `scripts/live-phases/` for the behaviour, not for the row's lane prefix.
- **Lane S's power actions and quit dialog** are gated only in dry-run form by
  `h49_power_actions_dry_run_only`, which is the correct choice on this laptop; do not read
  that gate as evidence that a real suspend or monitor power-off was exercised.

## 9. What this survey did not do

No build, no `cargo test`, no live cycle, no wiri instance, no IPC, no registry or system
setting, no process killed. Every statement about a gate being wired comes from reading
`scripts/live-test.ps1` (all phases are dot-sourced from `scripts/live-phases/`) and the lane
files; every statement about an implementation comes from grepping `src/`. Rows marked
**verify** need a code read before a gate is written for them.
