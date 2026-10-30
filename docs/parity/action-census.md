# Pinned niri action and CLI contract census

Pin: niri `e19df12cf81ffe805609fee497da1a097066ad2f`.  Files read from that commit:
`niri-ipc/src/lib.rs` (the typed `Action` enum, lines 194-962, and the `FromStr` of the
argument types, 1791-1915), `niri-config/src/binds.rs` (the KDL `Action` enum, 105-366, and the
`From<niri_ipc::Action>` conversion, 403-716) and `src/cli.rs` (`niri msg`).  wiri: branch
`wiri/parity-continue-20261030` plus the census lane (`cf8c23a`).  This is the "exhaustive
pinned action/CLI contract census" the [full-parity plan](../superpowers/plans/2026-10-28-full-parity.md)
queued; it is the evidence for rows 178, 232 and 257 of
[matrix B](niri-gaps-config-input-ipc.md).

## What was counted

- niri's KDL `Action` enum has 186 variants.  **136** are bindable; 50 are `#[knuffel(skip)]`.
- `niri_ipc::Action` has **142** variants (each is also a `niri msg action <name>` subcommand).
- The census rows are the union: **143** = 142 typed variants + `suspend`, which only KDL has.
  The 50 skipped KDL variants are accounted for in [Appendix A](#appendix-a-the-50-skipped-kdl-variants).

Every row runs niri's *own* grammar for the action through wiri's *own* parsers, up to the effect
boundary (the executors need a desktop and are covered by the engine tests and live gates the
matrices cite):

| Entry point | niri grammar used | wiri path exercised |
| --- | --- | --- |
| KDL bind | the node of `binds.rs` (`quit skip-confirmation=true`, `move-window-to-workspace "web" focus=false`, ...) | `config::decode::decode_text` then `HotkeyBinding::parse_action` |
| CLI | the words niri's clap CLI accepts (`-s`, `--window-id 5 --focus false`, `2 --reference web`, ...) | `wiri::ipc::niri_cli_words`, then the `RunAction` front half of `IpcServer::process_message` (move-floating-window, the screenshot parsers, `split_action_target`, `system_actions::parse`, `parse_action_flagged`) |
| typed IPC | the serde JSON of `niri_ipc::Action` (`{"MoveWindowToWorkspace":{"window_id":5,"reference":{"Name":"web"},"focus":false}}`) | `ipc::wire::decode` of `{"Action": ...}`, then the same pipeline, or the typed `WorkspaceIpcAction` / `SizeIpcAction` / `MoveFloatingWindow` conversion |

The table below is **measured**: `ipc::action_census` (`src/ipc/action_census.rs`) holds one row per
action, asserts every niri-valid form is accepted (or, for the N/A actions, refused with an error)
and prints the table (`cargo test --release --lib print_census_markdown -- --ignored --nocapture`
on Tridaris, keep the lines that start with `|`).  Cells: `ok` all niri forms accepted, `n/a` all
refused by design, `ext` not a niri form but wiri takes it, `=` not a niri form and wiri refuses it
too (as niri does), `-` no such form.

## Totals

| | actions |
| --- | --- |
| census rows | **143** |
| **full** parity (every niri-valid form accepted with niri's fields and defaults on every entry point niri has, executor `done` in matrices A/B, no known deviation) | **119** |
| **partial** (the contract matches; an executor-evidence gap or a by-id deviation remains, listed below) | **15** |
| **missing** (applicable, no executor or no entry point) | **0** |
| **N/A on Windows** (compositor or Wayland specific, refused with an explicit error everywhere) | **9** |

By entry point, measured: KDL 128 accepted + 8 N/A refused (the decoder says `not
implemented by wiri yet; the bind is ignored`) + 3 not bindable in niri and refused like
niri + 4 extensions; CLI 133 accepted + 9 N/A + 1 extension; typed IPC
133 accepted + 9 N/A + 1 extension.  The vocabulary list `config::decode::NIRI_ACTIONS` is
asserted equal to niri's 136 bindable names, in niri's order, by the same test module.

The **partial** actions:

- six whose executor evidence is incomplete in the matrices: `spawn` (matrix B: percent variables and
  secure-desktop behaviour are not pinned requirements), `toggle-keyboard-shortcuts-inhibit` (manual
  inhibitor only, no per-client protocol), `center-column`, `move-floating-window` and
  `switch-preset-window-height` / `-back` (matrix A: physical mixed-DPI and lifecycle routes);
- nine whose `--id` / `id` / `window_id` form still **activates the named window first** and then acts,
  while niri's `*ById` variant never changes focus (niri:src/input/mod.rs:1110-1119, 1728-1736,
  2111-2117, 2213-2221): `consume-or-expel-window-left` / `-right`, `center-window`,
  `reset-window-height`, `switch-preset-window-width` / `-back`, `toggle-window-floating`,
  `move-window-to-floating`, `move-window-to-tiling` (plus the tiled forms of
  `switch-preset-window-height` / `-back`, already counted above).  These need focus-neutral engine
  routes (the `_for(window)` shape that `fullscreen-window`, `set-window-width/height`,
  `maximize-window-to-edges`, the workspace and monitor transfers already have); that is engine work
  with live verification and is the largest remaining action gap.

## What this lane fixed

Each fix has a pure unit test that cites the niri file and line; the census and the targeted tests
fail on the old parsers (RED run: `quit -s`, `--reference`, the surplus-word and index-range tests
and the strict census row check failed; the census row check named `quit`,
`move-workspace-to-index` and `move-workspace-to-monitor`).

| # | Mismatch found | niri evidence | Fix | Test |
| --- | --- | --- | --- | --- |
| 1 | `move-column-to-index N` was read **0-based** (matrix A claimed "niri's 0-based column index" while citing the line that subtracts one) | niri:src/layout/scrolling.rs:1702-1707 `index.saturating_sub(1).min(len - 1)`; binds.rs:198 | the engine subtracts one: `1` is the first place, `0` too, past the end is last; the h76 live gate moves with `1` and restores with `column + 1` | `layout::engine::lane_l::move_column_to_index_is_one_based_like_niri_and_clamps` |
| 2 | A workspace index was any `u32` (`focus-workspace 300`) and `-1` became a workspace *named* "-1" | niri-ipc/src/lib.rs:1791-1805 (`u8`, "workspace index must be between 0 and 255"), binds.rs:754-758 | `WorkspaceRef::from_word`: an `i32`-parsable word is an index that must fit `u8`, else an error; other text is a name | `ipc::action_census::index_arguments_have_niris_integer_types` |
| 3 | `focus-window-in-column` took any number (niri: `u8`); `focus-column`, `move-column-to-index`, `move-workspace-to-index` (`usize`) refused values past `u32::MAX` | binds.rs:163,175,198,248 | `u8` parse for the first, a clamping `usize` parse for the others | same test |
| 4 | `niri msg action move-workspace-to-index N --reference R` and `move-workspace-to-monitor[-dir] --reference R` were rejected (wiri knew only `--workspace`) | niri-ipc/src/lib.rs:579-586, 805-812 | `--reference` accepted, `--workspace` kept as an alias, both at once refused, `set-workspace-name` keeps niri's `--workspace` only | `niri_cli_spellings_of_workspace_references_are_accepted` |
| 5 | `niri msg action unset-workspace-name [REFERENCE]` (positional) was rejected | niri-ipc/src/lib.rs:611-618 | `wiri::ipc::niri_cli_words` turns the one positional into `--workspace` and keeps its quote bit; the bind form stays a unit action | same test, and `unset_workspace_name_takes_its_reference_positionally` (ctl) |
| 6 | `quit -s` (niri's short flag) was rejected | niri-ipc/src/lib.rs:196-201 `#[arg(short, long)]` | `-s` accepted | `quit_takes_niris_short_flag` |
| 7 | `niri msg action spawn -- tool --id 5`: the `--id 5` was **stripped as a window target** and wiri focused window 5 (or failed) instead of starting `tool --id 5` | niri-ipc/src/lib.rs:207-218 (`last = true`) | `split_action_target`: `spawn`, `exec`, `spawn-sh`, `spawn-cmd` words are never options; the `--` separator is not a word (clap) | `spawn_words_are_never_taken_for_a_window_target`, `spawn_takes_niris_double_dash_and_keeps_every_word` (ctl) |
| 8 | 33 unit actions (`close-window "x"`, `center-column "x"`, ...) silently ignored surplus words; niri's KDL refuses them | the unit variants of niri-config/src/binds.rs:105-366 | the canonical niri names refuse any word (legacy aliases keep their old arms) | `unit_actions_refuse_surplus_words_like_niris_kdl` |
| 9 | `niri msg -j`, `niri msg layers` and `niri msg casts` did not parse | niri:src/cli.rs:44-45, 74, 114 | `-j` short flag; `layers` and `casts` forward niri's own request name and print the daemon's "not available on Windows" error, exit status 1 | `niri_msg_short_json_flag_and_every_msg_subcommand_exist`, `layers_and_casts_travel_as_niris_own_request_names` (ctl) |
| 10 | `maximize-window-to-edges --id N` and `toggle-window-rule-opacity --id N` activated window N first | niri:src/input/mod.rs:1753-1761 (`MaximizeWindowToEdgesById` never focuses) | direct `_for(window)` calls in `IpcServer`, a floating target is left alone as for the focused one | `ipc::server::tests::maximize_and_rule_opacity_with_id_leave_the_focus_alone` |
| 11 | typed defaults and required fields were unmeasured | niri-ipc/src/lib.rs `#[serde(default...)]` | measured: omitted `focus`/`show_pointer`/`write_to_disk`/axes equal their explicit defaults; 20 required fields are refused when absent | `omitted_typed_fields_take_niris_serde_defaults`, `required_typed_fields_are_required` |

Spot check of every numeric action argument against the pinned engine code (so fix 1 is not an
isolated claim): `focus-column`, `focus-window-in-column` (`scrolling.rs:1615,4879`),
`move-workspace-to-index` (`input/mod.rs:1605-1618`, `saturating_sub(1)`) and `focus-workspace` /
`move-*-to-workspace` indices are 1-based in wiri as in niri; `switch-layout`'s index is 0-based
in both.

## Where wiri accepts more than niri (extensions, not mismatches)

- `--id` / `--window-id` on **any** `RunAction` (niri has a window id on 27 actions only: `--id` on 26, `--window-id` on `move-window-to-workspace`); on an action
  without a niri by-id variant it activates the window first.
- `focus-window`, `toggle/set/unset-window-urgent` and `load-config-file` are also accepted as binds
  (niri: `#[knuffel(skip)]`); the urgency actions work without an id (the focused window) in every form,
  where niri requires `id`.
- `suspend` has a CLI and a typed form (niri: KDL only).
- A bare string `"FocusColumnLeft"` is accepted as a typed action; `SwitchLayout` also takes a legacy
  `target` field; `SetColumnWidth` / `SetWindowWidth` / `SetWindowHeight` take an `id` alias `window_id`.
- KDL arguments niri reads as strings (`argument, str`: `set-window-width`, `switch-layout`) also
  accept bare numbers, and integer arguments (`focus-column`) also accept a quoted number.
- Legacy aliases and wiri-only actions (`focus-left`, `column-width-half`, `toggle-sticky`,
  `set-workspace-layout`, ...) keep their older lenient arms.
- `workspace=` as a property of the workspace actions and `--workspace` on `move-workspace-to-*`.

## Remaining gaps

1. The nine focus-first by-id actions above (engine work, needs live verification).
2. The six executor-evidence partials above.
3. `--print-request` prints wiri's adjacent-tagged request, not niri's externally tagged JSON, so a
   script that pipes it into a niri-style client sees a different line (matrix B row 232 stays
   partial for this).
4. `niri msg output`: `custom-mode` and `modeline` have no CLI words; `vrr` and `max-bpc` are forwarded
   and refused by the daemon (Windows owns them); `flipped*` transforms are refused.
5. niri's top-level `niri [-c FILE] [--session] -- COMMAND`: the startup command is missing (config
   `spawn-at-startup` covers the use), `--session` (systemd/D-Bus environment import) and `niri panic` are N/A.
6. Executor semantics beyond the matrices' own evidence were not re-audited; the live gates (h76, h79,
   H35-H39, H82) were not run by this lane.

