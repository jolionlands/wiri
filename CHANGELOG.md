# Changelog

All notable changes to wiri are documented in this file. The format loosely
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Lane GRID — the unified XY grid (wiri extension, opt-in)

- `layout { unified-grid }` (also `unified-grid true` / `unified-grid false`, and accepted in a
  per-output `layout {}` override) makes the desktop **one 2D grid**: x is each workspace's
  endless column strip, y is the monitor's workspace stack, and every row of a monitor shares
  **one** horizontal camera instead of niri's one camera per row. Off by default; with the flag
  absent or false every code path in the lane is bypassed and behaviour is bit-identical to the
  niri-parity behaviour the matrices measure. `layout { unified-grid-chord-ms 150 }` sets the
  arrow-chord window (integer `0..=1000`; `0` disables merging).
- The camera keeps living in the active workspace's `scroll_offset.x`, so `calculate_positions`,
  `is_over_gap`, the gestures, the animation driver and the IPC geometry are untouched. What
  changes is the row change: `unified_switch_workspace` carries the offset across and lands on
  the column under the camera's x anchor. Horizontal focus and column moves inside a row keep
  today's reveal/centre rules; the workspace slide animates only vertically.
- Rule 3 (x-matched vertical moves) applies to `focus-workspace-up`/`-down`,
  `focus-window-or-workspace-up`/`-down` when they cross rows, `move-window-to-workspace-up`/`-down`
  and `move-column-to-workspace-up`/`-down`, and to named/indexed workspace moves: the destination
  column is the one whose strip x-range contains the focus's horizontal centre; an anchor in a gap,
  before the first column or past the last one takes the nearest column by distance (ties to the
  earlier one), an empty row just becomes active, and a moved window or column is inserted at the
  index the anchor falls into, keeping its width.
- Eight new actions, usable from `binds {}`, `wiri-ctl action <name>` and typed IPC:
  `focus-up-left`, `focus-up-right`, `focus-down-left`, `focus-down-right`,
  `move-window-up-left`, `move-window-up-right`, `move-window-down-left`,
  `move-window-down-right`. They resolve the row and the column before anything moves, so the
  row change and the horizontal reveal are one camera motion. With the mode off they fall back to
  the sequential vertical-then-horizontal niri actions.
- Rule 5 (two arrows together): the keyboard engine remembers the chordable vertical arrow with
  its virtual key and the input event's own timestamp (`KBDLLHOOKSTRUCT::time`). When the
  perpendicular arrow of the same family fires while that key is still held and within
  `unified-grid-chord-ms`, the pair becomes the diagonal action and the engine continues from the
  pre-vertical snapshot, retargeting the in-flight camera animation. **A single arrow press never
  waits** — it is dispatched by the same `event` call that observed the key-down, and a test pins
  that.
- Rule 6 (diagonal pan): `pan-up-left`, `pan-up-right`, `pan-down-left`, `pan-down-right` scroll
  the shared camera one `scroll-step` along x and move one row along y in one pass.
- IPC: `monitors` / `outputs` now report `unified_grid` per output. Diagonal moves go through the
  ordinary focus and workspace paths, so the usual focus/workspace events are emitted with no new
  event type.
- Tests: 15 engine cases in `src/layout/engine/unified_grid.rs` (3 rows x 4-6 columns of varied
  widths: x-match, gaps, row ends, empty row, insertion of moved windows/columns, diagonal focus
  and move, chord continuation, direct diagonal, diagonal pan, 100% mode-off identity) and 7
  keyboard cases (single-arrow dispatch, merge, released key, out-of-window, other family, move
  family, window `0`).
- Live gates: `scripts/live-phases/lane-grid.ps1` (`h98_*`), written and parse-checked.
- No timer, thread, polling loop or idle wake-up is added; with the mode off the only added work is
  one `bool` read on the workspace-switch and vertical-action paths.

