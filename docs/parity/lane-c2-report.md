# Lane C2 â€” config, bindings, keyboard and session configuration

Branch: `meylin/sa-4ac26-6b96b`, based on integrated `c2bb1ec`.
Upstream reference: niri `e19df12cf8` (configuration wiki and niri-config).

## Item 1: config system

Explicit CLI, nonempty WIRI_CONFIG, nonempty NIRI_CONFIG, then APPDATA is the
Windows search policy. Missing explicit paths are diagnosed and never created.
Implicit creation uses create_new and queues a tray balloon. Failed-load balloons
can be suppressed, without suppressing diagnostics or ConfigLoaded events.

Each physical config part validates singleton sections independently; included
parts retain positional merging. Diagnostics retain source snapshots, both
locations for duplicates, help and the include chain. Numeric integers do not
round silently; out-of-range opacity is an error. User binds no longer inherit
built-in defaults unless extend-defaults=true is explicit. Offline validation
and completion generation never start the daemon.

Pure tests cover search priority, missing explicit paths, duplicate/range/integer
validation, output/rule repeatability, notification decoding, empty bindings and
CLI parsing. H66 records executable-level offline validation, environment search,
source/include diagnostics, noncreation and PowerShell completion output.

## Item 2: binding identities and replacement

`input/keynames.rs` is the shared case-insensitive Windows VK/XKB/legacy alias
adapter. Chord identity uses the final merged Mod mapping (even when input comes
after binds), not textual modifier order or spelling. A duplicate within one
physical part is fatal and labels the first definition. Included matching chords
override; unrelated chords remain. Unknown/level-5 modifiers never become bare
keys. Empty/omitted tables remain empty at runtime; only explicit extend-defaults
or the separate invalid-startup emergency fallback requests built-ins.

AltGr is a hook-only Ctrl+right-Alt identity, distinct from Ctrl+left-Alt in both
keyboard and pointer hooks. It is never passed to RegisterHotKey. This is a Windows
approximation, not arbitrary XKB level selection. Shifted punctuation names share
the underlying US OEM VK without implicit Shift, keypad Enter collapses to Return,
and layout-dependent Unicode/dead/Linux-only hardware keysyms are not guessed.
Pure tests cover the complete alias table, canonical collisions, final Mod,
include override, pressed-state transitions and fallback exclusion. H67 covers
executable validation and actual non-mutating application/chord behavior.

Legacy live keymap scenarios now explicitly include a physical config part with
`binds { extend-defaults; }`; they no longer depend on implicit runtime defaults.
H1 validates unknown/level-5 modifiers offline (fatal errors), retains its F8/F9
application and Mod mapping assertions, and leaves supported Mod5 to H67.

## Item 3: explicitly configured keyboard options

One KeyboardConfigSession owns explicit options across startup/reload/shutdown.
Unset options are not queried or written. Removal restores the captured baseline;
an external change relinquishes ownership. Native repeat steps/rates approximate
niri (rate zero cannot disable native repeat), and one already-loaded layout is
requested only for a guarded foreground thread. Native NumLock enforcement is
refused with a diagnostic: GetKeyState is queue-relative, so the background
daemon cannot safely capture or compare the real toggle baseline. Its dry-run
uses a hypothetical baseline and shadow only; no injection is attempted.
Layout installation, arbitrary XKB compilation and guaranteed client acceptance
are not claimed. Native setters/restoration are synthetic-tested only here.
`WIRI_KEYBOARD_DRY_RUN=1` is sampled once for the whole lifetime and used by the
entire harness; H68 records planned apply/restore behavior without native writes.

## Item 4: child environment, startup and switches