## `niri msg` structure against `wiri-ctl`

niri:src/cli.rs:43-117 defines 17 `Msg` subcommands, `-j/--json` and `--print-request`.

| niri msg | wiri-ctl | status |
| --- | --- | --- |
| `outputs` | `outputs` | done (the reply's VRR and max-bpc fields are unknown, matrix B section 9) |
| `workspaces` | `workspaces` | done |
| `windows` | `windows` (`window-list` keeps wiri's older shape) | done |
| `layers` | `layers`: forwards `"Layers"`, daemon error "Layers is not available on Windows", exit 1 | N/A (new) |
| `keyboard-layouts` | `keyboard-layouts` | done |
| `focused-output` / `focused-window` | same names | done |
| `pick-window` / `pick-color` | same names (SDR only for colour) | done / partial |
| `action <name> [args]` | `action <name> [words]` and the `msg` prefix; the 143 actions above | partial (rows above) |
| `output <name> <action>` | `output <name> <words or external JSON>`: on, off, mode, scale, transform, position auto/set, vrr, max-bpc | partial (gap 4) |
| `event-stream` | `event-stream` (alias `events`) | done |
| `version` | `version` | done |
| `request-error` | `request-error` (alias `return-error`) | done |
| `overview-state` | `overview-state` | done |
| `casts` | `casts`: forwards `"Casts"`, "not available on Windows", exit 1 | N/A (new) |
| `raw-request` | `raw-request [JSON]` (argument or stdin) | done |
| `-j`, `--json` | `-j`, `--json` (global, before or after the subcommand) | done (short flag new) |
| `--print-request` | `--print-request` (global) | partial (gap 3) |

Other niri entry points: `niri validate` is `wiri validate -c FILE` (and `wiri-ctl validate-config`),
`niri completions <shell>` is `wiri completions <shell>` (no nushell).

## Census table

Status is `full`, `partial` or `N/A` as defined under Totals.

| # | niri action | niri typed fields (defaults) | KDL | CLI | typed IPC | status | wiri executor | notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | `quit` | skip_confirmation: bool = false (KDL skip-confirmation=, CLI -s/--skip-confirmation) | ok | ok | ok | full | System(Quit) -> system_actions::request_quit (confirmation unless skipped) |  |
| 2 | `suspend` | (no fields) | ok | ext | ext | full | System(Suspend) -> system_actions::request_power (SetSuspendState) | KDL-only in niri (binds.rs:109, no IPC variant); the CLI and typed forms are a wiri extension |
| 3 | `power-off-monitors` |  | ok | ok | ok | full | System(PowerOffMonitors) -> WM_SYSCOMMAND SC_MONITORPOWER 2 |  |
| 4 | `power-on-monitors` |  | ok | ok | ok | full | System(PowerOnMonitors) -> WM_SYSCOMMAND SC_MONITORPOWER -1 |  |
| 5 | `spawn` | command: Vec<String> (required) | ok | ok | ok | partial | Spawn -> message_loop::spawn_process (exact argv) | matrix B: percent-variable expansion and secure-desktop (locked) behaviour are not pinned requirements; `--id` inside the argv is the program's, not a window target |
| 6 | `spawn-sh` | command: String (required) | ok | ok | ok | full | SpawnCmd -> message_loop::spawn_shell_command (cmd.exe /C) | cmd.exe syntax stands in for sh |
| 7 | `do-screen-transition` | delay_ms: Option<u16> = None (KDL delay-ms=, CLI -d/--delay-ms) | n/a | n/a | n/a | N/A | none | niri's own screenshot-and-crossfade render effect; DWM composes the desktop and wiri animates with its own driver |
| 8 | `screenshot` | show_pointer: bool = true, path: Option<String> = None (absolute) | ok | ok | ok | full | ScreenshotUi(options) -> overlay::screenshot_ui::open |  |
| 9 | `screenshot-screen` | write_to_disk: bool = true, show_pointer: bool = true, path: Option<String> = None | ok | ok | ok | full | Capture(options) -> screenshot::action::CaptureAction::run |  |
| 10 | `screenshot-window` | id: Option<u64> = None, write_to_disk: bool = true, show_pointer: bool = FALSE, path: Option<String> = None | ok | ok | ok | full | Capture(options) -> screenshot::action::CaptureAction::run (explicit id, no focus change) |  |
| 11 | `toggle-keyboard-shortcuts-inhibit` |  | ok | ok | ok | partial | ToggleKeyboardShortcutsInhibit -> keyboard::ENGINE.toggle_inhibit | manual global inhibitor only; Windows has no per-client inhibit protocol (matrix B) |
| 12 | `close-window` | id: Option<u64> = None | ok | ok | ok | full | CloseWindow -> engine.close_target (by id: IpcMessage::CloseWindow, focus unchanged) |  |
| 13 | `fullscreen-window` | id: Option<u64> = None | ok | ok | ok | full | ToggleFullscreen -> engine.toggle_fullscreen_for |  |
| 14 | `toggle-windowed-fullscreen` | id: Option<u64> = None | n/a | n/a | n/a | N/A | none | Win32 has no "you are fullscreen" state to send to a foreign application (matrix A) |
| 15 | `focus-window` | id: u64 (required; no KDL form) | = | ok | ok | full | ipc::server::focus_window_by_hwnd |  |
| 16 | `focus-window-in-column` | index: u8 (1-based; 0 is the first) | ok | ok | ok | full | LayoutFocus(WindowInColumn) -> engine.focus_layout_action |  |
| 17 | `focus-window-previous` |  | ok | ok | ok | full | LayoutFocus(Previous) / FocusPrevious -> engine.focus_previous_window |  |
| 18 | `focus-column-left` |  | ok | ok | ok | full | FocusColumnLeft -> engine.focus_left |  |
| 19 | `focus-column-right` |  | ok | ok | ok | full | FocusColumnRight -> engine.focus_right |  |
| 20 | `focus-column-first` |  | ok | ok | ok | full | FocusColumnFirst -> engine.focus_column_first |  |
| 21 | `focus-column-last` |  | ok | ok | ok | full | FocusColumnLast -> engine.focus_column_last |  |
| 22 | `focus-column-right-or-first` |  | ok | ok | ok | full | LayoutFocus(ColumnRightOrFirst) -> engine.focus_layout_action |  |
| 23 | `focus-column-left-or-last` |  | ok | ok | ok | full | LayoutFocus(ColumnLeftOrLast) -> engine.focus_layout_action |  |
| 24 | `focus-column` | index: usize (1-based; 0 is the first) | ok | ok | ok | full | LayoutFocus(Column) -> engine.focus_layout_action |  |
| 25 | `focus-window-or-monitor-up` |  | ok | ok | ok | full | LayoutFocus(WindowOrMonitorUp) -> engine.focus_layout_action |  |
| 26 | `focus-window-or-monitor-down` |  | ok | ok | ok | full | LayoutFocus(WindowOrMonitorDown) -> engine.focus_layout_action |  |
| 27 | `focus-column-or-monitor-left` |  | ok | ok | ok | full | LayoutFocus(ColumnOrMonitorLeft) -> engine.focus_layout_action |  |
| 28 | `focus-column-or-monitor-right` |  | ok | ok | ok | full | LayoutFocus(ColumnOrMonitorRight) -> engine.focus_layout_action |  |
| 29 | `focus-window-down` |  | ok | ok | ok | full | FocusDown -> engine.focus_down |  |
| 30 | `focus-window-up` |  | ok | ok | ok | full | FocusUp -> engine.focus_up |  |
| 31 | `focus-window-down-or-column-left` |  | ok | ok | ok | full | LayoutFocus(DownOrColumnLeft) -> engine.focus_layout_action |  |
| 32 | `focus-window-down-or-column-right` |  | ok | ok | ok | full | LayoutFocus(DownOrColumnRight) -> engine.focus_layout_action |  |
| 33 | `focus-window-up-or-column-left` |  | ok | ok | ok | full | LayoutFocus(UpOrColumnLeft) -> engine.focus_layout_action |  |
| 34 | `focus-window-up-or-column-right` |  | ok | ok | ok | full | LayoutFocus(UpOrColumnRight) -> engine.focus_layout_action |  |
| 35 | `focus-window-or-workspace-down` |  | ok | ok | ok | full | LayoutFocus(WindowOrWorkspaceDown) -> engine.focus_layout_action |  |
| 36 | `focus-window-or-workspace-up` |  | ok | ok | ok | full | LayoutFocus(WindowOrWorkspaceUp) -> engine.focus_layout_action |  |
| 37 | `focus-window-top` |  | ok | ok | ok | full | LayoutFocus(WindowTop) -> engine.focus_layout_action |  |
| 38 | `focus-window-bottom` |  | ok | ok | ok | full | LayoutFocus(WindowBottom) -> engine.focus_layout_action |  |
| 39 | `focus-window-down-or-top` |  | ok | ok | ok | full | LayoutFocus(DownOrTop) -> engine.focus_layout_action |  |
| 40 | `focus-window-up-or-bottom` |  | ok | ok | ok | full | LayoutFocus(UpOrBottom) -> engine.focus_layout_action |  |
| 41 | `move-column-left` |  | ok | ok | ok | full | MoveColumnLeft -> engine.move_column (floating: float_move_by) |  |
| 42 | `move-column-right` |  | ok | ok | ok | full | MoveColumnRight -> engine.move_column (floating: float_move_by) |  |
| 43 | `move-column-to-first` |  | ok | ok | ok | full | MoveColumnToFirst -> engine.move_column_to_edge(false) |  |
| 44 | `move-column-to-last` |  | ok | ok | ok | full | MoveColumnToLast -> engine.move_column_to_edge(true) |  |
| 45 | `move-column-left-or-to-monitor-left` |  | ok | ok | ok | full | MoveColumnLeftOrToMonitorLeft -> engine.move_column_or_to_monitor |  |
| 46 | `move-column-right-or-to-monitor-right` |  | ok | ok | ok | full | MoveColumnRightOrToMonitorRight -> engine.move_column_or_to_monitor |  |
| 47 | `move-column-to-index` | index: usize (1-based like niri scrolling.rs:1707; 0 is the first) | ok | ok | ok | full | MoveColumnToIndex -> engine.move_column_to_index |  |
| 48 | `move-window-down` |  | ok | ok | ok | full | MoveWindowDown -> engine.move_window_in_column(1) (floating: float_move_by) |  |
| 49 | `move-window-up` |  | ok | ok | ok | full | MoveWindowUp -> engine.move_window_in_column(-1) (floating: float_move_by) |  |
| 50 | `move-window-down-or-to-workspace-down` |  | ok | ok | ok | full | MoveWindowDownOrToWorkspaceDown -> engine.move_window_or_to_workspace(true) |  |
| 51 | `move-window-up-or-to-workspace-up` |  | ok | ok | ok | full | MoveWindowUpOrToWorkspaceUp -> engine.move_window_or_to_workspace(false) |  |
| 52 | `consume-or-expel-window-left` | id: Option<u64> = None | ok | ok | ok | partial | ConsumeOrExpelWindowLeft -> engine.consume_or_expel_window_left | by id (`--id` / `id`): wiri activates the named window first, niri's ById variant never changes focus; needs a focus-neutral engine path |
| 53 | `consume-or-expel-window-right` | id: Option<u64> = None | ok | ok | ok | partial | ConsumeOrExpelWindowRight -> engine.consume_or_expel_window_right | by id (`--id` / `id`): wiri activates the named window first, niri's ById variant never changes focus; needs a focus-neutral engine path |
| 54 | `consume-window-into-column` |  | ok | ok | ok | full | ConsumeWindowIntoColumn -> engine.consume_window_into_column |  |
| 55 | `expel-window-from-column` |  | ok | ok | ok | full | ExpelWindowFromColumn -> engine.expel_window_from_column |  |
| 56 | `swap-window-right` |  | ok | ok | ok | full | SwapWindowRight -> engine.swap_window_toward(true) |  |
| 57 | `swap-window-left` |  | ok | ok | ok | full | SwapWindowLeft -> engine.swap_window_toward(false) |  |
| 58 | `toggle-column-tabbed-display` |  | ok | ok | ok | full | ColumnToggleTabbed -> engine.toggle_tabbed_for_focused_column |  |
| 59 | `set-column-display` | display: ColumnDisplay (Normal \| Tabbed) | ok | ok | ok | full | SetColumnDisplay -> message_loop::set_column_display |  |
| 60 | `center-column` |  | ok | ok | ok | partial | CenterColumn -> engine.center_focused_column (floating: float_center) | explicit-action evidence for variable-width edge columns and mixed outputs pending (matrix A) |
| 61 | `center-window` | id: Option<u64> = None | ok | ok | ok | partial | CenterWindow -> engine.center_focused_column / float_center | by id (`--id` / `id`): wiri activates the named window first, niri's ById variant never changes focus; needs a focus-neutral engine path |
| 62 | `center-visible-columns` |  | ok | ok | ok | full | CenterVisibleColumns -> engine.center_visible_columns |  |
| 63 | `focus-workspace-down` |  | ok | ok | ok | full | FocusWorkspaceNext -> engine.focus_workspace_relative(Next) |  |
| 64 | `focus-workspace-up` |  | ok | ok | ok | full | FocusWorkspacePrevious -> engine.focus_workspace_relative(Previous) |  |
| 65 | `focus-workspace` | reference: WorkspaceReferenceArg (Id \| Index u8 \| Name) | ok | ok | ok | full | FocusWorkspace / FocusWorkspaceNamed / WorkspaceAction(Focus id) -> engine.focus_workspace_* |  |
| 66 | `focus-workspace-previous` |  | ok | ok | ok | full | FocusWorkspaceLast -> engine.focus_workspace_last_visited |  |
| 67 | `move-window-to-workspace-down` | focus: bool = true (KDL focus=, CLI --focus <bool>) | ok | ok | ok | full | WorkspaceAction(MoveWindowRelative / MoveColumnRelative) -> engine.workspace_layout_action |  |
| 68 | `move-window-to-workspace-up` | focus: bool = true (KDL focus=, CLI --focus <bool>) | ok | ok | ok | full | WorkspaceAction(MoveWindowRelative / MoveColumnRelative) -> engine.workspace_layout_action |  |
| 69 | `move-column-to-workspace-down` | focus: bool = true (KDL focus=, CLI --focus <bool>) | ok | ok | ok | full | WorkspaceAction(MoveWindowRelative / MoveColumnRelative) -> engine.workspace_layout_action |  |
| 70 | `move-column-to-workspace-up` | focus: bool = true (KDL focus=, CLI --focus <bool>) | ok | ok | ok | full | WorkspaceAction(MoveWindowRelative / MoveColumnRelative) -> engine.workspace_layout_action |  |
| 71 | `move-window-to-workspace` | window_id: Option<u64> = None, reference: WorkspaceReferenceArg, focus: bool = true | ok | ok | ok | full | WorkspaceAction(MoveWindow) -> engine.workspace_layout_action (explicit id: no focus change) |  |
| 72 | `move-column-to-workspace` | reference: WorkspaceReferenceArg, focus: bool = true | ok | ok | ok | full | WorkspaceAction(MoveColumn) -> engine.workspace_layout_action |  |
| 73 | `move-workspace-down` |  | ok | ok | ok | full | MoveWorkspaceDown -> engine.move_active_workspace(1) |  |
| 74 | `move-workspace-up` |  | ok | ok | ok | full | MoveWorkspaceUp -> engine.move_active_workspace(-1) |  |
| 75 | `move-workspace-to-index` | index: usize (1-based), reference: Option<WorkspaceReferenceArg> = None (CLI --reference) | ok | ok | ok | full | WorkspaceAction(MoveToIndex) -> engine.workspace_layout_action |  |
| 76 | `set-workspace-name` | name: String, workspace: Option<WorkspaceReferenceArg> = None (CLI --workspace) | ok | ok | ok | full | RenameWorkspace / WorkspaceAction(SetName) -> engine.workspace_layout_action |  |
| 77 | `unset-workspace-name` | reference: Option<WorkspaceReferenceArg> = None (CLI positional) | ok | ok | ok | full | WorkspaceAction(UnsetName) -> engine.workspace_layout_action |  |
| 78 | `focus-monitor-left` |  | ok | ok | ok | full | MonitorAction(Focus) -> engine.monitor_layout_action |  |
| 79 | `focus-monitor-right` |  | ok | ok | ok | full | MonitorAction(Focus) -> engine.monitor_layout_action |  |
| 80 | `focus-monitor-down` |  | ok | ok | ok | full | MonitorAction(Focus) -> engine.monitor_layout_action |  |
| 81 | `focus-monitor-up` |  | ok | ok | ok | full | MonitorAction(Focus) -> engine.monitor_layout_action |  |
| 82 | `focus-monitor-previous` |  | ok | ok | ok | full | MonitorAction(Focus) -> engine.monitor_layout_action |  |
| 83 | `focus-monitor-next` |  | ok | ok | ok | full | MonitorAction(Focus) -> engine.monitor_layout_action |  |
| 84 | `focus-monitor` | output: String | ok | ok | ok | full | MonitorAction(Focus(Name)) -> engine.monitor_layout_action |  |
| 85 | `move-window-to-monitor-left` |  | ok | ok | ok | full | MonitorAction(MoveWindow) -> engine.monitor_layout_action |  |
| 86 | `move-window-to-monitor-right` |  | ok | ok | ok | full | MonitorAction(MoveWindow) -> engine.monitor_layout_action |  |
| 87 | `move-window-to-monitor-down` |  | ok | ok | ok | full | MonitorAction(MoveWindow) -> engine.monitor_layout_action |  |
| 88 | `move-window-to-monitor-up` |  | ok | ok | ok | full | MonitorAction(MoveWindow) -> engine.monitor_layout_action |  |
| 89 | `move-window-to-monitor-previous` |  | ok | ok | ok | full | MonitorAction(MoveWindow) -> engine.monitor_layout_action |  |
| 90 | `move-window-to-monitor-next` |  | ok | ok | ok | full | MonitorAction(MoveWindow) -> engine.monitor_layout_action |  |
| 91 | `move-window-to-monitor` | id: Option<u64> = None, output: String | ok | ok | ok | full | MonitorAction(MoveWindow(Name)) -> engine.monitor_layout_action (explicit id: no focus change) |  |
| 92 | `move-column-to-monitor-left` |  | ok | ok | ok | full | MonitorAction(MoveColumn) -> engine.monitor_layout_action |  |
| 93 | `move-column-to-monitor-right` |  | ok | ok | ok | full | MonitorAction(MoveColumn) -> engine.monitor_layout_action |  |
| 94 | `move-column-to-monitor-down` |  | ok | ok | ok | full | MonitorAction(MoveColumn) -> engine.monitor_layout_action |  |
| 95 | `move-column-to-monitor-up` |  | ok | ok | ok | full | MonitorAction(MoveColumn) -> engine.monitor_layout_action |  |
| 96 | `move-column-to-monitor-previous` |  | ok | ok | ok | full | MonitorAction(MoveColumn) -> engine.monitor_layout_action |  |
| 97 | `move-column-to-monitor-next` |  | ok | ok | ok | full | MonitorAction(MoveColumn) -> engine.monitor_layout_action |  |
| 98 | `move-column-to-monitor` | output: String | ok | ok | ok | full | MonitorAction(MoveColumn(Name)) -> engine.monitor_layout_action |  |
| 99 | `set-window-width` | id: Option<u64> = None, change: SizeChange (SetFixed i32 \| SetProportion f64 \| AdjustFixed i32 \| AdjustProportion f64) | ok | ok | ok | full | SizeAction / SetColumnWidth -> change_size(Width) (explicit id: set_window_width_for) |  |
| 100 | `set-window-height` | id: Option<u64> = None, change: SizeChange (SetFixed i32 \| SetProportion f64 \| AdjustFixed i32 \| AdjustProportion f64) | ok | ok | ok | full | SizeAction / SetWindowHeight -> change_size(Height) (explicit id: set_window_height_for) |  |
| 101 | `reset-window-height` | id: Option<u64> = None | ok | ok | ok | partial | ResetWindowHeight -> engine.reset_window_height | by id (`--id` / `id`): wiri activates the named window first, niri's ById variant never changes focus; needs a focus-neutral engine path |
| 102 | `switch-preset-column-width` |  | ok | ok | ok | full | ColumnWidthPresetCycle -> engine.switch_preset_column_width(true) |  |
| 103 | `switch-preset-column-width-back` |  | ok | ok | ok | full | ColumnWidthPresetCycleBack -> engine.switch_preset_column_width(false) |  |
| 104 | `switch-preset-window-width` | id: Option<u64> = None | ok | ok | ok | partial | WindowWidthPresetCycle -> engine.switch_preset_window_width(true) | by id (`--id` / `id`): wiri activates the named window first, niri's ById variant never changes focus; needs a focus-neutral engine path |
| 105 | `switch-preset-window-width-back` | id: Option<u64> = None | ok | ok | ok | partial | WindowWidthPresetCycleBack -> engine.switch_preset_window_width(false) | by id (`--id` / `id`): wiri activates the named window first, niri's ById variant never changes focus; needs a focus-neutral engine path |
| 106 | `switch-preset-window-height` | id: Option<u64> = None | ok | ok | ok | partial | WindowHeightPresetCycle -> engine.switch_preset_window_height(true) | native mixed-DPI and some lifecycle routes still need physical evidence (matrix A); a tiled target by id is activated first (floating targets are not) |
| 107 | `switch-preset-window-height-back` | id: Option<u64> = None | ok | ok | ok | partial | WindowHeightPresetCycleBack -> engine.switch_preset_window_height(false) | same row as switch-preset-window-height (matrix A); a tiled target by id is activated first |
| 108 | `maximize-column` |  | ok | ok | ok | full | MaximizeColumn -> engine.toggle_maximize_focused_column |  |
| 109 | `maximize-window-to-edges` | id: Option<u64> = None | ok | ok | ok | full | MaximizeWindowToEdges -> engine.toggle_maximize_window_to_edges_for |  |
| 110 | `set-column-width` | change: SizeChange | ok | ok | ok | full | SetColumnWidth -> change_size(Width) |  |
| 111 | `expand-column-to-available-width` |  | ok | ok | ok | full | ExpandColumnToAvailable -> engine.expand_column_to_available |  |
| 112 | `switch-layout` | layout: LayoutSwitchTarget (Next \| Prev \| Index u8) | ok | ok | ok | full | System(SwitchLayout) -> ipc::keyboard::switch_layout (WM_INPUTLANGCHANGEREQUEST) |  |
| 113 | `show-hotkey-overlay` |  | ok | ok | ok | full | ShowKeyBindings -> message_loop::show_hotkey_overlay |  |
| 114 | `move-workspace-to-monitor-left` |  | ok | ok | ok | full | MonitorAction(MoveWorkspace) -> engine.monitor_layout_action |  |
| 115 | `move-workspace-to-monitor-right` |  | ok | ok | ok | full | MonitorAction(MoveWorkspace) -> engine.monitor_layout_action |  |
| 116 | `move-workspace-to-monitor-down` |  | ok | ok | ok | full | MonitorAction(MoveWorkspace) -> engine.monitor_layout_action |  |
| 117 | `move-workspace-to-monitor-up` |  | ok | ok | ok | full | MonitorAction(MoveWorkspace) -> engine.monitor_layout_action |  |
| 118 | `move-workspace-to-monitor-previous` |  | ok | ok | ok | full | MonitorAction(MoveWorkspace) -> engine.monitor_layout_action |  |
| 119 | `move-workspace-to-monitor-next` |  | ok | ok | ok | full | MonitorAction(MoveWorkspace) -> engine.monitor_layout_action |  |
| 120 | `move-workspace-to-monitor` | output: String, reference: Option<WorkspaceReferenceArg> = None (CLI --reference) | ok | ok | ok | full | MonitorAction(MoveWorkspace(Name)) -> engine.monitor_layout_action |  |
| 121 | `toggle-debug-tint` |  | n/a | n/a | n/a | N/A | none | compositor render debugging; there is no wiri renderer to tint |
| 122 | `debug-toggle-opaque-regions` |  | n/a | n/a | n/a | N/A | none | compositor render debugging (opaque-region overlay) |
| 123 | `debug-toggle-damage` |  | n/a | n/a | n/a | N/A | none | compositor render debugging (damage overlay) |
| 124 | `toggle-touchpad` |  | ok | ok | ok | full | System(ToggleTouchpad) -> system_actions::toggle_touchpad |  |
| 125 | `toggle-window-floating` | id: Option<u64> = None | ok | ok | ok | partial | ToggleFloating -> engine.toggle_floating_for | by id (`--id` / `id`): wiri activates the named window first, niri's ById variant never changes focus; needs a focus-neutral engine path |
| 126 | `move-window-to-floating` | id: Option<u64> = None | ok | ok | ok | partial | MoveWindowToFloating -> engine.set_window_floating(true) | by id (`--id` / `id`): wiri activates the named window first, niri's ById variant never changes focus; needs a focus-neutral engine path |
| 127 | `move-window-to-tiling` | id: Option<u64> = None | ok | ok | ok | partial | MoveWindowToTiling -> engine.set_window_floating(false) | by id (`--id` / `id`): wiri activates the named window first, niri's ById variant never changes focus; needs a focus-neutral engine path |
| 128 | `focus-floating` |  | ok | ok | ok | full | FocusFloating -> engine.focus_layer_action(Floating) |  |
| 129 | `focus-tiling` |  | ok | ok | ok | full | FocusTiling -> engine.focus_layer_action(Tiling) |  |
| 130 | `switch-focus-between-floating-and-tiling` |  | ok | ok | ok | full | SwitchFocusBetweenFloatingAndTiling -> engine.focus_layer_action(Switch) |  |
| 131 | `move-floating-window` | id: Option<u64> = None, x: PositionChange = AdjustFixed(0), y: PositionChange = AdjustFixed(0) (CLI -x/--x, -y/--y) | = | ok | ok | partial | IpcMessage::MoveFloatingWindow -> engine.move_floating_window | physical mixed-DPI evidence pending (matrix A) |
| 132 | `toggle-window-rule-opacity` | id: Option<u64> = None | ok | ok | ok | full | ToggleWindowRuleOpacity -> engine.toggle_window_rule_opacity |  |
| 133 | `set-dynamic-cast-window` | id: Option<u64> = None | n/a | n/a | n/a | N/A | none | no PipeWire/portal cast targets on Windows; capture applications choose their own sources |
| 134 | `set-dynamic-cast-monitor` | output: Option<String> = None | n/a | n/a | n/a | N/A | none | no PipeWire/portal cast targets on Windows |
| 135 | `clear-dynamic-cast-target` |  | n/a | n/a | n/a | N/A | none | no PipeWire/portal cast targets on Windows |
| 136 | `stop-cast` | session_id: u64 (required; no KDL form) | = | n/a | n/a | N/A | none | another application's capture session cannot be stopped from outside |
| 137 | `toggle-overview` |  | ok | ok | ok | full | OverviewToggle -> engine.toggle_overview |  |
| 138 | `open-overview` |  | ok | ok | ok | full | OverviewOpen -> engine.enter_overview |  |
| 139 | `close-overview` |  | ok | ok | ok | full | OverviewExit -> engine.exit_overview |  |
| 140 | `toggle-window-urgent` | id: u64 (required in niri; no KDL form) | ext | ok | ok | full | System(WindowUrgent) -> system_actions::execute (engine.set_window_urgent + FlashWindowEx) | extension: wiri also accepts the action without an id (the focused window) and as a bind |
| 141 | `set-window-urgent` | id: u64 (required in niri; no KDL form) | ext | ok | ok | full | System(WindowUrgent) -> system_actions::execute (engine.set_window_urgent + FlashWindowEx) | extension: wiri also accepts the action without an id (the focused window) and as a bind |
| 142 | `unset-window-urgent` | id: u64 (required in niri; no KDL form) | ext | ok | ok | full | System(WindowUrgent) -> system_actions::execute (engine.set_window_urgent + FlashWindowEx) | extension: wiri also accepts the action without an id (the focused window) and as a bind |
| 143 | `load-config-file` | path: Option<String> = None (CLI --path) | ext | ok | ok | full | System(LoadConfigFile) -> system_actions::request_reload | extension: wiri also accepts it as a bind |

## Appendix A: the 50 skipped KDL variants

`niri-config` marks 50 `Action` variants `#[knuffel(skip)]`: no name in a `binds {}` block.

**27 `*ById` / `*ByRef` forms** (the typed `id`, `window_id` or `reference` field of the census rows):
`ScreenshotWindowById`, `CloseWindowById`, `FullscreenWindowById`, `ToggleWindowedFullscreenById` (N/A),
`ConsumeOrExpelWindowLeftById`, `ConsumeOrExpelWindowRightById`, `CenterWindowById`,
`MoveWindowToWorkspaceById`, `MoveWorkspaceToIndexByRef`, `MoveWorkspaceToMonitorByRef`,
`SetWorkspaceNameByRef`, `UnsetWorkSpaceNameByRef`, `MoveWindowToMonitorById`, `SetWindowWidthById`,
`SetWindowHeightById`, `ResetWindowHeightById`, `SwitchPresetWindowWidthById`,
`SwitchPresetWindowWidthBackById`, `SwitchPresetWindowHeightById`, `SwitchPresetWindowHeightBackById`,
`MaximizeWindowToEdgesById`, `ToggleWindowFloatingById`, `MoveWindowToFloatingById`,
`MoveWindowToTilingById`, `MoveFloatingWindowById`, `ToggleWindowRuleOpacityById`,
`SetDynamicCastWindowById` (N/A).  Focus-neutral in wiri: screenshot-window, close-window,
fullscreen-window, the workspace/monitor/reference forms, set-window-width/height, maximize-window-to-edges,
toggle-window-rule-opacity, move-floating-window.  Focus-first: the nine listed under Totals.

**6 typed-IPC-only actions**: `FocusWindow`, `StopCast` (N/A), `ToggleWindowUrgent`, `SetWindowUrgent`,
`UnsetWindowUrgent`, `LoadConfigFile` (rows `focus-window`, `stop-cast`, `toggle-window-urgent`,
`set-window-urgent`, `unset-window-urgent`, `load-config-file`).

**17 internal variants** with no name on any user-facing surface: `ChangeVt` (N/A: no virtual terminals),
`ConfirmScreenshot`, `CancelScreenshot`, `ScreenshotTogglePointer` (keys of niri's own screenshot UI; wiri's
screenshot UI handles its keys itself, matrix B section 7), `FocusColumnLeftUnderMouse`,
`FocusColumnRightUnderMouse`, `FocusWorkspaceDownUnderMouse`, `FocusWorkspaceUpUnderMouse` (produced by niri's
scroll dispatch; wheel binds are matrix B row 186), `MruAdvance`, `MruConfirm`, `MruCancel`,
`MruCloseCurrentWindow`, `MruFirst`, `MruLast`, `MruSetScope`, `MruCycleScope` (the recent-windows binds table,
matrix B section 14) and `TestAction` (niri's own unit tests).

## Verification

Tridaris, `agent-check` on `wip/sonnet-acts` at `{head}`: rustfmt clean, `cargo check --release
--all-targets` with `-D warnings` clean, `cargo test --release`: {tests}.  The census was also run against
the pre-fix parsers (RED, five failures) before the fixes were kept.