### Lane PERF — one session UI thread, a lazy overview banner and a shorter blocking keep-alive
- **Every thread has a description.** The hotkey pump (now `wiri-ui`), the tray, the status bar, the snap guide and the overview banner were created with `std::thread::spawn` and inherited the process name. All of them now go through `std::thread::Builder::name`, which is `SetThreadDescription` on Windows, so the new information-only `p_rest_threads_by_*` keys in `scripts/live-phases/lane-e-perf.ps1` can attribute each thread by name at rest. [docs/PERF.md](docs/PERF.md#thread-and-memory-budget-work-2026-10-31-lane-perf) lists every thread, who owns it, whether it exists at rest and when a lazy one starts.
- **The tray icon moved onto the session UI thread.** `hooks::tray::create_icon` now runs on the same pump as the hotkey window, the low-level keyboard hook and the compositor/decoration windows: `TrayIcon::setup` installs a request, posts `WM_TRAY_HOST` to that thread and waits (5 s bound) for the answer, and only falls back to the old dedicated `wiri-tray` thread when the pump is not running. `Shell_NotifyIconW(NIM_DELETE)` still runs on the thread that added the icon (`hooks::tray::shutdown`, called when the pump stops). Behaviour is unchanged except that an open tray context menu now delays hotkey delivery while it is up, as any Win32 application's modal menu does.
- **The overview banner is lazy.** It used to start a message-pump thread and create its layered window at startup although only `overview-toggle` shows it. `OverviewBanner::ensure_thread` starts it on the first `show`/`hide` and posts one `WM_BANNER_UPDATE` when the window exists, so a `show` that raced the thread is not lost. No timer, no poll, no wake-up at rest.
- **Blocking-pool threads exit sooner.** `runtime::BLOCKING_KEEP_ALIVE` is 250 ms instead of 2 s, so the threads `spawn_blocking` creates for IPC requests, event-stream snapshots and the animation clock wait are gone right after the burst instead of lingering for seconds.
- New probe `scripts/perf/lane-perf-probe.ps1` (its own windows, its own temporary config, `Global\wiri-live-test` first, cleanup in `finally`) measures threads, private bytes, working set, handles, GDI/USER objects, idle CPU and idle context switches for a 2-window rest and a ~28-window action workload; `scripts/perf/parse-check.ps1` is the Parser-API check used for the lane scripts and `scripts/perf/thread-name-check.ps1` self-checks the thread-description observer. Lane E's `lane-e-perf.ps1` gains `p_rest_threads_total`, `p_rest_threads_named`, `p_rest_threads_unnamed`, `p_rest_threads_unavailable` and one `p_rest_threads_by_<description>` per thread name, all information only.
- Measured on TRIDARIS with three runs per build against an unmodified `810e896` build in a second target directory ([method and table](docs/PERF.md#numbers)): **threads 14 → 12 at rest and 17 → 13 after the action sequence**; private bytes 7.90 → 7.52 MiB at rest and 9.00 → 9.53 MiB after (overlapping spreads), working set 23.33 → 23.18 MiB and 28.27 → 27.06 MiB, handles 214 → 211 and 263 → 228, USER objects 32/37 → 28/28; idle CPU stays below the 15.625 ms counter increment and idle context switches stay in the same 16-19/s band. The lane does **not** reach the proposed ≤8 settled threads: seven of the remaining twelve are wiri's own and the other five could not be attributed per thread here, because this host's `GetThreadDescription` refused every call (candidates are the `notify` backend, the lazily started workspace-indicator overlay and Win32/WinRT-owned threads; the record does not claim which). The next step is the budget's own "shared watcher delivery": one `ReadDirectoryChangesW` wait on the existing `wiri-config-reload` thread instead of the `notify` backend.

### Lane EVIDENCE — every partial parity row classified, and eight live gates for the evidence gaps
- New [docs/parity/partial-rows-2026-10-30.md](docs/parity/partial-rows-2026-10-30.md) puts one primary condition on each of the 92 `partial` rows of both matrices and states the exact remaining condition in one sentence: 5 missing or incorrect software, 15 implemented but evidence not reconciled, 37 hardware evidence unavailable and 35 Windows backend differences. Matrix totals are unchanged (**164 done / 45 partial / 17 N/A** layout, **141 done / 47 partial / 71 N/A** config/input/IPC, 305 / 92 / 88 of 485) because nothing was promoted. Promotion needs a named live gate that passed in a complete official run, and this pass could not read `C:\wiri-dev\evidence\<run>\`; the rows therefore name the gate they still need instead of claiming a pass.
- New `scripts/live-phases/lane-evidence.ps1` (dot-sourced from `scripts/live-test.ps1` next to the other lanes inside `try { . lane } catch { LanePhaseError 'lane_evidence' $_ }`, gate defaults in the summary block, both files parse-checked with the PowerShell Parser API) adds eight `h95_*` gates for behaviour that existed but had no live observation of exactly it: the overview workspace shadow and its `off`, the deprecated per-output `background-color` reaching the overview backdrop, `center-column` on 320 px and 640 px edge columns, the tab indicator's urgent colour overriding the selection, Mod+drag move / resize / Escape-cancel on a floating window through real `SendInput` grabs, and `toggle-keyboard-shortcuts-inhibit` blocking exactly the inhibitable binds while `allow-inhibiting=false` keeps firing. Owned fixtures only, each group with its own `try`/`catch`, and a `finally` that releases held synthetic input; no gate has run in a cycle, so no row changes status.
- Six matrix rows now cite the gate they are waiting on — `center-column`, Mouse move/resize of floats, Tab indicator, Workspace shadow in the overview (layout) and the deprecated `background-color` plus `toggle-keyboard-shortcuts-inhibit` (config) — and the README totals table was corrected to the current 164/45/17 and 141/47/71 recount.

### Lane OVERVIEW — the overview is niri's fixed-zoom camera over the 2D grid
- The overview no longer computes a fit-everything zoom. niri's `compute_overview_zoom` (src/layout/mod.rs:5034) is now the rule: `zoom = clamp(overview { zoom }, 0.0001, 0.75)` (default 0.5), animated as `1 - progress * (1 - zoom)`. With many columns the camera pans instead of shrinking the thumbnails to nothing, and `zoom 0.3` after a reload re-scales the whole overview. `enter_overview`'s legacy fit computation (width budget from the widest strip, height budget from the number of non-empty workspaces) is deleted; the geometry lives in the new pure `layout::overview_geometry`.
- The workspace stack is niri's `Monitor::workspaces_render_geo` (src/layout/monitor.rs:1467): workspace height `ceil(view * zoom)`, gap `round(0.1 * view height * zoom)` (at least 1 px), the focused workspace centred, one entry per workspace — empty workspaces keep their slot instead of being skipped — and the stack follows the focused workspace as a camera (`workspace_render_idx`), animated with the existing workspace-switch timeline, so `focus-workspace-down` pans in y instead of jumping.
- Each workspace is drawn through its own horizontal camera (`overview_geometry::workspace_camera`, the seam the opt-in unified-grid mode will fill) and cropped to its own rectangle, as niri's `render_workspaces` crops each workspace (src/layout/monitor.rs:1735). The layout already subtracts the workspace's own `scroll_offset.x`, so `workspace_camera_adjustment` applies only the difference a shared monitor camera would need. DWM thumbnails are published with that workspace crop (`update_thumbnail_clipped`, `thumbnail_geometry`'s proportional source crop) and hit-testing only selects the part inside the workspace, so a long strip is cut at the workspace edge at any zoom and `focus-column-right` pans the strip instead of showing every column at once.
- Opening the overview no longer resets the active workspace's `scroll_offset.x`: niri keeps each workspace's own view offset.
- Fixed after the desktop probe: the camera had been applied **twice** (once by the layout, once more by the scene), which pushed the focused column off its workspace. `the_workspace_camera_is_applied_exactly_once` pins the arithmetic.
- Tests: `layout/overview_geometry.rs` (zoom from config and clamping, the open/close interpolation, workspace size/gap, the centred stack with visible neighbours, animated vertical panning, a 3x10 grid navigation case, an empty workspace keeping its slot, a cropped edge thumbnail and a dropped one, and the zoom-times-tile-size fixture); the existing engine/overlay overview tests are re-pinned to the new geometry, and a new `thumbnail_cropped_at_a_workspace_edge_keeps_a_proportional_source` case pins the DWM source crop. `cargo test --release --no-fail-fast -j 2` is 2263 passed / 0 failed / 3 ignored. Live gates: eight `h97_*` in `scripts/live-phases/lane-overview.ps1` plus six `p_overview_*` information keys, parse-checked, not yet run in a cycle.
- Desktop probe (2026-10-30, this worktree's release build, 30 owned forms over 4 workspaces, 1366x728 work area, default `zoom 0.5`): the stack published 5 entries at x=342, y=182/582/982/1382/1782, all 683x364, stride 400 — exactly `ceil(view*zoom)` and `round(0.1*728*0.5)` with the focused workspace centred (382 = 364/2). The focused tile measured 175x356 px = `zoom` x the 350 px fixed column and 356 of the 364 px tile. `focus-column-right` moved the strip under a fixed workspace frame; `focus-workspace 2` moved every stack entry by exactly -400 px (one stride); `overview { zoom 0.3 }` after a reload gave a 219 px workspace (= ceil(728*0.3)) and a 105 px tile (= round(350*0.3)); clicking the panned thumbnail of the focused window closed the overview with that window in the foreground. Information only: overview open 470-545 ms, close 443-524 ms, camera publish interval mean 35-47 ms / max 61-93 ms (spring, containing desktop), 34 live DWM thumbnails for 30 owned windows plus 4 foreign ones, 344-703 ms of daemon CPU over the open+close window.


### Lane DPI task 1 — border, focus ring and shadow are logical pixels projected per output
- `border { width }`, `focus-ring { width }` and the window-rule overrides of both now keep their fractional LOGICAL value (`RingStyle::width` and `RingOverride::width` are `f64`; the decoder no longer rounds). The engine turns them into physical pixels once for the output a window is on, with niri's `round_logical_in_physical_max1` (`layout/tile.rs:244,252-257`, `utils/mod.rs:185-191`; new `layout::metrics::ring_width_px`): zero stays zero, any other width is at least one physical pixel, halves round away from zero. `LayoutConfig::from_config_at_scale` carries the result in `border_width` / `focus_ring_width`, so the tile shrink, native allocation, proportional widths, the drawn band and the ring's outward extent follow the output's scale, and a scale change or a move to another output re-projects them. `RingOverride::inset` takes the scale for rule widths and for the saved width of an off layout border a rule enables (`LayoutConfig::border_inset_with`).
- The shadow's softness, spread and offsets (layout and rule) are projected like niri's `Shadow` (`shadow.rs:43-61`, new `layout::metrics::project_shadow`): the blur sigma scales unrounded, the spread magnitude and each offset are ceiled to a physical pixel.
- 96 DPI is unchanged for whole-pixel values; the flat `border-width` / `focus-ring-width` spellings stay integers. Fractional values now follow niri (a 2.5 px border is 3 px at 100%, 3 px at 125%).
- Tests: `decor_dpi_tests.rs` (fake outputs at 100/125/150/175/200% pinned to niri's formulas, rule overrides, an off border a rule enables, window moved between outputs of different scale, a scale change, the decoder keeping fractions, 100% bit-identical); two existing pins at a 1.5 scale (ipcview border insets, a rule border in `named_config`) now expect the projected pixels.
- The five appearance rows stay `partial`: their remaining condition is physical mixed-DPI evidence (a display scaled above 100%, which this lab does not have).

### Lane DPI task 2 — the floating layer is hidden under a fullscreen tile
- niri does not draw or hit-test the floating windows of a workspace while its active window is a fullscreen scrolling window and the floating layer is not active (`workspace.rs:1722-1728` `is_floating_visible`, `scrolling.rs:2946-2962`). New `engine/float_layer.rs` evaluates exactly that (`FloatLayerFacts`: floating layer active = `Workspace::active_float`, strip not empty, no scroll animation on the output, the active column holds a fullscreen tile) and `reconcile_floats` hides the floats of the active workspace with the same `show_window(false)` path as those of other workspaces (`layout { hide-method }`), without decorating them, and shows them again in place with their stack repaired when a float takes the focus (`focus-floating`, `focus-window`, a native foreground report), a tile that is not fullscreen takes it, the view scrolls, or fullscreen ends. With `hide-method "cloak"` an Alt+Tab to the hidden float after the hide grace is taken as the user asking for it.
- Not modelled: `FloatingActive::NoButRaised` (focus-follows-mouse activating a tile without raising); wiri's hover focus is a normal activation, so hovering the fullscreen tile hides the floats like an explicit focus.
- Tests: ten engine cases in `float_layer.rs` (truth table of the niri condition, hide/show round trip with the float's rectangle unchanged, scroll animation, active column, workspace round trip, maximize-to-edges untouched, decorations, two floats, cloak versus hide on an Alt+Tab request, native foreground reports). Live gates: six `h96_*` in `scripts/live-phases/lane-dpi-floats.ps1` (float in front of the tile, fullscreen hides it, `focus-floating` and `focus-window <id>` reveal it at its place, focusing the tile hides it again, leaving fullscreen restores it). Written and parse-checked; not yet run in a cycle.

### Lane CFG — exact config bounds and what a reload re-applies
- **Bounds census.** [docs/parity/config-bounds-census.md](docs/parity/config-bounds-census.md) lists every numeric or ranged niri option with its pinned type, range and error (`niri-config/src/*.rs`), what wiri's decoder did before, what it does now and the regression test that proves it. The decoder now uses niri's exact ranges and integer typing: `output { position }` is a whole `i32` instead of a rounded -1e6..1e6; preset `fixed`, `spring stiffness`, gradient `angle`, `cooldown-ms`, gestures `delay-ms` and recent-windows `*-ms` reject decimals instead of rounding them; window-rule `min/max-width/height` are `u16`; `opacity` and `geometry-corner-radius` accept niri's unbounded range (opacity still clamps when the rule is resolved); preset `proportion`, `slowdown`, `duration-ms` and `shadow spread` accept niri's range and a negative spread is no longer clamped to zero; shadow `softness` is 0..1024; gestures trigger width/height and `max-speed`, recent-windows padding and `max-height` use niri's bounds; and `accel-speed`, `pinch-sensitivity` and `xcursor-size` are type- and range-checked even though Windows cannot apply them. 17 new regressions (`parity_bounds_*`, `parity_unsupported_device_options_*`).

- **Reload plan.** New `src/config/reload.rs` plans the sections niri's `reload_config` re-applies, in niri's order, by comparing the config the engine holds with the newly decoded one. `apply_loaded_config` now gates the environment, switch-event subscriptions, screenshot path, outputs, input/cursor, layout, binds and window-rule re-evaluation on that plan, so a reload that changes only a bind no longer rewrites OS pointer settings or re-places every window; the new config is always published because the engine reads binds, effects and recent-window options from it. A reload now also unnamed workspaces whose `workspace "x"` declaration disappeared, as niri does, while runtime renames survive. 12 new tests (11 plan, 1 engine).

- **Live gates.** `scripts/live-phases/lane-cfg.ps1` (four `h91_*` gates) reloads a config that changes gaps, adds a window rule, adds a chord and switches animations off, and checks each takes effect without a restart. Written and parse-checked; not yet run — the lead's cycle owns that.

### Lane PANGO — configured hotkey titles are Pango markup
- `hotkey-overlay-title` is now interpreted as Pango markup, as niri paints it (`src/ui/hotkey_overlay.rs` with `set_markup`; wiki Configuration:-Key-Bindings, "Custom Hotkey Overlay Titles"). The new pure `overlay/pango_markup.rs` parses every convenience tag (`b big i s small sub sup tt u`) and `<span>` with `font`/`font_desc`, `font_family`/`face`, `size` (1024ths of a point, `pt`, `px`, percentages, `xx-small`..`xx-large`, `smaller`, `larger`), `style`, `weight`, `variant`, `stretch`, `foreground`/`fgcolor`/`color`, `background`/`bgcolor`, `alpha`/`fgalpha`/`bgalpha`, `underline` (none/single/double/low/error) and `underline_color`, `strikethrough` and `strikethrough_color`, `rise`, `letter_spacing`, plus `font_features`/`fallback`/`lang`/`gravity` (accepted, no GDI effect), and the entities `&amp; &lt; &gt; &quot; &apos; &#NNN; &#xHH;`. A literal `&` or `<`, an unknown tag or attribute, an unbalanced tag or a malformed entity is a markup error: the **whole original string** then paints literally, which is what Pango's `set_markup` does, and `config/decode.rs` logs one warning per titled bind at load instead of on every paint.
- `overlay/markup_text.rs` paints the styled runs with `CreateFontIndirectW` per distinct style (fonts cached only while the sheet is shown and deleted with its session), composites `alpha` against the sheet because GDI text has no alpha channel, draws underline/strikethrough as strokes so `underline_color`/`strikethrough_color` work, shifts `rise` on the baseline and applies `letter_spacing` with `SetTextCharacterExtra`. Measurement and painting share the same cached fonts: the chord column is sized from the widest painted chord (`chord_split`) and a styled title is ellipsized by the same runs it paints (`fit_runs`), while a plain title keeps the old one-`DrawTextW` fast path. No timer, thread or polling loop is added, and an unused sheet still allocates no font.
- Stated approximations, kept in the matrix: `size` keywords and `smaller`/`larger` use Pango's documented scale factors, `<sub>`/`<sup>` use fixed ±2 pt rises (Pango's markup cannot compute a fraction of a resolved size), `error` underlines are straight rather than wavy, colour names are a practical subset of the X11 list, and small caps, stretch, features, fallback, lang and gravity are accepted but not rendered by GDI.
- Add the h89_* live phase (`scripts/live-phases/lane-pango.ps1`, dot-sourced next to the other phases): one owned daemon whose sheet carries `<b>`, a `#ff8800` span with `&amp;`, the same words in regular weight and an unknown tag. Windows OCR reads the painted words from a screenshot and the same capture is sampled inside those word boxes for the colour span and for the bold run's heavier strokes; the invalid title must paint its tag text and log exactly one load warning. Six gates, not yet run in a cycle. The existing `h80_hotkey_overlay_title_per_bind` gate now expects `<b>` to be formatting rather than text (it also still requires the title's own words, so the default action description cannot stand in), which is the one existing expectation this lane changes.

### Lane PACE — presentation-paced animation clock
- The animation frame worker no longer sleeps a fixed 16 ms. While timelines run it waits for the desktop's presentation clock: `DCompositionWaitForCompositorClock` (`dcomp.dll`, resolved with `GetProcAddress` on the first animation, so a machine without the export still starts) on a Tokio blocking-pool task, then `DwmFlush`, then a sleep of one interval taken from the output refresh rate (`DwmGetCompositionTimingInfo().rateRefresh`, else `EnumDisplaySettingsW().dmDisplayFrequency`, else 60 Hz) instead of a hard-coded 16 ms.
- Frame timestamps are `Instant`s (QueryPerformanceCounter, the compositor clock's domain), so springs and easings advance by the interval that was actually presented. A missed frame is one catching-up delta — never a burst of extra ticks — a sub-millisecond remainder keeps 59.94 Hz from losing 0.67 ms per frame, and a suspended machine's QPC jump is capped at 1 s of animation time.
- The wait exists only while timelines run: the first animation request arms it and the last finished timeline disarms it, logging one `animation clock: backend=... frames=... mean_us=... max_us=...` line at debug level. No thread, timer or wait is added at rest, the engine lock and hook threads never wait on a frame, and a backend that fails is replaced for the rest of the process instead of retried (`src/animation/`).
- Ten new unit tests cover start/stop on the timeline count, fallback order and downgrade, refresh-derived intervals (including 59.94 Hz), monotonic timestamps with exact long-run accumulation, missed-frame catch-up without bursts and the suspension cap. One `#[ignore]` measurement test waits for 120 compositor frames: median 16.663 ms, p95 16.703 ms, max 16.735 ms, 0.130 ms process CPU per frame on TRIDARIS ([method and limits](docs/PERF.md#animation-clock-measurement-2026-10-30-lane-pace)). The matrix row stays `partial`: this is still not a hardware page-flip hook, and cadence/idle CPU under the official cycle, other refresh rates and multi-output are unmeasured.
- Add the h90_* live phase (`scripts/live-phases/lane-pace.ps1`, included before the lane-e-perf phase): one slowed column resize must log a presentation backend with a mean spacing near the reported interval over at least 30 frames, and a settled daemon must neither spend CPU nor log another burst over three seconds. Both gates are written and have not been run; only the lead's live cycle verifies them.

### Focus core item 3 — new-window focus policy and `strict-new-window-focus-policy`
- Implement niri's decision for a window that opens without an `open-focused` rule (niri:src/handlers/compositor.rs:149-176; Smart in src/layout/workspace.rs:652 and src/layout/mod.rs:1060): a window Windows itself gave the foreground (wiri's counterpart of a valid xdg-activation token, observed with a fresh `GetForegroundWindow` at admission) is focused; otherwise the default heuristic focuses it unless the active window of the focused output's active workspace is fullscreen, and `debug { strict-new-window-focus-policy }` refuses the focus. A refused window stays in the strip and takes the focus when Windows hands it the foreground or the user asks, without the `open-focused false` hand-back. Floating windows, dialogs and startup-scan windows are unchanged; `open-focused true/false` rules still win.
- Parse niri's `debug {}` section: `strict-new-window-focus-policy` (a flag, `true`/`false` accepted) reaches the engine through `LayoutConfig::strict_new_window_focus_policy` and reloads with the config; every other debug option is a reported warning, as before.
- Default behaviour is unchanged except that a background launch no longer takes the focus from an active fullscreen window. Tests: `layout::engine::rules::tests` (truth table, fullscreen guard, strict, rule override both ways, startup and floating exemptions, config reaching the engine), `config::decode` (flag and reported options). New live gates `h92_new_window_*` (`scripts/live-phases/lane-focus.ps1`; a background launch is opened with ShowWithoutActivation) cover the default heuristic, the fullscreen guard and the strict flag; the token-granted path is unit-proven only. Matrix rows updated honestly: layout-behaviour "New-window focus policy" stays `partial` with its gaps named, the `strict-new-window-focus-policy` debug row moves from N/A to `partial`.

### Focus core item 2 — the nine focus-first `*ById` actions are focus-neutral
- `consume-or-expel-window-left/right`, `center-window`, `reset-window-height`, `switch-preset-window-width[-back]`, `toggle-window-floating`, `move-window-to-floating` and `move-window-to-tiling` with `--id` / a typed `id` no longer activate the named window first. Each takes a new engine route (`src/layout/engine/by_id.rs`) that resolves the id to the window's own workspace and output, however inactive, and acts there on that window in place, as niri's `*ById` variants do (niri:src/input/mod.rs:1110-1136, 1684-1736, 2111-2117, 2213-2250). The tiled forms of `switch-preset-window-height[-back]` by id take the same shape.
- niri's details are kept: a floating window is left alone by consume/expel and reset-window-height and is centred and preset-sized by the floating layer; a tile is centred by id only when its column is its workspace's active column; a tile moved between the layers by id becomes active only when it already was its workspace's active window. Every route runs under a no-activation guard, so a layout pass inside the shared float/unfloat/size code cannot hand the foreground to the engine focus.
- `switch_preset_column_width` and `switch_preset_window_height` now share their core with the by-id routes (`apply_column_width_preset`, `switch_tiled_window_height_at`); the focused forms behave as before.
- Tests: `ipc::server::by_id_focus_tests` drives the real `RunAction` handler with `--id` on a two-monitor engine (a non-focused tile, an inactive workspace of another output, a floating window, an unknown id) and proves the effect and that no activation was requested, the simulated native foreground did not move, and the IPC focus, focused output and every monitor's workspace and focus selection are unchanged. New live gates `h92_by_id_*` (`scripts/live-phases/lane-focus.ps1`, next to the other lanes in `live-test.ps1`) check the same on the desktop. Census rows and matrix row 257 updated: the nine actions are `full`, 128 full / 6 partial / 9 N/A; their first official live run is still to come.

### Focus core item 1 — explicit focus converges on the native foreground
- Fix the stale managed focus the official run `aa6c884` recorded in `h15-fullscreen-id`: wiri's `SetForegroundWindow` for k1 was accepted, Windows raised no EVENT_SYSTEM_FOREGROUND for it, and the live engine/IPC focus (and so `focused_window` and the event stream) stayed on k3 for good because only the hook notice moved them. A request for the window that already is the foreground never raises a notice either.
- After wiri's own activation request returns success, `TilingEngine::converge_live_focus` runs a fresh `GetForegroundWindow` query under the engine lock; when it names the requested window the live focus follows through the same `sync_os_foreground` path a native notice uses, and the legacy `window_focused` / `column_focused` pair is queued for the IPC server (the stream's `window_focus_changed` comes from the state diff). A refused request, or one Windows accepted but did not carry out yet, moves nothing.
- Keep the foreground history design: the convergence never calls the native reducer, adds no stamp, dwell or visit, and the header of `foreground.rs` says why the query is a native observation and not a synthetic one. A native notice that does follow still ends the previous visit at its native time and updates state, but is not broadcast a second time.
- Tests: `layout::engine::foreground_runtime` (already-foreground target converges with history unchanged, accepted-without-notice converges, refused does not move live focus, deferred acceptance waits for the notice, a late notice is not told twice and still ends the right visit) and `ipc::server::focus_converge_tests` (typed and `--id` requests, `window_focused` and `window_focus_changed` exactly once, a repeat tells nothing). The h15 setup retry in `live-test.ps1` stays as a safety net; this removes the cause.

### Official live cycle at cc3920d — 761/761 gates, measured event-driven baseline
- The complete locked desktop cycle at `cc3920d` passed **761 of 761 enforced gates with 0 failures** (88 further information entries, 849 report cells). Every failure recorded by earlier cycles is gone: all six `h78_*` cursor-hiding gates, all five H31 focus-history gates including `h31_history_crosses_workspaces`, and lane V's tab gates (H65). `hidden_json_present_after_restart = False` stays **expected-pass** semantics: the crash record is deleted once the windows are back.
- The same commit is clean under `-D warnings`: release build with 0 warnings, `cargo check --release --all-targets` clean, and **1792 tests passed / 0 failed / 1 ignored doctest**.
- [PERF.md](docs/PERF.md#live-baseline) now records this commit as the live baseline: the event-driven perf pass (`0ebdebf`: bounded tokio runtime, event-driven main loop, event-woken backend handler, event-armed reconcile, lazy indicator pump) is what the numbers measure, with deltas against the `af3eb30` record. Measured idle cost in one 30 s window fell to `p_idle_cpu_ms = 0` (below the 15.625 ms counter increment) and `p_idle_context_switches_per_s = 9.16` (was 321.935/s), with threads 24/26 → 14/15 and handles 235/278 → 220/239. Private bytes and working set, and both executables, are **larger** than that record; the baseline states plainly that this run cannot explain the increase.
- Matrix rows are updated only where this cycle supplies evidence, and the recount is one row: `set-column-display normal or tabbed` is now `done` (its only gap was live verification). The two `cursor {}` rows and the tablet stylus row keep `partial` for their documented Windows approximations; only their live-verification evidence changed.

### Lane LAST — cursor hiding and tablet stylus binds
- Decode niri's `cursor { hide-when-typing; hide-after-inactive-ms <ms> }` section (and keep `xcursor-theme`/`xcursor-size` as reported warnings: the Windows pointer scheme owns them). `hide-when-typing` is Windows' own per-user "hide pointer while typing": it goes through lane P's `PointerSettingsBackend`, so `SPI_GETMOUSEVANISH`/`SPI_SETMOUSEVANISH` is written only when the option is configured, the value the machine had is remembered, and it is restored on a graceful quit or when a reload drops the key (`WIRI_POINTER_SETTINGS_DRY_RUN` still records instead of writing).
- `hide-after-inactive-ms` hides the global pointer after that many milliseconds without pointer movement and shows it on any movement or click. It rides the existing low-level mouse hook: one one-shot `SetTimer` on the hook's own message thread, re-armed by pointer activity, existing only while the pointer is visible and the option is set — nothing runs at rest. Hiding swaps all fourteen `OCR_*` system cursors for transparent copies (`SetSystemCursor`); showing and quitting reload the user's scheme (`SystemParametersInfoW(SPI_SETCURSORS)`). The user's cursors are restored on quit, on a reload that drops the option, from the panic hook (with a `try_lock` fallback so a panic inside the state machine cannot deadlock the hook) and once at startup in case a previous run died hidden. Every OS call goes through a small `CursorBackend` trait; unit tests drive a fake and assert the exact hide/show/restore sequences, and a Windows-only test creates the transparent cursor for real (a 32x64 monochrome mask, checked with `GetIconInfo`) without swapping any system cursor — `SetSystemCursor` itself is only exercised by the live gate.
- Decode niri's `TabletStylusButton1`/`2`/`3` bind triggers and dispatch them from the promoted pen mouse events Windows delivers (`MI_WP_SIGNATURE` with the pen bit): the pen tip, the barrel button and a second barrel/eraser button. A stylus bind wins over the plain mouse button the event was promoted to, an unbound pen button keeps its ordinary mouse behaviour, and press/release pairing survives a modifier-first release or a reload.
- Approximation, stated in the matrix: Windows has no global pen-button stream without UIAccess, so stylus binds see promoted mouse events only — no raw `WM_POINTER`, no pressure, and which physical button lands on which promoted button is the driver's choice.
- Add the h78_* live phase (`scripts/live-phases/lane-last.ps1`, included before the lane-e-perf phase): `hide-when-typing` is checked only in dry-run mode (the audit shows the write and the restore) with the machine's real `SPI_GETMOUSEVANISH` value asserted unchanged, `hide-after-inactive-ms` runs one private daemon with a 400 ms timeout (the cursor object must change when wiri hides it, change again on an injected move and be restored after the quit; the phase's `finally` reloads the user's cursors unconditionally), and the stylus binds are decode+registration evidence only. All six gates passed in the 2026-10-27 official cycle at `cc3920d` (`hide_when_typing_dry_run`, `hide_when_typing_real_value_unchanged`, `cursor_hides_after_inactive_ms`, `cursor_shows_on_pointer_movement`, `cursor_restored_on_quit`, `tablet_stylus_bind_decode_and_registration`), in a cycle that passed 761/761 enforced gates with 0 failures; the release suite there is 1792 passed / 0 failed / 1 ignored doctest.

### Live-gate fixes (34694e4-live)
- Config strings: a quoted string that starts with three or more backslashes is KDL's escaped spelling of a UNC/device name (`"\\\\.\\DISPLAY1"` is `\\.\DISPLAY1`), not a literal path. The path-compat rule kept all four leading backslashes, so `output "\\\\.\\DISPLAY1" { hot-corners ... }` (the spelling in `docs/modules/CONFIG.md`) named no monitor and was silently ignored. The literal `"\\.\DISPLAY1"` is unchanged.

### Lane C2 item 1 — config policy and diagnostics
- Match explicit CLI → nonempty WIRI_CONFIG → compatibility NIRI_CONFIG → APPDATA selection; never create explicit paths. Notify after creating the implicit default and support `config-notification { disable-failed }` without hiding logs/events.
- Reject duplicate singleton nodes with both source locations and invalid ranges/fractional integers. Render source labels, help and include chains in validation/logs. User binds replace defaults unless the wiri `extend-defaults` extension is explicit.
- Add offline `wiri validate [-c path]`, generated shell completions, pure policy/validation tests and H66 offline checks; desktop evidence is tracked in `docs/parity/lane-c2-report.md`.

### Lane C2 item 2 — binding identities and replacement
- Keep omitted and empty bind tables empty at runtime; built-in defaults require explicit `extend-defaults=true` (emergency invalid-startup fallback remains separate).
- Share canonical XKB/Win32/legacy key names and final-Mod chord identities across validation and input. Unknown modifiers and same-part duplicate aliases are errors with both source locations; included parts override earlier matching chords.
- Distinguish Ctrl+right-Alt (AltGr/Mod5/ISO_Level3_Shift) from Ctrl+left-Alt in the existing hooks, never register the hook-only bit with RegisterHotKey, and reject indistinguishable level-5 modifiers. Add pure normalization/input regressions and H67 checks.

### Lane C2 item 3 — explicit keyboard configuration
- Apply only explicitly configured repeat delay/rate and an already-loaded foreground keyboard layout. Track original values through reload/removal/exit and relinquish ownership after external changes; Windows repeat granularity and asynchronous layout acceptance remain approximations. Native NumLock requests warn without changes because queue-relative toggle state cannot provide a trustworthy background-thread restoration baseline; dry-run models it only.
- Add synthetic backend/lifecycle tests and a lifetime-stable `WIRI_KEYBOARD_DRY_RUN=1` adapter. The shared live harness always selects dry-run; no real native keyboard setting or layout mutation is part of validation.

### Lane C2 item 4 — child environment and switch events
- Apply string/null environment entries to every non-elevated argv/shell/startup/IPC launch through one Unicode Windows snapshot, including executable lookup. Keep the parent environment unchanged, label case-insensitive duplicates, and refuse UAC broker launches that cannot honor overrides.
- Run shell startup commands through the same path, suppress both startup forms with `--no-autostart`, and never rerun them on reload. Observe configured lid/slate edges on the existing hidden-window pump, with no initial spurious edge, polling or extra thread; physical switches are synthetic-tested only.
- Add H69 child snapshots and reload/startup checks using private profiles and PID-verified IPC.

### Lane C2 item 5 — named workspace configuration
- Resolve workspace-local layout after global and output settings throughout placement, targeted rules, sizing, presets, decorations and overview. Rebuild views on reload/removal without losing inherited values; reject output-only options in workspace layouts.
- Preserve repeated `open-on-output` priority and reevaluate on config/output/name events, migrating columns, floats and minimized homes while keeping layout/focus state. Add nine synthetic integration regressions and overview geometry coverage, plus H70 single-display width/reload checks; multiple displays are not live-tested.

### Lane C2 integration on the merged lanes
- Port C2 onto the stable ordered workspaces: a `workspace` layout block is found by workspace *name*, so it follows renames, reordering and moves between outputs. Output preferences (repeated `open-on-output`, then the legacy `monitor`) are reevaluated on output and workspace-name events (a reload rebuilds the layout views but never re-places a realized workspace, as Lane F requires) by moving the whole live workspace through the same path as `move-workspace-to-monitor`; with no connected preference a workspace stays where it is.
- The generated first-run config and `examples/minimal.kdl` now say `extend-defaults` in `binds {}` (the explicit extension C2 introduced), so a first run keeps the full built-in keymap instead of only the 53 written chords; README, DOCUMENTATION and PARITY describe the opt-in. Unit test `the_shipped_configs_opt_in_to_the_builtin_keymap`.

### Lane V integration - overview and tabs on the stable-workspace head
- Port Lane V onto lanes K, W, F2, T2 and FK. Overview workspaces are added by a fresh stable key (names, IPC ids, floating homes stay put), the host paints names and publishes IPC ids, and native drag-and-drop uses the configured overview stack instead of a fixed half scale.
- `close-overview` and Escape are idempotent while the camera closes. A four-finger vertical swipe (Precision Touchpad opt-in) opens or closes the overview; a Windows-promoted touch pans it and a 500 ms rest picks a window up. `gestures { hot-corners }` alone no longer starts the drag-capture adapter.
- The 2026-10-27 official cycle at `cc3920d` passed every enforced lane V gate (761/761 overall, 0 failed), so no lane V row stays `partial` for missing live verification; the rows that keep `partial` do so for their documented Windows approximations and unit-tested-only surfaces. Only `set-column-display normal or tabbed` became `done`; see `docs/parity/lane-v-report.md`.

### Lane V item 4 - explicit tab display and clickable indicators
- Add niri `set-column-display "normal"|"tabbed"` binds, ctl commands and typed IPC with idempotent selection/focus preservation.
- Make tab indicators select/focus hidden tabs without activating the overlay. Apply pinned-niri geometry, signed gap/rounding, colours/gradients and reload semantics; ignore inter-tab gaps and margins.
- Share fullscreen/maximize-to-edges across tabbed columns, suppressing both indicator and reservation until exit. Keep maximize-column distinct. Add pure regressions and H65 desktop checks; evidence and limits are in the Lane V report.

### Lane V item 3 - event-driven overview hot corners
- Enable niri's default top-left physical-pixel corner, named corners and global/per-output `hot-corners` policies. A 120 ms one-shot dwell toggles once per entry; parking or an IPC close cannot rearm it.
- Reuse the existing low-level mouse hook and message thread; no idle timer, poller or worker. Add pure policy/dwell/negative-origin tests and H64 locked-desktop checks (results in the Lane V report).

### Lane V item 2 — overview pointer and normal actions
- Add captured click/drag handling, workspace/new-workspace drops above/between/below, drag dwell, high-resolution wheel workspace navigation and right-drag scrolling. Keep normal actions live and refresh DWM sources after model changes; floating previews participate too.
- Cover pure hit/drag/scroll/insertion/source lifecycle logic and add H62/H63 desktop checks; live results remain explicitly pending in the Lane V report.

### Lane V item 1 — configured overview camera
- Implement niri's overview zoom, backdrop colour and workspace shadow, with a reversible open/close spring on the existing Phase 2B timeline/notifier. `overview-open-close` animation overrides, global off and slowdown apply; no frame clock runs after settling.
- Add pure config, shadow-pixel and camera-lifecycle regressions. Desktop acceptance and compositor limits are recorded in `docs/parity/lane-v-report.md`.

### Lane L2 — missing layout-action rows
- Close eleven rows of the layout parity matrix through the one action grammar (binds, `wiri-ctl action`, typed IPC): `swap-window-left` / `-right`, `move-column-to-index`, `center-visible-columns`, `switch-preset-window-width` / `-back`, `switch-preset-window-height` / `-back` with `preset-window-heights`, `move-window-{down,up}-or-to-workspace-{down,up}`, `move-column-{left,right}-or-to-monitor-{left,right}`, `layout { empty-workspace-above-first }`, `toggle-window-rule-opacity`, `focus-floating` / `focus-tiling` / `switch-focus-between-floating-and-tiling`.
- Add `layout { fullscreen-backdrop-color }`: the existing click-through decor layer fills the monitor's tiling view behind a fullscreen window in the configured colour, only while one is up.
- Decode `animations { recent-windows-close }` and fade the recent-windows overlay out on close over the existing easing curves; one timer runs only while a fade lasts, so a daemon at rest is unchanged and animations-off keeps the immediate close.
- Every action has pure unit tests covering the edges (first/last column, a single column, tabbed columns, floating windows, an empty workspace, the end of the workspace list). Release check with `-D warnings` is clean and the suite is green at 1520 library + 14 CLI tests. The new `h76_*` live gates (`scripts/live-phases/lane-l2.ps1`) are written and parse with zero errors but have not been run; the parity rows therefore say `partial (Lane L2; unit-tested; live pending lead cycle)`.

### Lane P — pointer settings, hotkey sheet at startup, backdrop colours
- Add niri's pointing-device options (`accel-profile`, `left-handed`, touchpad `dwt` and `disabled-on-external-mouse` reach Windows; the options Windows has no lever for are parsed, validated and reported) behind one backend trait, `WIRI_POINTER_SETTINGS_DRY_RUN=<file>` audit recording, remembered originals and restore on quit or when a reload stops naming an option. `toggle-touchpad` emits the niri `touchpad_toggled` event.
- Show the Important Hotkeys sheet at startup with `hotkey-overlay { skip-at-startup; hide-not-bound }`, `--no-startup-overlay` (the live harness passes it) and `--no-autostart` also keeping it closed; decode `layout { background-color }` (and the deprecated per-output key, mapped with a warning). Add the h75_* live phase (`lane-p.ps1`).
- Integration: the backdrop reaches the one overview host Lane V owns (`overview { backdrop-color }` wins, else the layout colour, else niri's default through `set_appearance`); Lane P adds no `overview` section, `OverviewContext` field or hot corner of its own.

### Lane Z — floating layer survives delayed native raises
- Repair actual workspace z-order inversions on native REORDER events, not only on focus/layout. Correct the SHOW/HIDE WinEvent IDs so REORDER is no longer misclassified as HIDE. Keep float MRU and tile keyboard focus with asynchronous relative-HWND placement, without TOPMOST, global float promotion, idle timers or new threads.
- Add deterministic H45 delayed-raise coverage and opt-in repeated H3/H5/H45 evidence. The delayed-layer check fails 3/3 before and passes 4/4 after; the complete release desktop run passes 500 gates. Release validation passes 1210 library + 14 CLI tests. Record repeat pass rates, one unrelated Terminal focus interruption, measured resting cost and limits in `docs/parity/lane-z-report.md`.
### Lane S item 1 — frozen screenshot selector
- Add an on-demand focused-output screenshot UI with selection/Space-drag, pointer toggle, save/copy/cancel and absolute destinations through binds, CLI and typed IPC. Frames are frozen before the initially nonactivating host is shown; UI pixels never enter the saved crop.
- Add pure geometry/options tests and H46 locked desktop checks: 9/9 passed in the interrupted run; full-suite sign-off remains pending. No new crate, timer or idle worker.
- Finalize the last drag coordinate before reentrant mouse-capture release; held Space autorepeat cannot save when a move-drag ends. Two pure regressions pass.

### Lane S item 2 — screenshot action options
- Screen/window captures accept niri disk/pointer flags and absolute paths through binds, CLI and typed IPC. Explicit managed-window ids capture without activation; copy-only overrides a supplied destination.
- Share the existing PNG/clipboard/event pipeline and composite the pointer only for its receiving window. Add six action tests and typed-wire coverage; all six H47 gates passed in the interrupted desktop run (full-suite sign-off pending).

### Lane S item 3 — non-layout system actions
- Add foreground keyboard-layout switching, managed-window urgency actions without focus changes, asynchronous quit confirmation with an explicit skip flag, active config-file switching, monitor power controls and suspend. Spawn expands only a leading home component in its executable, never the other arguments.
- Add pure parser, typed-wire, layout-selection, quit-property and urgency tests. Typed IPC reaches every system-action executor; urgency flashes the taskbar without activation and clears on foreground before the separate hook event arrives. Legacy management `wiri-ctl quit` remains immediate; action/bind quit confirms unless skipped.
- H48/H49 checks record events, config-file reload, literal spawn arguments, confirmation and dry-run power audits. Fix their missing F10 mapping and avoid serializing provider-decorated file strings. The interrupted run recorded 18/21 Lane S gates passing; its three failures have targeted corrections but no permitted rerun. See `docs/parity/lane-s-report.md`; no complete live pass is claimed.
### Lane W — gradients and interactive insertion
- Render all niri gradient spaces and OKLCH hue paths with strict syntax, premultiplied alpha and workspace-view coordinates. General Mod+left-drag shares its insertion-slot calculation with the lazy hint after niri's logical pull-out threshold; clicks stay put, release inserts a tile, and Escape/Mod release restores the untouched source. No new dependency, timer or thread.

### Lane W — policy event delivery
- Queue tagged wheel replacements after the low-level hook returns, avoiding duplicate original-plus-scaled delivery. Clear rejected late foreground flashes in the native urgency registry so the next background request is not lost. Both fixes use existing event pumps only; no added timer or thread.

### Lane W — window-policy defaults
- Add niri per-window column display, anchored first-floating position, application-wheel multiplier and latched ignore/set-urgent/focus policies for actual shell attention requests. Remember floating moves, preserve existing columns and never treat user focus as an application activation request; Windows cannot supply Wayland serials or veto OS-granted foreground.

### Lane W — per-window appearance
- Add sparse niri window-rule focus-ring, border, shadow and tab paints, four-corner geometry and opt-in native clipping with original-region restoration. Attention uses shell flashes/ALERT, not generic accessibility state changes. Changes follow existing rule/focus/resize events; no resting timer or thread. Foreign-HWND clipping is binary and can suppress DWM shadows; compositor blur remains unsupported.
### Lane K — measured evidence and H22 diagnosis
- Record the saved lane-local result (497 pass / 6 fail; all ten H54–H57 checks pass), animation CPU/frame samples and before/after idle/resources in the parity report and PERF.md. Cite the Terminal foreground takeover that aborted H22 and the separate stale shutdown-helper pipe; no code or gate changed. Keep Windows approximations and unmeasured costs explicit; integrated verification remains pending.

### Lane K — desktop acceptance checks
- Add H54–H57 lifecycle, two-frame workspace PNG, cancellation, cross-fade and global-off checks plus before/after 30-second idle/resource observations. H17 now observes the moving thumbnail rather than the already-final cloaked HWND; its unsupported-options fixture retains the unimplemented overlay warnings and separately requires open/close blocks to validate without warnings.

### Lane K — part-time DWM compositor
- Animate new tiled windows from half-size/transparent to their final frame while existing strip motion slides neighbours; temporary source cloaks and DWM thumbnails live on one lazy click-through, non-activating topmost layer.
- Copy available pixels before a wiri close request, then fade/shrink the copy only after destruction. Never keep an application HWND alive or wait for an application capture; unavailable, protected and empty sources fall back without a black tile.
- Slide outgoing and incoming workspaces vertically together, retaining their redirection sources until final layout/visibility reconciliation.
- Cross-fade pre-resize content with the resized live thumbnail using the configured Phase 2B timeline. Input, actions, reload and quit snap/cancel; all thumbnails, snapshots and layer resources are released at rest, with no added idle clock, timer or thread.

### Lane K — animation configuration
- Decode niri's `window-open` and `window-close` blocks as independent easing/spring/off timelines, including global off/slowdown and legacy overrides. Defaults are 150 ms ease-out-expo for opening and 150 ms ease-out-quad for closing; absent animations remain disabled for wiri compatibility. Pure tests cover defaults, precedence and invalid/duplicate settings.
### Lane F2 — integrated focus, workspaces and monitor actions
- Connect the lane-F executors through the existing B/2B input and H IPC paths; preserve G's column/tile selection, logical width intent, tab visibility, effective view/struts and float geometry. Workspace identity, names and visible order survive reordering and synthetic hotplug/whole-workspace moves.
- Previous-window focus uses stamped native occurrence order, not delayed delivery or requested activation time. Stale notices finish dwell history without undoing live focus; unconfirmed previous commands resume on observation, preserving exact cross-workspace endpoints without a retry timer.
- Add fail-closed H30–H34 desktop checks through both CLI and typed IPC, plus pure queued-event, rollover, float-memory and visible-order regressions. The Lane F2 parity report separates desktop evidence, multi-monitor unit coverage and remaining gaps.

### Lane F2 — ordered workspace foundation
- Separate workspace order, monitor-local keys and session-stable IPC identities; runtime names belong to workspace objects. Retain Lane G's existing column/tile focus memory rather than adding duplicate state. Mutation executors are connected in the next part.

### Lane F2 — structured layout action transport
- Port lane F's shared focus/workspace/monitor grammar onto the integrated branch. Preserve quoted workspace names and carry explicit window-transfer targets through Lane H's typed action path without focus-first activation. Engine execution follows in the next integration part.

- Lane T: opt-in `input { touch {} tablet {} }` promoted touch/pen Mod drags share move/drop and pixel scrolling; reported secondary touch IDs toggle floating. MI_WP_SIGNATURE decoding, mode selection and contact pairing are synthetic-tested only; no overlay or hardware-support claim.

- Lane T: opt-in niri `gestures` DnD edge view/workspace settings, foreign-capture drag detection and 500 ms hover activation on the existing dispatcher; one-shot dwell and active-edge frames only.

- Lane T: opt-in Precision Touchpad HID Raw Input assembles three-contact frames and feeds shared swipe inertia/snapping; registration and its message-only HWND are removed on disable/reload/quit. Windows gesture settings are never changed.

- Lane T: system-time/rectangle edge double-clicks reset tiled height or toggle full width; Mod move/resize excludes layout writes while held and persists its drop, with right-click floating/tiling toggle.

### Lane T — spatial pointer gestures
- Configured Mod+middle drag follows the pointer horizontally or vertically after niri's axis lock. Release projects the 150 ms velocity history to column/workspace snaps, handing velocity to Phase 2B's spring; bounded end rubber-banding and pause-before-release are unit-tested. Explicit pointer/wheel binds retain precedence and niri syntax. No idle timer or extra frame thread. Native workspace previews still show only the active HWND scene, not niri's two-workspace compositor crop; see the Lane T report.

### Phase 2B integration — desktop sign-off
- Integrated release validation passes: 1114 library + 7 CLI tests, build and all-target check with warnings denied. The complete locked desktop run passes 440 gates, zero failures (102/102 Lane A checks; Lane B/D retained); perf collector errors and final H9 terminal overlaps are zero. Record earlier findings, measured resting cost and remaining limits in the Phase 2B report; recount matrix totals without claiming new features.

### Phase 2B integration — guarded H5 input setup
- Fix a real older H5 fixture race on the shared desktop: settle before requesting focus, not after, then require and record the float as foreground immediately before its real workspace-move chord. An unrelated terminal took focus during the old 1.5-second gap. Add a precondition gate and evidence without relaxing any existing assertion or changing product targeting.

### Phase 2B integration — live fixture compatibility
- H16 now configures Alt as Mod so its unchanged Ctrl+Alt input exercises lane B's Ctrl+Mod column-reorder gesture, rather than the plain Mod move gesture. The insert-hint assertion is unchanged.
- Fix a real older perf-observer bug: give its owned child an explicit unique `WIRI_PIPE` endpoint and use that endpoint for every PID-validated request instead of the unused global pipe. No measurement thresholds or earlier gates change.

### Phase 2B integration — single-instance probe
- Resolve the startup probe and startup log with `ipc::pipe_path()`, matching the server/client user-session endpoint and `WIRI_PIPE` override instead of probing the unused global prefix. The server's listener check remains unchanged.

### Phase 2B integration — desktop acceptance checks
- Add the source lane's 102 H16–H21 checks through one Lane A entry point, retaining H22–H29, IPC and performance phases unchanged. Classify an intentional decoration host as passive only when both its class and all required extended styles match; H16 separately checks styles, shell exclusion and cleanup. No older assertion is weakened.

### Phase 2B integration — look and feel on the shared input pump
- Land configured spring/easing motion, unified passive decorations, active-only tabs, live overview/recent previews and Important Hotkeys from lane A. Preserve lane B's primary keyboard engine, pointer/focus handling, fallback bindings and message-thread dispatcher: no legacy keyboard relay thread/channel.
- Modal dismissal and recent events keep physical modifier state and captured down/up pairs synchronized; tagged activation input cannot promote MRU. Keep lane D screenshots/output handling unchanged. See the Phase 2B report for source-run evidence, completed integrated validation and remaining limits.

### Phase 2B integration — recent-window diagnostics
- Preserve lane A's shell-collision-free recent-window fixture and foreground/preview diagnostics; no assertions are relaxed.

### Phase 2B integration — recent-window service
- Import the isolated recent-window session/config/preview model and desktop checks, ahead of re-hosting its keyboard events on lane B's input pump.

### Phase 2B integration — overview host
- Port the session-local DWM overview host, hit testing and H19 desktop fragment; resources are created only for an overview session.

### Phase 2B integration — decoration foundation
- Port the Phase 2B effects decoder, pure decoration model and passive layer foundation onto the integrated input/output base, retaining both backend module lists.
### IPC 1.3 — transport compatibility (Lane H)
- JSON-line connections accept repeated requests and one-line replies, including recoverable
  unknown-request errors. Existing undelimited one-shot clients still receive EOF.
- Niri's externally tagged requests, Ok/Err replies and event spelling coexist with wiri's
  envelopes. ReturnError exercises client error handling; wiri replies name the IPC schema.
- Spawned clients inherit WIRI_PIPE. Framing is bounded per line and subscriptions release on
  disconnect without an idle timer.

### IPC 1.3 — CLI and reusable client (Lane H)
- Reusable connect/request/read-events client, per-request commands, optional `msg` prefix,
  `--print-request`, raw JSON argument/stdin requests and schema mismatch guidance.
  `windows` uses niri's objects; `window-list` retains wiri's geometry list.

### IPC 1.3 — output and keyboard queries (Lane H)
- Outputs and FocusedOutput expose connector/EDID identity, DXGI rational-rate modes and
  logical geometry; unknown VRR/max-bpc capabilities remain null. KeyboardLayouts follows the foreground
  thread with localized layout names, not the IPC caller's thread.
- Temporary output requests reuse the conservative target-only executor; layout scale
  never changes Windows DPI. Unsupported settings and last-display-off are errors.
  Mutating output paths are synthetic-tested only, never applied on the shared laptop;
  a successful temporary change answers `Ok.OutputConfigChanged: Applied`.

### IPC 1.3 — interactive picks (Lane H)
- PickWindow and SDR PickColor consume click down/up on the existing input hooks;
  Escape returns null, disconnect cancels, concurrent picks fail, and release guards
  survive completion. No extra overlay, idle timer, thread or dependency.

### IPC 1.3 — complete state events (Lane H)
- Window objects include daemon-relative monotonic focus timestamps and full logical
  layout geometry before offscreen parking, with dedicated deduplicated delta events.
- Keyboard layout state replays before deltas; foreground and shell language notifications
  on the existing hotkey window produce list/switch events without polling.
- Automated H35-H39 live gates cover both wire dialects, queries, timestamps/layouts,
  click/Escape consumption and legacy CLI compatibility. A missing second loaded keyboard
  layout is reported as an explicit skip, not a passed switch check.
- The performance collector resolves `WIRI_PIPE` or the same per-user/session pipe as
  the daemon instead of the retired global name, with a pure endpoint self-test; earlier
  unavailable samples were collector timeouts, not measurements.
### Lane G — strip layout and native window manipulation (2026-10-25)
- **Strip policies and persistent widths.** niri centering modes, the single-column override,
  mixed fixed/proportional presets and layout default widths act on the effective output view.
  Explicit widths survive opening/closing neighbors; new columns insert right of the active one,
  and stacked-tile/workspace activity survives navigation beyond the bounded MRU history.
  Signed struts no longer wrap to an empty work area. Legacy equal-sharing remains compatible.
- **Owned dialogs auto-float.** Dialogs center over their owner and follow its workspace/output
  unless placement rules override; ordinary top-level apps stay tiled and `open-floating false`
  suppresses the default. Existing workspace-float stacking, non-scrolling geometry and memory
  are regression-tested rather than replaced.
- **Native caption/edge drags reconcile on drop.** The app owns its HWND throughout the native
  move/size loop. On release wiri adopts column width and stacked-tile height, preserving sibling
  allocation and float geometry; a caption-only tiled move deliberately snaps back to its slot.
  H40-H44 add real desktop checks, including refresh while the mouse is held. Regressions cover
  restore/workspace/monitor transfers preserving requested width intent, small proportions not
  being re-derived after a pixel minimum, `center-focused-column` acting on the effective output
  even for first/last columns, inactive admission and `open-focused false` restoring the remembered
  column/tile focus, unsorted float presets remembering their configured cycle, and legacy rule
  widths outranking the layout default. No new dependency, idle timer, thread or resting window.
  Limitations/evidence: `docs/parity/lane-g-report.md`.

### The pipe belongs to the user and the session (2026-10-25)
- **A second user can run wiri.**  The pipe name was machine-global
  (`\\.\pipe\wiri_control`), so the second user logged in on the same machine (fast
  user switching, an RDP session) could not create its pipe, was told "another wiri
  instance is already running", and its clients would have talked to the first user's
  daemon.  The pipe is now the pipe of one user in one session:
  `\\.\pipe\wiri_control_<user>_<session>` (the account name and the session id, made
  safe for a pipe name).  The server, the client and the single-instance check use the
  same function (`wiri::ipc::pipe_path()`), and `WIRI_PIPE` overrides the name.

### IPC 1.2 — niri's request/reply forms (2026-10-25)
- **Five requests answer with niri's shapes.**  `workspaces`, `windows`, `focused_window`,
  `overview_state` and `version` are answered from the engine's current state, with the
  same description the event stream is built from: niri's `Workspace` and `Window` objects
  (`id`, `idx`, `name`, `output`, `is_active`, `is_focused`, `active_window_id`; `id`,
  `title`, `app_id`, `pid`, `workspace_id`, `is_focused`, `is_floating`, plus the
  `is_urgent` and `layout` the stream already sends).  What Windows cannot tell is
  `null`, never missing: a workspace without a name, the program of an elevated window,
  the process of a window wiri does not know, the workspace of a float whose monitor is
  away.  niri's `focus_timestamp` has no Windows meaning and is therefore not reported.
  niri's own spelling of the request (`"Workspaces"`, `"Windows"`, `"FocusedWindow"`,
  `"OverviewState"`, `"Version"`) is read as well.  The IPC schema is 1.2.
- **`wiri-ctl workspaces`, `windows`, `focused-window`, `overview-state` and `version`.**
  Each of them prints a readable table (or a line), and `--json` prints the raw reply.
  `wiri-ctl windows` now answers with niri's shape; the older list (the class, the
  geometry, the place in the strip) is `wiri-ctl window-list` now.
### Phase 2C — Lane D screenshots and outputs (2026-10-25)
- **Focused-output and focused-window PNG screenshots.** Hotkeys, tray and window-capture IPC share
  one on-demand WIC PNG / clipboard pipeline. Windows.Graphics.Capture is tried first for windows,
  with PrintWindow fallback; captures use request-local WinRT factories so a second capture stays
  safe after COM teardown. `screenshot-path` accepts a strftime template, leading `~`, or `null`
  (clipboard only); the default is the redirected Pictures\\Screenshots folder. Saved/copied tray
  balloons and `screenshot_captured { path }` events cover both actions. Live H26-H28 verify PNGs,
  content, clipboard, reload and null events. Interactive selection and per-action pointer/disk flags
  remain outstanding; see `docs/parity/phase2c-lane-d-report.md`.
- **Output configuration acts on startup/reload.** Connector or EDID identity matching is
  case-insensitive and no longer falls back by index for an unmatched name. Unset scale keeps real
  Windows DPI; startup focus follows config order. A conservative CCD planner applies only explicit
  physical differences, preserves other timings, refuses the last-display-off and unsupported flips,
  and never persists changes. H29 verifies matching, focus, reload and current-mode no-op; physical
  mode/position/rotation/off preparation is synthetic-unit-tested only, never applied on the laptop.
- **Terminal minimum-width regression.** An unanswered admission query can now retry when the
  title becomes ready, still outside the engine lock and within the existing three-query cap.
  Native visible minima include wiri's border in tile units, just like rule minima. Two pure
  regressions and the unchanged live H9 gate verify three 466px terminals with no overlap.
- **Safer autostart command and elevation guidance.** Run startup always quotes the executable,
  including paths without spaces; four pure regressions cover spaced, unspaced, Unicode and UNC
  paths without writing the registry. `docs/ELEVATION.md` compares normal integrity, an opt-in
  interactive elevated task and signed uiAccess; no elevation mechanism is enabled.
- **Shared-desktop live safety.** The harness refuses foreign daemons instead of killing them,
  isolates AppData/temp inside the worktree, and retains interrupted-run recovery state. The
  recovery metric checks this run's forms, not unrelated windows legitimately hidden again.
### Phase 2C — input (lane B)
- Keyboard binds now run before the shell in `WH_KEYBOARD_LL`, including configured Win chords;
  Win+L, Win+D and Win+Shift+S remain Windows-owned. Bound down/up pairs, per-bind repeat/cooldown,
  manual shortcut inhibition (`allow-inhibiting=false` escape hatches), and the wiri extension
  `on-release=true` work without stealing unbound keys or faking modifier releases. RegisterHotKey
  remains a down-only fallback; the hook uses the existing hotkey thread instead of two extra threads.
- Mouse button and vertical/horizontal wheel binds use niri trigger names, paired suppression,
  per-bind cooldown and per-axis consumption. High-resolution Windows touchpad wheel reports
  accumulate into ticks (TouchpadScroll names alias that stream). Configured Mod+left/right
  gestures work over nested client HWNDs, and resizing persists into floating/column/tile state.
  The old hard-coded Alt/wheel shortcuts and per-wheel worker threads are removed.
- Pointer focus is event-driven on the existing input pump: nested-window hit testing, a cancellable
  one-shot dwell, `max-scroll-amount="N%"`, output entry and configured warp modes replace the
  500 ms polling task. Foreground events cancel stale hover requests; pointer-driven focus never
  warps and a hidden cursor stays hidden. Warp completion waits for asynchronous native geometry
  against the submitted layout frame, not stale observed bounds. Zero-delay hover uses the hook's
  logical point instead of the not-yet-updated native cursor position. Drag motion keeps the system
  cursor moving while paired button edges stay captured. No new dependency, idle timer, thread
  or resting HWND.

### Phase 2A — a real config parser, window rules, fullscreen, IPC (2026-10-25)
- **A real KDL config parser with visible errors.**  The line scanner is gone.  `src/config/kdl.rs` reads
  KDL (nested blocks, `/-` slashdash, `/* */`, raw strings `r#"..."#`, escapes, numbers, a BOM, UTF-16
  files) with no new dependency, and `src/config/decode.rs` maps it onto the config and reports every
  problem with `line:column`.  niri's syntax loads as written: a `binds { Mod+T { spawn "wt.exe"; } }`
  block with `repeat=false`, `cooldown-ms=`, `allow-when-locked=` and `hotkey-overlay-title=`, `input`,
  `layout` (`gaps`, `border`, `focus-ring`, `shadow`, `struts`), `output`, `workspace`, `window-rule`,
  `animations`, `spawn-at-startup`, `include`.  wiri's older spellings keep working.  An unknown section
  or key, a value of the wrong type, a bad color, a misspelled action or key and a bad regular
  expression are errors (with a "did you mean"); a niri option wiri lacks, a bind that cannot work on
  Windows and a duplicate bind are warnings.  `cooldown-ms=` and `repeat=false` act (the keyboard hook
  tells key auto-repeat from a first press), `hotkey-overlay-title=` titles the key-bindings overlay.
- **A bad config is never applied half.**  At startup an erroneous config is not used: wiri starts with
  its defaults and a tray balloon names the first error (`file:line:column`).  On a reload the last good
  config stays in use, a balloon names the file and line, every error is in the log, and a
  `config_loaded` IPC event with `failed: true` and the error text is sent.  The reload is a
  trailing-edge debounce: the file is read once, 200 ms after the last change to it or to a file it
  includes (included files in other directories are watched too), so an editor that saves in steps never
  gets half a file applied.  `wiri-ctl validate-config [path]` runs the same parser and also checks the
  included files; `wiri-ctl reload-config` answers with the first error.
- The shipped `resources/default_config.kdl` and `examples/` are written in niri syntax.  A string that
  starts like a Windows path keeps its backslashes (`"C:\Program Files\x"`), elsewhere a bad escape is an
  error that points at it and a raw string always works.
- **Window rules act.**  Several `match` lines are OR-ed and `exclude` lines veto (a window matches a
  rule when it matches any `match` line and no `exclude` line); `app-id` and `title` are regular
  expressions searched in the process's file name (`firefox.exe`, and without `.exe`) or the
  AppUserModelID, and in the title.  The `open-*` properties were parsed and never read; now
  `open-on-output` (a monitor by its device name, `wiri-ctl list-monitors` shows it), `open-on-workspace`
  (by name) and `open-in-workspace` (by position; the older `workspace "2"` / `workspace "web"` key is a
  position or a name, it used to be an internal id), `open-floating` (centred, at the size
  `default-column-width` / `default-window-height` ask for), `open-fullscreen`, `open-maximized` and
  `open-focused false` (the window that had the focus keeps it) all do what they say.
  `default-column-width` (a proportion, a fixed size or the window's own), `default-window-height`
  (used by the layout now: a tile of a column of several windows gets that height) and the limits
  `min-width`, `max-width`, `min-height`, `max-height` hold columns and tiles; `opacity` multiplies with
  `dim-unfocused`.  Opacity and the limits **follow the window**: they are worked out again when its
  title changes, when the focus moves, when it floats or tiles again, when it starts or stops flashing
  and when the config is reloaded - nothing polls, and only windows that a rule depends on are looked
  at.  `wiri-ctl list-monitors` shows the device name of each monitor.  The process name the rules see is
  the file name (`firefox.exe`; `process-name "firefox"` still matches).
- **Fullscreen and maximize keep the window in the strip (niri).**  A fullscreen window used to hide every
  other tile of its monitor (they left the taskbar and Alt+Tab).  Now it keeps its column and its place in
  it, its column is made as wide as the view meanwhile (and gets its width back), the window is drawn
  over the monitor in its slot and the other windows are neither hidden nor moved but out of the way:
  focusing a neighbour scrolls the strip and the fullscreen window goes off screen with its slot.  The shell
  is told (`ITaskbarList2::MarkFullscreenWindow`) so that the taskbar goes and comes back.  New niri action
  `maximize-window-to-edges` (default bind `Mod+M`, rule `open-maximized-to-edges`): the window fills the
  work area with no gap and no border and keeps its column; wiri's older `maximize` is the same state
  instead of a native maximize that the layout fought.  An application that goes fullscreen on its own
  (a video player, F11) or whose maximize button is pressed is followed: it is adopted into the same
  states (its frame is not touched), and wiri tiles it again when it leaves them.  `wiri-ctl windows` shows
  `fullscreen` and `maximized`.  `wiri-ctl focus-window` (and the IPC requests that focus a window first)
  now scrolls the strip to show the window's column, as focusing it with the keyboard does.
- **niri's event stream, and actions with niri's arguments (IPC 1.1).**  A client that subscribes to
  `wiri-ctl events --stream` is told the state first - `workspaces_changed`, `windows_changed`,
  `overview_opened_or_closed`, `config_loaded` - and then every change as niri's events:
  `window_opened_or_changed`, `window_closed`, `window_focus_changed`, `window_urgency_changed`,
  `window_layouts_changed`, `workspace_activated`, `workspace_active_window_changed`,
  `workspace_urgency_changed`, `workspaces_changed`, `overview_opened_or_closed`.  The objects are niri's
  `Window` and `Workspace` (`id`, `title`, `app_id`, `pid`, `workspace_id`, `is_focused`, `is_floating`,
  `is_urgent`, `layout`; `id`, `idx`, `name`, `output`, `is_urgent`, `is_active`, `is_focused`,
  `active_window_id`) plus wiri's `class_name`, `output`, `is_fullscreen`, `is_maximized`, `is_minimized`.
  The events are worked out from the state of the layout once after whatever changed it (a key, the mouse,
  an application, `wiri-ctl`, a config reload), only while somebody is subscribed, with no timer; a client
  that falls behind is told the state again instead of silently missing events.  `workspaces_changed` and
  `workspace_activated` were declared and never sent: they are sent now, with niri's payload.
  `window_closed` is sent for the windows wiri managed (it was sent for every window that was destroyed) and
  carries `id` as well as `window_hwnd`.  wiri's older events are unchanged.
  `wiri-ctl action <name> [args]` (the `run_action` request, also spelled `action`) takes numbers and booleans
  as arguments, `quoted` (`focus-workspace "2"` is a name, `focus-workspace 2` an index; in a shell
  `wiri-ctl action focus-workspace '"2"'`) and niri's `--id <hwnd>` for the window to act on.  A live test
  never runs twice at once on the test machine (a machine-wide lock; idea from Codex Sol's ee41d27).

### Phase 1 — safe to daily-drive (2026-10-24)
Each item is verified on a spare Windows 11 laptop with `scripts/live-test.ps1` (real key
presses, `wiri-ctl`, window rectangles); the gate names are in `docs/parity/phase1-report.md`.
- **Unknown modifiers are rejected.**  `Hyper+T`, `Mod5+T` or a typo such as `Crtl+T` used to be
  dropped silently, which registered a bare global `T` hotkey and took that key away from every
  application.  Such a bind is now ignored at parse time with a warning naming the bind and the
  modifier (`wiri-ctl validate-config` reports it as an error), and nothing is registered.  niri's
  `Mod` token is understood: it resolves to the `input { mod-key }` modifier (Ctrl+Alt unless set,
  also `super`, `alt`, `win-alt`, `ctrl-shift`, `ctrl`), so `Mod+Shift+T` ports as written.  An
  unknown `mod-key` value warns instead of silently becoming Ctrl+Alt.
- **`spawn`, `spawn-at-startup` and `wiri-ctl spawn` share one spawner** that keeps every argument
  (`spawn "wt.exe" "-d" "C:\My Projects"` used to run `wt.exe` alone), understands quoted paths with
  spaces and backslashes (the Windows command-line rules, also for `spawn-at-startup` lines), and
  finds programs on the PATH, so the default config's `wt.exe` works (`CreateProcessW` is given a
  properly quoted command line and no application name; `.cmd` / `.bat` launchers are found through
  PATHEXT).  A program started with `spawn` gets its own console window and starts in the user's
  profile; `spawn-sh` runs `cmd.exe /d /s /c` without one.  `wiri-ctl spawn prog arg...` passes the
  arguments through, `wiri-ctl spawn '"C:\x y\a.exe" -f'` takes a whole command line.  The built-in
  `Mod+Enter` starts Windows Terminal and falls back to PowerShell instead of a bare `cmd.exe`.
- **Actions act on the window you are looking at, a floating one included.**  The target of
  `close-window`, `toggle-floating`, `fullscreen`, `center-window`, the width/height presets and the
  float move/resize actions is resolved from `GetForegroundWindow()` first (the application window
  that owns a dialog or popup is found through the owner chain), the engine's own focus second.
  Before, a floating window never held the engine focus, so with a float in front `close-window`
  closed the last focused *tile* behind it and a second `toggle-floating` floated the next tile
  instead of putting the float back.  `close-window` with a dialog in front closes the dialog (like
  Alt+F4), and it refuses to guess when an unmanaged window (the Start menu, an elevated program) has
  the focus.  A floating window is moved by 50 px with `move-column-left/right`, resized with the
  width/height actions and centred with `center-window`; `fullscreen` covers the monitor the window
  lives on and gives a float its rectangle back.  A window floats where it floated last time (the
  first time: its own size, in the middle of the work area) and is remembered per window.
  `wiri-ctl focus-window <hwnd>` now raises a floating window (it used to answer "ok" and do
  nothing), `wiri-ctl windows` reports the workspace of a floating window, and the new
  `wiri-ctl action <name> [args]` (IPC `run_action`) runs any bindable action through the same code
  as its hotkey, so scripts and status bars can drive wiri.
- **A minimized window leaves no gap.**  wiri now listens to `EVENT_SYSTEM_MINIMIZESTART` and
  `EVENT_SYSTEM_MINIMIZEEND`.  A window that is minimized leaves its column, the others close up and
  the view keeps the focused column visible; when the window is restored it goes back where it sat:
  into the column it shared with other windows at its old place, or into a column of its own, at its
  old position and with its old width.  The place is found again by neighbour, not by index, so
  windows that came and went in the meantime do not misplace it, and a whole desktop minimized at
  once (Win+D) comes back in its old order whichever window is restored first.  Restored after the
  user switched to another workspace, the window comes back on the workspace they are looking at.
  A window that was fullscreen or floating keeps that state, and a window closed while minimized is
  forgotten.  `wiri-ctl focus-window` restores a minimized window; `wiri-ctl windows` reports
  `minimized` and the new `column` / `tile` position of every window.
- **Floating windows belong to their workspace.**  A floating window is hidden when its workspace is
  left and shown again, where it was, when the user comes back (it used to stay on screen on every
  workspace), `move-window-to-workspace` / `Mod+Shift+N` move a float with its membership, and
  `wiri-ctl windows` / `focus-window` know which workspace a float lives on (`focus-window` switches
  to it).  Floats stay above the tiles: they are raised again whenever a tile takes the focus or a
  workspace is entered (a tile that was just clicked used to cover the float), and a workspace that
  holds only floating windows is no longer pruned as empty.  A window floats where it floated last
  time.
- **niri's column keymap.**  `consume-or-expel-window-left` / `-right` (a window alone in its column
  joins the column on that side at the bottom, one of several leaves its column to that side),
  `consume-window-into-column` (pulls the first window of the column on the right under the focused
  one: it used to push the focused window into the right neighbour) and `expel-window-from-column`
  (the bottom window leaves, it used to be the focused one), `move-window-up` / `move-window-down`
  (swap inside the column, the window keeps the focus), `focus-column-first` / `focus-column-last`,
  `move-column-to-first` / `move-column-to-last`, and the niri names `focus-window-up` /
  `focus-window-down`.  `maximize-column` now makes the column as wide as the view, also for a
  column of one window, and toggling gives the old width back; an explicit width ends it, a maximized
  column follows the view, and a window expelled from it stays maximized.  The new chords:
  `Ctrl+Alt+Home` / `End`, `Ctrl+Alt+Shift+Home` / `End`, `Ctrl+Alt+Shift+[` / `]`, `Ctrl+Alt+K` /
  `J`.  A floating window has no column: the column actions leave it alone and `move-window-up` /
  `down` nudge it by 50 px.
- **niri's action names and argument grammar.**  A niri `binds {}` block now ports: `set-column-width`
  and `set-window-width` take niri's size argument (`"50%"`, `"+10%"`, `"-10%"`, `"1200"`, `"+100"`; wiri's
  `"1/3"`, `"full"`, `"cycle"` still work), `set-window-height` and `reset-window-height` change a
  window's share of its column, `focus-workspace 2` is the second workspace while `focus-workspace "2"`
  is the workspace named 2, and the niri names are accepted for what wiri already does:
  `fullscreen-window`, `toggle-window-floating`, `move-window-to-floating` / `-tiling` (not toggles),
  `toggle-overview` / `open-overview` / `close-overview`, `toggle-column-tabbed-display`,
  `switch-preset-column-width` (and `-back`), `expand-column-to-available-width`,
  `focus-window-previous`, `focus-workspace-up` / `-down`, `focus-monitor-left` / `-right`,
  `move-window-to-monitor-left` / `-right`, `screenshot-screen`, `set-workspace-name`,
  `show-hotkey-overlay`.  A proportion is niri's: two `"50%"` columns and the gap between them fill the
  view exactly (the old presets were a gap too wide, so three thirds spilled off the screen); a fixed
  size is the window's, the border comes on top.  `switch-preset-column-width` is niri's cycle (a third,
  a half, two thirds, the first one wider than the column, going on from the last preset it set) instead
  of the old quarter-to-full list.  `expand-column-to-available-width` counts only the columns that are
  fully in view like niri (it subtracted the whole strip, which came out as 50 px on a long one) and
  maximizes a column that is alone in view.  `wiri-ctl set-column-width` and the IPC request take the
  same words.  A bind whose action is known but whose arguments are not now says so
  (`unknown action or bad arguments`).
- **Workspaces are addressed by position, like in niri.**  `focus-workspace 2`, `Ctrl+Alt+2`,
  `move-to-workspace 2` and the numbers `wiri-ctl` and the status displays show are the position of the
  workspace from the top of its monitor, not an id: once an empty workspace in the middle has gone (it
  now goes as soon as you leave it) the second workspace used to be unreachable, and pressing
  `Ctrl+Alt+2` made an empty one up.  A number past the last workspace is the last one (the empty
  workspace at the bottom); nothing is made up.  `focus-workspace-up` / `-down` (PageUp / PageDown) no
  longer wrap around, `focus-workspace-previous` is niri's (the workspace you were on before; wiri's
  old meaning of the name is `focus-workspace-up`), and `input { workspace-auto-back-and-forth }`
  makes asking for the workspace you are on go back to the previous one.  Named workspaces
  (`workspace "mail"`, with or without a block) exist from the start on every monitor, in the order
  they are declared, and are not removed when they are empty or left.  `move-workspace-up` / `-down`
  take the floating and minimized windows of the workspace and (with one monitor) its name along, and
  `move-column-to-workspace-up` / `-down` go to the next workspace that exists instead of an id that
  may not.