Environment values are strings or null, with Windows ordinal case-insensitive
identity and duplicate source labels. Each launch snapshots inherited UTF-16,
per-call additions and configured overrides; lookup and process creation use the
same snapshot. Null removes, empty sets an empty value, and the daemon's process
environment never changes. Argv, shell, startup and IPC share the spawner. UAC
broker launches refuse configured overrides rather than silently bypassing them.
H69 records child snapshots for direct/shell spawn, good/bad reload, both startup
forms, no-autostart and no rerun on reload, through private PID-verified pipes.

Lid notifications and slate posture changes reuse the existing hidden-window pump.
No initial or repeated sample is an edge. Configured subscriptions allocate on
first use and are removed on reload/exit; actions use the current environment and
bypass keyboard inhibition. Reducer/payload/reload behavior is unit-tested; real
lid/posture transitions and hardware-specific delivery are not live-verified.

## Item 5: named workspace layout and output preferences

Named workspace layout partials resolve global -> matching output -> workspace,
including inactive workspaces and window-targeted rules, presets, sizing,
decorations, pointer geometry and overview previews. Removing a partial on reload
returns to the output/global base. Output-only empty-workspace/insert-hint options
are rejected inside a workspace layout. Workspace names remain case-sensitive.

Repeated `open-on-output` declarations retain their order. Reload, monitor
registration/removal and workspace-name events reevaluate the first connected
preference; an unavailable preference retains a connected owner before choosing a
fallback. Whole columns, floats and minimized homes migrate without re-adding
windows, preserving widths, tab state, scroll and focus. Nine synthetic engine
regressions cover precedence/removal, inactive workspace rule/admission widths,
output reload, fallback/hotplug priority, state-preserving migration (including
empty-strip metadata with floating/minimized homes) and name swaps. Overview
geometry has an additional workspace-strut regression.
H70 measures workspace -> output -> reloaded-output column widths on the single
physical display. Multi-output priority/hotplug is unit-tested only.

## Validation

Warning-denied `cargo build --release -j 2`, `cargo test --release -j 2` and
`cargo check --release --all-targets -j 2` passed after integration: 1334 library,
1 main and 14 control-CLI tests; one ignored documentation example. Local logs:
`.scratch/c2-final-{build,tests,check}.log`.

First complete locked run: **535/561 gates passed, 26 failed**. Its summary and verdict are preserved in `.scratch/c2-first-{summary,verdict}.json`. Failures: settle_positioning_per_s, h1_f9_f8_reach_application, h9_default_hotkeys_registered, h47_bind_properties_act, h49_load_config_file_and_reload_current, h49_bad_config_keeps_last_good, h67_omitted_binds_no_defaults, h67_empty_binds_no_defaults, h67_nonempty_binds_replace_defaults, h67_include_overrides_canonical_aliases_final_mod, h67_duplicate_aliases_both_locations, h67_unknown_modifier_rejects_retains_last_good, h67_altgr_aliases_validation_only, h68_explicit_keyboard_reload_dry_run, h68_keyboard_removal_dry_run, h68_explicit_keyboard_startup_dry_run, h69_environment_spawn_argv, h69_environment_spawn_sh, h69_invalid_reload_keeps_environment, h69_reload_replaces_environment, h69_parent_environment_unchanged, h69_startup_argv_and_shell_environment, h69_no_autostart_suppresses_both, h70_workspace_layout_overrides_output, h70_workspace_override_removal_uses_output, h70_output_reload_rebuilds_workspace_layout. Fixture corrections followed (unit-variant IPC requests, exact log prefixes, literal KDL paths, explicit legacy defaults and singleton-compatible Lane S fixtures). A final-binary rerun was interrupted by the 1800-second command limit in H1 and produced no summary; it is NOT a passing run. Only retained task-binary processes and their verified harness parent were stopped; cleanup is recorded in `.scratch/c2-final-cleanup.json`. Evidence is
under `.live/lane-c2`, `.live/lane-c2-binds`, `.live/lane-c2-keyboard`,
`.live/lane-c2-session` and `.live/lane-c2-workspaces`.
H66 was live-verified (9/9); H67-H70 remain unverified after fixture corrections. No full-suite pass is claimed. The initial perf sample was 0.0521% of one core, 189.68 context switches/s, 0 idle positioning lines and 0 collector errors; the separate settling gate failed at 0.4 positioning/s. No idle-cost regression sign-off is claimed.