- **Windows Terminal no longer lies on top of its neighbour.**  wiri now asks every window how narrow
  it can be: the `WM_GETMINMAXINFO` question the system itself puts to a window before each resize
  (asked once when the window is added, outside the engine lock, with a 120 ms timeout that gives up
  at once on a hung or higher-integrity window, and twice more at most when the window had no time to
  answer and takes the focus).  A column is never narrower than its windows can be: in proportional
  mode the other columns share what is left, in fixed mode the wide one just takes its width and the
  strip moves over.  Three terminals on a 1366 px screen used to be asked for 440 px each, refused
  (Windows Terminal will not go below about 462 px) and overlapped.  A width the user asked for that
  is below the minimum is kept, and comes back when the limit goes.
- **Fullscreen is exactly the monitor.**  A fullscreen window used to be 7 px larger than the monitor
  on three sides: the invisible window border measured while it still had a frame was added to a
  window that no longer has one.  It is placed exactly now, and its border is measured again when
  the frame comes back.
- **`Ctrl+Alt+Tab` is no longer a default.**  It is Windows' task switcher, so registering it failed
  with a warning at every start; `focus-previous` (niri's `Mod+Tab`) is ``Ctrl+Alt+` `` (the key
  above Tab) by default.  A test keeps every default chord off the ones Windows reserves and off
  each other.
- **After a crash, `quit` puts the windows back where they were.**  `hidden.json` now also records
  the rectangle every managed window had before wiri first moved it.  A restarted wiri takes that
  record over for the windows that are still there (the same handle, process and class), so a
  `quit` after a kill or a crash restores the real places instead of piling the windows that the dead
  wiri had parked below the screen into the middle of it (what is not known is still rescued, now
  cascaded 32 px apart instead of stacked).  The record is written once for the startup pass, once
  per new or closed window, and deleted by a graceful quit.
- **A monitor that is unplugged keeps its windows.**  `unregister_monitor` used to throw the monitor
  and its workspaces away: its windows stayed in `tiled_windows`, so nothing ever tiled them again
  and they were orphaned until wiri was restarted.  Now the workspaces that hold something move to the
  monitor that is left (niri does the same): below its own workspaces, a named one joining the
  workspace of the same name, floating and minimized windows with them, empty ones dropped.  When the
  monitor comes back (or another monitor appears after the last one went) the windows that are still
  where they were put go back into the same columns with the same widths; a window that was closed or
  moved by the user meanwhile is left alone.  The display-change handler registers the monitors that
  are there before it removes the ones that went.  **Tested with engine unit tests only: the laptop
  these changes are verified on has a single display, so no real unplug has been tried.**