## Cost and limits

No idle timer, polling loop or always-on thread is added. Config validation and
completion allocations occur only on load or command invocation. Balloon display
is not visually asserted. Windows search deliberately has no machine-wide config
fallback. Keyboard/system settings are tested synthetically and dry-run only on
the shared laptop; output-priority behavior on multiple displays is synthetic.

## Integration on the merged head (lanes W, K, F2, T2, S, FK)

Ported onto `wiri/shine-20261024` (e7ced50). Workspace identity stays Lane F's: stable
local keys, ordered stacks, names stored on the workspace, `open-on-output` creating a
declared workspace once on the first available preferred output. C2 adds to that without
replacing it:

- A `workspace` layout block is looked up by the workspace's **name** (and the output it is
  on), so it follows renames, reordering and moves between outputs; there is no per-key or
  per-position table to go stale. Overrides of a name nobody carries are inert.
- Output preferences (repeated `open-on-output`, then the legacy `monitor` as the last
  entry, the same list that decides first placement) are reevaluated on output
  registration/removal, output rename/identity and workspace-name events, not on a config
  reload: Lane F requires that a reload never re-places a realized declaration (its
  `configured_output_preferences_and_reload_top_insertion...` test), so a reload only
  rebuilds the layout views and the next output event applies the new list. A live named
  workspace moves as one object to the first connected output of its list, through the
  same path as `move-workspace-to-monitor` (identity, columns, floats, minimized homes,
  scroll and focus state kept; pending hotplug returns are cancelled). With no connected
  preference it stays where it is; a declaration whose preferred outputs are all absent is
  still created lazily by Lane F, not on a fallback output.
- `binds {}` replacement is C2's: the generated first-run config and `examples/minimal.kdl`
  now carry the explicit `extend-defaults` line, so a first run keeps the whole built-in
  keymap (53 written chords plus the 17 only the built-in table has: previous-window,
  resize mode, width preset cycle, sticky, cheatsheet, workspace previous/next, ...).

Classification of the 26 gates that failed in the first locked run (evidence:
`.scratch/c2-first-summary.json`, the unit-variant request fix script `.scratch/c2-live-fixes.py`,
the generated-config start in the rerun's `wiri.stderr.log`):

| Gates | Cause | Class |
| --- | --- | --- |
| settle_positioning_per_s | 0.4 repositions/s with 40 managed windows (38 strays); the clean-desktop rerun settles at 0 and the idle sample was 0 | environment |
| h1_f9_f8_reach_application | the chords were typed without making the owned form foreground; the corrected fixture called `FocusForm` before the script defines it (now uses the early helpers) | harness |
| h9_default_hotkeys_registered | the shipped config registered 52 of its 53 chords and no built-ins once binds stopped inheriting defaults | product (fixed: shipped config opts in; H9 again starts with no config) |
| h47_bind_properties_act, h49_load_config_file_and_reload_current, h49_bad_config_keeps_last_good | Lane S fixtures appended a second `binds {}` section; duplicate singleton sections are now errors, so the reload failed and F9/F10 never bound | harness (corrected earlier; unit regressions pin the duplicate error) |
| all seven h67_* | request `{type,data:{}}` for unit variants was rejected before the first probe | harness |
| h68_explicit_keyboard_reload_dry_run, h68_keyboard_removal_dry_run, h68_explicit_keyboard_startup_dry_run | the log snapshot prefix carried a synthetic newline, so the later snapshot never started with the earlier one | harness |
| all seven failing h69_* | unit-variant requests and escaped KDL paths in the fixture | harness |
| the three h70_* | unit-variant requests | harness |