- **`layout { hide-method "cloak" }` (optional, off by default).**  The windows of the other
  workspaces are hidden with `SW_HIDE`, which takes them out of the taskbar and out of Alt+Tab.  With
  `hide-method "cloak"` the shell cloaks them instead (the way komorebi does, through the
  undocumented `IApplicationView::SetCloak`): they stay visible to the system, keep their taskbar
  button and their place in Alt+Tab, and are not drawn.  Alt+Tab (or a taskbar click) to a cloaked
  window brings its workspace along, once the window has been hidden for a moment (a report right after
  the hide is still Windows picking a replacement).  The shell interfaces are private and change
  between builds of Windows: the view of a window is asked for by the IID wiri knows before its vtable
  is used, the first refusal turns cloaking off for the run and the window is hidden the plain way.
  `hidden.json` marks cloaked windows, so a wiri that is killed while windows are cloaked is
  followed by one that uncloaks them (also a quit and a panic), and a window that is visible to the
  system but cloaked is restored.  The default stays `hide`.
- **Quit leaves the windows shown.**  A layout pass that ran between the restore and the end of the
  process (the 750 ms reconciler and the IPC server keep running until then) hid the windows of the
  other workspaces once more, so a quit could leave them hidden (with `hide-method "cloak"`, whose
  restore takes a few milliseconds per window, the live test hit it every time).  After the restore
  the engine lays nothing out any more.  A restart that shows the windows a killed wiri cloaked now
  also waits until DWM has dropped the cloak before it enumerates the desktop (a cloaked window is
  not tiled), and a window the shell would not uncloak is tried again at the next show.

### Fixed — live-desktop shakedown (2026-10-24)
First long run on a real desktop (Windows 11, 1366x768, Windows Terminal
windows, one hung application) turned up nine defects.  Each is fixed with unit
tests and was re-checked on that desktop.
- **Repositioning loop (~68 log lines a second).**  Windows nested inside
  another window (the XAML island in every Windows Terminal window, WebView
  hosts) were tiled as columns of their own, and the host and the island passed
  the foreground back and forth.  Child windows, wiri's own windows and a
  console that only wiri is attached to are never tiled
  (`window::classify_skip`).
- **`wiri-ctl` starved for minutes behind a hung application.**  The layout
  pass made synchronous cross-process calls (`DeferWindowPos`, `RedrawWindow`
  with `RDW_UPDATENOW`, `ShowWindow`) while holding the engine lock.
  Positioning is now one asynchronous `SetWindowPos` per window, redraw only
  invalidates, show and hide use `ShowWindowAsync`, and every IPC request is
  answered within 1.5 s ("wiri is busy" instead of a hung pipe).
- **New windows were never tiled.**  Columns outside the visible strip were
  skipped by the layout; they are parked below the screen instead, and a new
  column is scrolled into view by the minimal amount.  A window that closes on
  an inactive workspace no longer leaves a ghost tile.
- **Foreground fight.**  The OS foreground is mirrored into the focus state
  instead of being activated again.  Activation is attempted once per focus
  target, escalates (`SetForegroundWindow`, an injected F24 key,
  `AttachThreadInput`, `SwitchToThisWindow`) and a refusal is logged once.  The
  focused tile's inset no longer differs, so a focus change cannot resize
  anything.
- **Key names.**  `Bracket_Left`, `Bracket_Right`, `Backslash`, `Comma`,
  `Period`, `Page_Up` and `Page_Down` (the xkb/niri spellings; case, `_`, `-`
  and spaces are ignored) are accepted, so the default config and both examples
  load without unknown-key warnings.  `set-column-width` is an action.  In
  `focus-ring {}` and `shadows {}` the keys `width` and `color` no longer
  overwrite the border settings.
- **`wiri.exe` is a GUI-subsystem program** (release builds): it opens no
  console window that wiri could tile.  The log is
  `%LOCALAPPDATA%\wiri\wiri.log`, rotated at 4 MB with one backup
  (`wiri.log.1`).  `-v`, `--help`, `--version` and command-line errors attach
  to the console of the shell that started wiri.
- **`wiri-ctl` output** is folded to ASCII when stdout or stderr is a pipe or a
  file (the box-drawing characters used to arrive garbled); a real console
  keeps the Unicode tables.  `WIRI_ASCII=1` / `WIRI_UNICODE=1` override it and
  `--json` is untouched.
- **238 px columns on a 1366 px display.**  In proportional mode every window
  got an equal share of the screen, down to `column-width / 2`.  The new
  layout option `max-visible-columns` (default 3) bounds the split: one window
  fills the screen, two get halves, three get thirds, further windows scroll
  in at a third.  Explicit widths (`set-column-width`, grow/shrink) now apply
  in proportional mode too.
- **Workspace numbers are 1-based** at every user-visible edge (hotkeys,
  `switch-workspace`, IPC replies, events).  `Ctrl+Alt+1` and
  `switch-workspace 1` used to select the second workspace.
- **Crash safety.**  Every hidden window is recorded in
  `%LOCALAPPDATA%\wiri\hidden.json`; the next start, `wiri-ctl quit` and a
  panic show them again, and a window an earlier killed wiri left off-screen
  is moved onto the work area.

Second pass, same day, with real key presses and a real mouse on the same desktop:
- **Escape did not work in any other application while wiri ran.**  The shipped
  config binds `Escape` to the overview toggle and wiri registered it with
  `RegisterHotKey`, which delivers the key to wiri and to nobody else.  It is no
  longer registered; the low-level keyboard hook leaves overview and resize mode
  and passes the key on.
- **Leaving overview with Escape froze wiri** (every `wiri-ctl` call timed out; the
  original build does the same).  The DWM-thumbnail host window belonged to
  whichever thread entered overview and was hidden from whichever thread left, a
  cross-thread call that waits for the owner while the engine lock is held.  The
  host window now lives on a thread of its own.  It was also fully transparent, so
  the thumbnails were invisible; it is now a dark translucent backdrop.
- **Leaving fullscreen** left the window as big as the screen (the fullscreen
  rect bypassed the applied-state cache).
- **`move-column-left` / `move-column-right`** merged the column into its
  neighbour instead of swapping the two.
- **`toggle-floating`** acts on the floating window that has the focus, and the
  window just floated keeps the focus; a floated window can be put back with the
  keyboard.
- **`Ctrl+Alt+2`** could be undone at once by the foreground report of a window
  wiri had just hidden.
- `wiri-ctl get-focus` prints the focused window.

### Added — opus pass K (2026-05-18)
- ✅ **DwmRegisterThumbnail-based overview** — niri-parity overview rendering
  that leaves the underlying HWNDs at their normal size and instead
  composites live, accurately-scaled DWM thumbnails onto a fullscreen
  transparent topmost host window.  Replaces the previous behaviour of
  resizing every tracked HWND to a tiny rect on overview enter (which
  caused apps to render incorrectly at 100 × 80 px and fight subsequent
  resizes).
  - New `src/overlay/thumbnail_overview.rs` module exposing
    `ThumbnailOverviewSink` (trait), `DwmThumbnailOverview` (Win32
    impl) and `MockThumbnailOverview` (test double).
  - `TilingEngine::install_thumbnail_overview(Arc<dyn ThumbnailOverviewSink>)`
    opts the engine into the new rendering path; without it the engine
    falls back to the historic SetWindowPos overview so unit tests and
    headless smoke paths keep working.
  - `enter_overview` calls `sink.enter()` and registers a thumbnail for
    every visible tile across every workspace on the focused monitor;
    `exit_overview` unregisters everything and destroys the host
    window; the overview branch of `apply_layout_for_monitor` routes
    each tile's rect to `sink.update_thumbnail(window_id, rect)`
    instead of `SetWindowPos`.
  - Four new tests cover registration, update-rect propagation,
    teardown, and the fallback path when no sink is installed.
- ✅ **Wallpaper-aware accent for focused borders** — niri's
  `wallpaper-aware accent` parity.  New `LayoutConfig.border_color_focused_mode:
  BorderColorMode` enum (`Fixed` / `WindowsAccent`).  KDL surface:
  `layout { border-color-focused "accent" }` (or `"windows-accent"`)
  flips the mode; any other value parses as the historic `#rrggbb`
  literal.  The new `crate::backend::accent` module reads
  `HKCU\Software\Microsoft\Windows\DWM\AccentColor` (REG_DWORD packed as
  `0xAABBGGRR`), unpacks it via `unpack_abgr`, and caches the result for
  30 s.  `apply_layout_for_monitor` resolves the focused-border colour
  per pass — a successful accent fetch is used verbatim; a failed fetch
  falls back to the literal in `border_color_focused` so the user
  always gets a visible focus indicator.  Three new tests cover parser
  acceptance, cached lookup, and the failing-reader fallback.

### Added — opus pass J (2026-05-18)
- **Move column vertically between workspaces (niri parity)** — new
  `Action::MoveColumnToWorkspaceUp` / `Action::MoveColumnToWorkspaceDown` plus
  engine helper `move_focused_column_to_workspace(delta, backend)` lift every
  tile in the focused column out of the current workspace and append them as
  a single new column to `current_workspace_id + delta`.  Destination
  workspace is created on demand; focus follows the moved column.  Default
  binds: `Ctrl+Alt+Shift+Up` / `Ctrl+Alt+Shift+Down`.  IPC:
  `MoveColumnToWorkspaceUp` / `MoveColumnToWorkspaceDown` +
  `wiri-ctl move-column-up` / `move-column-down`.
- **Workspace move/swap (niri parity)** — new
  `Action::MoveWorkspaceUp` / `Action::MoveWorkspaceDown` plus engine helper
  `move_active_workspace(delta, backend)` swap the focused workspace with the
  workspace immediately above / below it on the same monitor.  Focus follows
  the moved workspace.  Default binds: `Ctrl+Alt+Shift+Page_Up` /
  `Ctrl+Alt+Shift+Page_Down` (relocated from `move-column-to-monitor-*`,
  which moved to `Ctrl+Alt+Shift+Comma` / `Period` to match niri).  IPC +
  ctl wired (`MoveWorkspaceUp` / `Down`, `move-workspace-up` /
  `move-workspace-down`).
- **Smart borders** — niri's `border { disable-when-only-one-window true }`
  parity.  New `LayoutConfig.smart_borders: bool` (default `false`); when
  enabled and the active workspace contains exactly one column with exactly
  one tile, the per-tile DWM border colour is forced to the
  `DWMWA_COLOR_DEFAULT` (`0xFFFFFFFF`) sentinel so the tile renders without
  a wiri-painted outline.  KDL surface: flat `smart-borders true`, hyphen
  variants, and nested `borders { smart true }`.  Honours per-output
  `output { layout { smart-borders true } }` overrides.
- **Blur backdrop for floating windows** — niri's `blur { … }` parity via
  `DwmEnableBlurBehindWindow`.  New `ResolvedWindowRules.blur: bool` (was
  previously misused as a "borderless proxy" — the legacy `resolved.border =
  !rule.blur` line is replaced with a true `resolved.blur = rule.blur`
  surface).  Engine helper `apply_window_blur(hwnd, enabled)` tracks state
  in `blur_applied: HashSet<WindowId>` so DWM is called at most once per
  HWND lifetime; entries are dropped in `remove_window` so HWND reuse
  re-applies.  Wired into every `add_window_with_target` branch.
- **System-critical hotkey passthrough guard** — new
  `SYSTEM_CRITICAL_HOTKEYS` denylist + `is_system_critical_hotkey` helper.
  `register_hotkeys` skips `Win+L` (lock screen), `Win+Shift+S` (snipping
  tool), and `Win+D` (show desktop) before ever calling `RegisterHotKey`,
  with a WARN-level log entry so the audit trail explains the skip.  Even
  if a user binds one of these chords in `config.kdl`, the daemon refuses
  to register it.

### Added — opus pass I (2026-05-18)
- **Workspace-slide animation engine wiring** — `switch_workspace` now calls
  `AnimationManager::start_workspace_slide(...)` with `+work_area_height` (or
  `-work_area_height` when stepping to a lower workspace) and
  `apply_layout_for_monitor` adds the per-monitor `workspace_slide_offset(oid)`
  to every tile's Y position.  Honours `animations.enabled` and
  `animations.workspace_transition` — disabled = snap, no slide.
- **Per-output `layout {}` engine wiring** — new `TilingEngine.config_per_monitor:
  HashMap<OutputId, LayoutConfig>` populated from every
  `output { layout { … } }` block at `set_full_config` time, plus
  `effective_config(output_id) -> &LayoutConfig` accessor that falls through to
  the global default.  `apply_layout_for_monitor` + `calculate_positions_in_section`
  read through the accessor so per-monitor column-width / border / gap / mode
  overrides take effect on multi-monitor setups.
- **`Action::SpawnCmd(String)`** — new dispatcher arm for `spawn-cmd "<command>"`
  binds.  Routes through `cmd.exe /C "<command>"` so cmd-builtins, env-var
  expansion, and PowerShell one-liners work as the user typed them.
  `HotkeyBinding::parse_action` now returns `SpawnCmd` for the `spawn-cmd` /
  `spawn-sh` aliases.
- **`binds { extend-defaults true | false }` directive (niri parity)** — default
  `true`.  When extending, config binds merge with the shipped defaults; config
  wins on `(mods, vk)` collision, non-colliding config binds are appended.
  Startup logs `Hotkeys: N from defaults, M from config (X overridden)`.
  Fixes the long-standing "new shipped actions disappear after editing config".
- **Engine-level snapshot/restore + `wiri-ctl save-snapshot/load-snapshot/list-snapshots/delete-snapshot`** —
  new `src/layout/snapshot.rs` module serialises monitor → workspace → column →
  tile structure to JSON at `%APPDATA%\wiri\snapshots\<name>.json` (override
  with `WIRI_SNAPSHOT_DIR`).  On restore, HWNDs that no longer exist are
  skipped silently so yesterday's snapshot still loads today.  Snapshot name
  is sanitised to ASCII alphanumerics + `-_.` so `..\\..\\evil` cannot escape
  the dir.  New IPC messages: `SaveSnapshot`, `LoadSnapshot`, `ListSnapshots`,
  `DeleteSnapshot`.
- **Interactive resize mode (niri-`Mod+R` parity)** — `Action::EnterResizeMode`
  toggles `TilingEngine.resize_mode`; while engaged the WM_HOTKEY dispatcher
  re-routes the `Mod+Arrow` chords to grow/shrink the focused column / tile by
  5% and `Esc` exits (Esc is registered globally + state-gated on
  `is_overview() || is_resize_mode()`).  Default chord moves: `Mod+R` becomes
  EnterResizeMode, CenterColumn moves to `Mod+Shift+R`.  New
  `wiri-ctl resize-mode` IPC + ctl subcommand backed by `IpcMessage::ToggleResizeMode`.

### Added
- **Niri-style multi-workspace overview** — `enter_overview` now collects ALL
  non-empty workspaces of the focused monitor, stacks them vertically with a
  50 px gap, and picks a single zoom that fits both width and height. New
  `OverviewState { zoom, workspace_offsets }` replaces the old `Option<f64>`;
  `overview_zoom()` and `overview_state()` accessors retain backwards compat.
  `exit_overview` now hides every tile that doesn't belong to the active
  workspace so the user is back to a single-workspace view.
- **Overview banner overlay** (`src/overlay/overview_banner.rs`) — translucent
  "Overview — Space or Esc to exit" strip pinned at the top of the focused
  monitor while overview is active.  Wired via a process-wide singleton so
  `engine.rs::enter_overview` / `exit_overview` can toggle it without
  threading state through.
- **GPU-hotkey-conflict detector** (`src/backend/hotkey_conflicts.rs`) —
  startup probe (x86_64 only) that logs a WARN when Intel HotKey Service or
  AMD Radeon registry keys are present, since those drivers steal
  `Ctrl+Alt+Arrows` for screen rotation.  No-op on aarch64.
- **`wiri-ctl test-bindings`** — diagnostic subcommand that prints every
  hotkey binding the running daemon successfully registered with Windows
  (chord, modifier mask, VK code, action).  Backed by new
  `IpcMessage::ListBindings` + `ListBindingsResponse { bindings: Vec<BindingInfo> }`
  + `MessageLoop::current_bindings()` helper.
- **State-gated `Escape` overview-exit binding** — registered unconditionally
  so the OS routes Esc to wiri while overview is active, but the WM_HOTKEY
  dispatcher swallows it (no action) when overview is off, keeping Esc
  available to other applications in normal mode.
- TROUBLESHOOTING.md sections: "Known hotkey conflicts on Windows" + "How to
  verify wiri received your keypress".

### Changed
- **Default overview chord moved from `Ctrl+Alt+Tab` to `Ctrl+Alt+Space`**
  (plus `Escape` to exit).  Windows' accessibility task switcher reserves
  `Ctrl+Alt+Tab` system-wide — `RegisterHotKey` succeeds but the OS still
  intercepts the keystroke, so the binding silently never fires.  Default
  fixed in both `resources/default_config.kdl` and the in-code defaults in
  `backend/message_loop.rs::default_hotkeys`.

- `wiri-ctl validate-config <path>` — parse and validate a config file without
  needing a running daemon
- Live `MatcherContext` flags (`is_active`, `is_floating`, `is_urgent`,
  `at_startup`) now populated from real engine state when resolving window rules
- DWM drop-shadow toggle via `LayoutConfig.shadow_enable` (uses the canonical
  1-px-top `DwmExtendFrameIntoClientArea` trick; applied only on first tile to
  avoid the Windows Terminal blank-paint regression)
- Two-finger pinch gesture coalescing in `input/touch.rs` (frame-coalesced
  WM_POINTER updates emit `Gesture::Pinch { scale }` when distance changes >8 px)
- **KDL `include "…"` directive (niri parity)** — pre-processed before parsing;
  paths resolve relative to the including file or absolute; depth capped at 8;
  cycles detected.  New API: `Config::load_from_str_with_base(input, base_dir)`
  in `src/config/loader.rs`
- **Per-output `layout { … }` override block (niri parity)** — `output "DP-1"
  { layout { column-width 800 } }` populates `OutputConfig.layout_override:
  Option<LayoutConfigPartial>`.  `LayoutConfigPartial::apply_to(&base)` folds
  Some-fields onto a base `LayoutConfig` to produce the effective per-monitor
  config
- **niri-parity `open-*` window-rule properties** — `open-on-output`,
  `open-in-workspace`, `open-fullscreen`, `open-floating`, `open-max-bounds`
  (accepts `WxH`, `W H`, `W,H`).  Surfaced on `ResolvedWindowRules`; resolver
  in `src/window/rules.rs` folds them with last-rule-wins semantics
- **Workspace slide animation primitive** —
  `AnimationTarget::WorkspaceX(OutputId)` plus
  `AnimationManager::start_workspace_slide(output, from_px, to_px, duration_ms)`
  and `workspace_slide_offset(output)` reader.  Sign convention matches niri:
  positive = slide in from the right, negative = from the left
- **`spawn-cmd "<command>"` bind (niri-`spawn-sh` parity)** — recognised by
  `HotkeyBinding::is_spawn_cmd()`; `shell_command_argv()` returns
  `["cmd.exe", "/C", "<command>"]` for the dispatcher
- **Output `scale` validation** — values outside `0.5..=4.0` log a warning but
  are still stored (warn-and-keep, matches niri)
- `MAX_INCLUDE_DEPTH` constant (`8`) exposed in `src/config/loader.rs`
- Default config (`resources/default_config.kdl`) documents the new `include`,
  `spawn-cmd`, and per-output `layout {}` blocks with commented examples

### Requests to Agent E (engine wiring not in this pass)
- **`OutputConfig.layout_override` is parsed but not yet applied.** The
  engine currently holds one global `LayoutConfig`.  Wire per-monitor
  effective configs by folding `output.layout_override.as_ref().map(|p|
  p.apply_to(&global))` onto a `HashMap<OutputId, LayoutConfig>` (or store
  the partial on `Monitor` and merge at the layout-calculation site)
- **`AnimationTarget::WorkspaceX` needs a reader in
  `apply_layout_for_monitor`.** During a workspace-switch, call
  `engine.animation.workspace_slide_offset(oid)` and add the f64 to every
  column's X position on the active workspace.  Engine should call
  `start_workspace_slide()` in `switch_workspace(...)` when
  `config.animations.workspace_transition` is true
- **`ResolvedWindowRules.open_on_output / open_in_workspace /
  open_fullscreen / open_max_bounds` are populated but not consumed.** Wire
  them in `engine.rs::add_window` (or the matching `add_window_with_target`
  path): route by `open_on_output` → matching `OutputId`; pick workspace by
  `open_in_workspace`; cap initial bounds by `open_max_bounds`; flip
  fullscreen state when `open_fullscreen == Some(true)`
- **`HotkeyBinding::is_spawn_cmd() / shell_command_argv()` need a
  dispatcher arm** in `backend/message_loop.rs::WM_HOTKEY` handler.  The
  existing `Action::Spawn(cmd)` path can be extended with a sibling
  `Action::SpawnCmd(cmd)` variant in `input/mod.rs`; route it to
  `CreateProcessW` with the argv from `shell_command_argv()`

### Changed
- `build.rs` watches `.git/logs/HEAD` so `--version` git hash refreshes on
  every commit (previously stale until `cargo clean`)
- `Config::load_sync_inner` (in `loader.rs`) now resolves the file's parent
  as the include base directory and routes through `load_from_str_with_base`,
  so on-disk configs gain `include` support automatically

## [0.1.0] — 2026-05-17

Initial public release. See `docs/PROGRESS.md` for the full feature matrix.

### Added
- Scrollable-tiling layout (columns on an infinite horizontal strip)
- Multi-monitor with per-monitor dynamic workspaces
- KDL configuration with `notify`-based hot-reload and 300 ms debounce
- 22 default key bindings (Ctrl+Alt prefix), all rebindable via `binds { … }`
- Interactive move (Alt + left-drag) and edge-resize (Alt + right-drag) via
  `WH_MOUSE_LL` global hook
- Per-monitor DPI awareness (`PER_MONITOR_AWARE_V2`)
- DWM border colour focus rendering on Windows 11+
- Named-pipe IPC at `\\.\pipe\wiri_control` with ACL restricted to the
  current user + Administrators
- `wiri-ctl` companion CLI with 27 subcommands, human-formatted output, and a
  global `--json` opt-out for scripting
- Tray icon with menu: Show/Hide, Focus Last Tile, Open Config Folder,
  Reload Config, About, Take Screenshot, Quit
- Workspace switcher overlay (multi-monitor aware, 100 ms ease-in fade,
  Win11 DWM rounded corners)
- Animation framework (tick-based, 6 easing functions, auto-driven at ~60 fps
  whenever animations are active)
- Window rules with `class`, `title`, `instance`, `pid` matchers; regex via
  `match { regex … }` blocks
- Per-rule properties: `float`, `workspace`, `opacity` (now `Option<f64>`,
  so explicit `opacity 0` is honoured), `blur`, `sticky`, `scale`
- Urgent-window detection via `EVENT_SYSTEM_ALERT` WinEvent hook
- Auto-engage overview zoom when column count exceeds `auto_tile_threshold`
- Position-failure circuit breaker (3 strikes → auto-float; prevents runaway
  `SetWindowPos` retries against UAC and anti-cheat windows)
- Configurable terminal spawn (Mod+Enter; configurable via the `spawn` action
  argument, with `cmd.exe` as final fallback)
- Mouse-wheel workspace scrolling when modifier is held (respects
  `natural_scroll` and `scroll_speed` from config)
- Monitor cycling via the `switch-monitor` action (sorted by X position)
- Screenshot capture to `%USERPROFILE%\Pictures\wiri-<unix>.bmp` (falls back
  to the current working directory), bound to `Ctrl+Alt+P`
- `wiri-ctl list-monitors` for diagnosing multi-monitor / HiDPI configuration
- ARM64-safe hotkey registration: software key-repeat debounce at 125 ms
  instead of `MOD_NOREPEAT` (which fails on aarch64-pc-windows-msvc)
- 329 unit tests, all passing on `aarch64-pc-windows-msvc`

### Known limitations
- Fullscreen-exclusive games cannot be tiled (no compositor can)
- Some UWP apps expose limited HWND control
- A handful of anti-cheat shims dislike `WH_MOUSE_LL` — disable the mouse
  hook in those games

[Unreleased]: https://github.com/jolionlands/wiri/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/jolionlands/wiri/releases/tag/v0.1.0
