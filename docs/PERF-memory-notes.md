# Where the event-driven runtime's memory went: an attribution, not a leak hunt

Read-only analysis lane. Base commit `cc3920d` (= the head the `f1528c7` baseline measured,
one commit above `0ebdebf`). Nothing in `src/` was changed, no build was produced, and
`scripts/live-test.ps1` was **not** run. Every number below is either taken from
`docs/PERF.md`'s "Live baseline" (the `af3eb30` → `cc3920d` delta table), from the git
history, or is arithmetic on the source in this tree. Where a claim is arithmetic rather
than measured it says so.

The baseline records the growth and refuses to explain it:

| Endpoint | `af3eb30` | `cc3920d` | Change |
| --- | ---: | ---: | ---: |
| Private bytes, startup | 3,862,528 | 8,511,488 | +4,648,960 (+120.36%) |
| Private bytes, after actions | 4,521,984 | 8,523,776 | +4,001,792 (+88.50%) |
| Working set, startup | 14,716,928 | 25,145,344 | +10,428,416 (+70.86%) |
| Working set, after actions | 18,432,000 | 27,037,696 | +8,605,696 (+46.69%) |
| `wiri.exe` file | 6,698,496 | 8,965,120 | +2,266,624 (+33.84%) |
| Threads, startup / after | 24 / 26 | 14 / 15 | −10 / −11 |

## What the delta table already settles

Four observations narrow the search before any code is read.

1. **The growth is not per-event.** `p_after_private_bytes - p_startup_private_bytes` is
   **12,288 bytes — exactly three 4 KiB pages** on the new build, against 659,456 bytes on
   the old one. Over an activity window that included ten hotkey placements, four IPC
   actions, a screenshot and a subscriber, private commit moved by three pages. There is no
   per-action, per-event or per-window admission slope in this record. Whatever costs the
   +4.65 MiB was already committed ~5 s after process start, before the actions ran.
   **The per-activity growth looks flat; the growth is a startup-time structure.**
2. **The old build had the per-activity growth, not the new one.** 659,456 bytes appeared
   between the old endpoints and only 12,288 between the new ones. The old build's lazy
   growth is what its doc called "lazy resources and Tokio blocking workers"
   (8 default workers, blocking pool growing to 512 threads with a 10 s keep-alive). The
   bounded runtime removed it.
3. **Threads fell, so the runtime stacks are a saving, not a cost.** 24/26 → 14/15 threads.
   `a588b96` is on the negative side of this ledger: `WORKER_THREADS = 2`,
   `MAX_BLOCKING_THREADS = 4`, `BLOCKING_KEEP_ALIVE = 2 s` (`src/runtime.rs:17,22,26`) replace
   8 workers and an up-to-512-thread blocking pool. Its own commit message claims that as a
   private-bytes reduction; the thread counts in the table agree.
4. **The two builds are 179 commits apart, not five.** `git rev-list --count af3eb30..cc3920d`
   is **179**; the perf pass is the last 5 of them (`a588b96`, `79f9ee5`, `93286d9`,
   `2c53631`, `0ebdebf`). The delta table is therefore an *integration* delta, not a
   perf-pass delta, and the baseline says so only implicitly.

## Attribution table

Costs are for the measured machine: one 1366×768 display, work area `1366×728`
(1,994,624 px), 96 DPI, shipped default config (`border { width 4 }`, `focus-ring { width 3 }`,
`shadow off`, `gaps inner 16 / outer 8`, `max-visible-columns 3` —
`resources/default_config.kdl:60-85`).

| Component | Expected cost | Bounded? | Evidence |
| --- | --- | --- | --- |
| **Decoration overlay surfaces** (new since `af3eb30`; one layered window per visible decoration, each retaining a full-tile-size 32-bit DIB) | **≈3.7–4.2 MiB private** = one screen of ARGB pixels (1366×728×4 = 3.79 MiB), plus tab strips and spec clones. Two columns: 677×722×4 = 1,864 KiB each → 3.64 MiB. Three columns: 449×722×4 = 1,240 KiB each → 3.64 MiB. Measured delta is 4.43 MiB = 1.17 screen-frames | **BOUNDED** — per decoration only, and only for tiles that intersect the monitor; each surface capped at 16,777,216 px (64 MiB); destroyed with its overlay | `src/overlay/decor/layer.rs:459-486` (DIB kept in `Overlay` and reused), `:503` (realloc only when the size changes), `:354` (cap), `:409-414` (`DeleteObject`), `:600-605` (`DestroyWindow`), `:690-706` (windows not in the wanted set are removed + dropped); `src/layout/engine/decor.rs:75` (off-screen tiles are not decorated), `:98` (`bounds()` = tile + `extent()`); `src/layout/engine.rs:3383` (one `refresh_decorations` per layout pass); module absent at `af3eb30` (`git ls-tree af3eb30 src/overlay` lists no `decor/`) | 
| **Bounded tokio runtime** (`a588b96`) | **negative**: −8 worker stacks/run queues/TLS blocks, blocking pool 512 → 4, keep-alive 10 s → 2 s | BOUNDED (constants) | `src/runtime.rs:17,22,26`; threads 24/26 → 14/15 in the delta table |
| **Binary / image growth** | +2,266,624 B file (+33.84%); mapped read-only pages are *shareable*, only COW-touched data pages reach private bytes | BOUNDED (one-off) | `Cargo.toml` (`windows` features), `Cargo.lock` (`clap_complete` only), 179 commits / +55,536 net production lines — see the next section |
| **Backend event channel** (`tokio::sync::mpsc::unbounded`) | one heap node per queued event, freed on `recv`; the 256-event burst cap bounds *work per iteration*, not queue depth | **UNBOUNDED in the type**, effectively bounded by producer rate; no backlog sign here (idle CPU 0, +12 KiB private over the activity window) | `src/backend/mod.rs:405,527` (channel), `src/main.rs:638-670` (`EVENT_BURST = 256`, `recv().await` then `try_recv` drain) |
| **Reconcile schedule** (`2c53631`) | 16–24 bytes of state (`Option<Instant>` + `u32`); no timer at rest; one transient `Vec` per pass | BOUNDED | `src/backend/reconcile.rs:24-32` (`PASS_INTERVAL`, `QUIET_PASSES_TO_REST = 6`), `:53-85` (deadline/rest) |
| **Tray `Notify` + action queue** (`79f9ee5`) | one `Notify::const_new()` static (an `AtomicUsize` + an empty waiter list); a stored permit is a single wake-up; the std `mpsc` action queue is drained completely on every wake | BOUNDED | `src/hooks/tray.rs:31,34,41,81,94`; `src/main.rs:1011-1013` (`while let Some(action) = system.poll_tray_action()`) |
| **Workspace watch `Notify` + `LAST_WS` compare** (`79f9ee5`) | O(1): two `Notify` statics, an `Option<i32>`, a compare that runs only on wake | BOUNDED | `src/ipc/mod.rs:145,149,154-172`; `src/main.rs:855-861,927-931` |
| **Lazy workspace-indicator pump** (`0ebdebf`) | one thread + one window + one small DIB, on the first `show`, then reused — this is the +1 thread at the "after actions" endpoint | **EXPECTED-ONE-OFF**, then bounded | `src/overlay/workspace_indicator.rs:74,82,103,128-157,540`; `src/main.rs:686-688` (no spawn at startup) |
| **Engine per-window state** (`rule_state`, `applied_state`, decor specs, self-applied tracker) | proportional to managed windows; tracker prunes stale/`>256` entries; `LayoutConfig` is cloned per tile per layout pass (transient churn → allocator high-water, not a leak) | BOUNDED by window count | `src/backend/mod.rs:25,1049-1057,1481-1503`; `src/layout/engine/decor.rs:373-396` (`let mut per_window = cfg.clone()` per tile), `:222-230` (`merged` spec clone in `set_output`) |
| **Rust / std / dependency version** | **nil**: both baselines report rustc 1.95.0, and the only lockfile change in the gap is `clap_complete` (used by `wiri-ctl`) | RULED OUT | `docs/PERF.md` identity blocks; `git diff af3eb30 cc3920d -- Cargo.toml Cargo.lock` |
| **Screenshot / Windows.Graphics.Capture D3D11 device** | would be multi-MiB private, but is created per capture only, never at startup | RULED OUT for the startup endpoint | `src/screenshot/mod.rs:1` ("No persistent windows, codecs, devices, workers or clock"), `:83,125`; `src/screenshot/wgc.rs:205-215` (`create_device` inside `capture`) |
| **Working-set shareable part** (`WS − private`): 10.35 → 15.86 MiB, **+5.5 MiB** | ~2.27 MiB is the larger image mapped; the rest is not identified here | partially attributed | delta table; no module list was captured this run — **INVESTIGATE** |

### Why the decoration surfaces are the one candidate with the right magnitude

- The growth is present at the *startup* endpoint and grows by three pages during activity
  (observation 1). Decorations are created by the **first layout pass at startup** and are
  only reallocated when a tile changes size (`layer.rs:503`) — exactly that shape.
- The layer keeps one `Dib` per overlay for reuse (`Overlay { dib: Option<Dib> }`,
  `layer.rs:459-486`). A DIB section created over the pagefile is committed memory charged
  to the process: it counts in **private bytes**, not only in working set.
- The DIB is the size of the whole decoration, not of the 4 px border:
  `DecorSpec::bounds()` = tile expanded by `extent()` = `max(ring_width, shadow_reach) + 2`
  (`spec.rs:98`, `:84-96`), and the surface is created at that size (`layer.rs:498-503`).
  A full-height tile therefore costs one full-tile ARGB surface.
- Summed over a full row of columns the tile surfaces tile the screen, so the cost is
  **≈ one screen-frame of 32-bit pixels regardless of the column count**: 3.79 MiB for
  1366×728. The measured private delta is 4.43 MiB. The residual (≈0.6 MiB) is tab strips,
  the extra `Vec<DecorSpec>`/`LayoutConfig` copies, and the ordinary per-window growth of
  179 commits.
- It is *not* proportional to the 49 managed windows the old baseline recorded: tiles that
  do not intersect the monitor are skipped (`decor.rs:75`), and overlays whose decoration
  disappears are dropped (`layer.rs:690-706`). That is why a desktop with many parked
  windows does not cost dozens of surfaces.
- `af3eb30` had no decoration layer at all: `git ls-tree af3eb30 src/overlay` lists
  `bindings_cheatsheet.rs`, `mod.rs`, `overview_banner.rs`, `status_bar.rs`,
  `thumbnail_overview.rs`, `workspace_indicator.rs` — no `decor/`, no `compositor/`, no
  `recent_windows.rs`.

This is an attribution by mechanism + geometry, not a measurement. It is falsifiable with
the same binary and no code change; see "What a follow-up lane should measure".

## Binary-size breakdown (reasoned from code; `cargo bloat` not run)

No tool was installed, so this is a decomposition hypothesis, not a measurement.

The measured growth is `wiri.exe` +2,266,624 B (+33.84%) and `wiri-ctl.exe` +802,304 B
(+24.91%). Between the two measured commits:

- **179 commits, 159 `src/` files touched.** Production (non test-named) source:
  **+55,536 net lines**, of which **113 new production files totalling 45,321 lines**.
  Test-named files add 4,034 more changed lines that are `#[cfg(test)]` and cost no binary
  bytes. +2.27 MiB over +55.5k lines is ≈ **40 bytes of code+rodata per new line**, a normal
  ratio for release Rust with symbols.
- **`windows` crate features: 14 added** in the gap — `Win32_Graphics_Direct3D`,
  `Direct3D11`, `Dxgi_Common`, `Graphics_Capture`, `Win32_System_WinRT_Direct3D11`,
  `Win32_System_WinRT_Graphics_Capture`, `Graphics_Imaging`, `Win32_System_Com_StructuredStorage`,
  `DataExchange`, `Memory`, `Variant`, `Win32_Devices_Display`,
  `Win32_Devices_HumanInterfaceDevice`, `Win32_UI_Input`, `RemoteDesktop`,
  `WindowsProgramming`, `Foundation`. The `windows` crate emits generated bindings per
  active type/interface plus WinRT type metadata tables; each activation is linked code and
  `rdata` even when the path is only entered on demand. The screenshot/WGC path
  (`src/screenshot/mod.rs:8` `mod wgc;`) and the D3D11 imports in
  `src/screenshot/wgc.rs:16-32,205-215` are compiled in unconditionally, so that metadata is
  in the image whether or not a device is ever created.
- **`clap_complete` added** (`Cargo.toml`, `Cargo.lock`): a completion generator used by
  `wiri-ctl` (`src/bin/ctl.rs` +987/−203) — consistent with `wiri-ctl` growing by a quarter.
- **No size knobs to soften it:** `Cargo.toml` has no `[profile.release]` section, so the
  build is default release: `opt-level = 3`, `lto = false`, `codegen-units = 16`,
  `strip = "none"`. Every new generic instantiation therefore brings its own
  monomorphized body **and** its mangled symbol name in the symbol table, and each
  `codegen-unit` boundary re-emits what it borrows. "Static dispatch grew the binary" is a
  real mechanism here, but it is acting on 55k new lines and 14 new binding families, not on
  the 700 lines of the perf pass.
- The perf pass's own binary delta is small and mostly additive-free: `runtime.rs` 97 lines,
  `reconcile.rs` 220 lines, and edits to `main.rs` (+587/−341), `hooks/tray.rs` (+86),
  `ipc/mod.rs` (+27), `layout/engine.rs` (+50), `backend/message_loop.rs` (+15). Net ≈ +700
  lines, i.e. ~1.3% of the gap's production line growth.

A measured breakdown (`cargo bloat --release`, or `dumpbin /headers` section sizes for both
commits) needs a tool install and a build of `af3eb30`; both are outside this lane.

## Verdict

**Is +120% private bytes acceptable for a lightweight WM?** On absolute numbers, yes, with a
caveat and a fix.

- The endpoint is **8.12 MiB private / 23.98–25.79 MiB working set** at idle on a 22631
  desktop. That is still a small process: the growth is one screen's worth of ARGB pixels,
  not a runaway. Nothing in the record shows a slope — three pages of private commit across
  an activity window that included ten placements is the strongest "no leak" evidence this
  dataset can offer.
- The trade is worth it. Measured in the same window: idle CPU 437.5 ms → below the 15.625 ms
  counter increment, idle context switches 321.9/s → 9.16/s (−97%), hotkey median
  23.68 → 20.17 ms, first IPC reply 182.9 → 64.1 ms. Spending ~4 MiB of resident commit to
  remove ~40–60 timer wake-ups a second on an idle desktop is the right trade for a WM whose
  whole point is to be invisible when idle.
- The caveat is that **the mechanism is a design inefficiency, not a necessity**: a 4 px
  border and a 3 px focus ring are painted into a surface the size of the whole tile
  (`layer.rs:459-503`), because `UpdateLayeredWindow` takes one bitmap for one window. Only
  the band and the ring can change pixels (`paint.rs:1-10`: "the inside of a window is left
  alone"). A 9-slice or per-edge-band surface would cut the decoration cost from ≈ screen
  area per column-row (3.7 MiB) to tens of KiB, giving back essentially the whole delta
  without touching the idle wins.
- Two honest limits. First, the delta table is **not population-matched**: the `cc3920d`
  record does not state the managed-window count, the per-thread table, or GDI/USER objects
  (`docs/PERF.md`, "Not recorded by this run"). Decorations are bounded by *visible* tiles,
  so a different layout between the two runs moves this number; the geometry arithmetic
  above lands within 20% of the whole delta for any 1–3 column layout, which is why the
  attribution is credible, but it is not a proof. Second, ~5.5 MiB of the working-set delta
  is *shareable* resident memory and is only partly explained by the +2.27 MiB image; that
  part stays **INVESTIGATE**.

**Soak lane: recommended, but it is the second thing to do.** The first is a 30-minute
`border { off }` / `focus-ring { off }` A/B on the *same* binary, because it answers the
attribution question with no new tooling and no timing risk. The soak answers a different
question — "does anything grow with uptime?" — which this dataset cannot answer at all
(one interval, `p_*` are single observations, and the doc already says they are not leak-test
slopes).

### What a follow-up lane should measure

1. **A/B on decoration surfaces (decisive, cheapest).** Same binary, same desktop, same
   collector; run once with the shipped default config and once with
   `border { off }` + `focus-ring { off }`. Expect private bytes at the startup endpoint to
   fall by ≈3.6–4.0 MiB and GDI objects to fall by 2 per decoration. If private bytes do not
   move, this attribution is wrong and the decor layer must be exonerated.
2. **Record the fields this run omitted** — managed-window count, the per-thread table, and
   `GetGuiResources` GDI/USER. Each retained `Dib` holds one `HBITMAP` + one `HDC`
   (`layer.rs:349-407`), so GDI objects are a direct counter for decoration surfaces:
   expect ≈ `3 + 2 × (frames + tab indicators)`. That single number resolves the question
   the current record could not.
3. **Soak with a time series, not two endpoints.** Same process, sample private bytes,
   working set, handles, GDI/USER and the layer's own counters
   (`wiri::overlay::decor::layer::stats()` → `PAINTS`, `MOVES`, `layer.rs:66-72`) at
   1 min / 10 min / 60 min of idle, then after N workspace switches (which create and
   destroy decorations). Decision rule: flat within a few pages and `PAINTS` not growing
   while idle → bounded, close the item. A monotone rise, or `PAINTS` rising at rest → real
   finding, reopen.
4. **Resize/scroll churn** (create/destroy/replace decoration surfaces ~100×) to confirm the
   `DeleteObject`/`DestroyWindow` path returns the commit and that the allocator high-water
   from `Dib::new` churn does not ratchet upwards.
5. **Optional, only in a lane allowed to install tools:** a measured binary-size breakdown
   (`cargo bloat --release`, or section sizes via `dumpbin /headers`) for `af3eb30` and
   `cc3920d`, to replace the 40-bytes-per-line estimate above and to attribute the
   `windows`-feature share of the +2.27 MiB.
6. **Optional:** an idle budget to replace "unexplained". For a one-monitor desktop:
   private ≤ 12 MiB, working set ≤ 40 MiB, and *flat over 60 minutes* — a ceiling plus a
   slope, since this dataset shows the two endpoints alone cannot distinguish a plateau from
   a slow leak.

## Method and scope

Read-only: `git log`/`show`/`ls-tree`/`diff` (no checkout), `docs/PERF.md` at `f1528c7`,
`src/runtime.rs`, `src/main.rs`, `src/backend/reconcile.rs`, `src/backend/mod.rs`,
`src/hooks/tray.rs`, `src/ipc/mod.rs`, `src/overlay/workspace_indicator.rs`,
`src/overlay/decor/*`, `src/layout/engine/decor.rs`, `src/screenshot/*`,
`resources/default_config.kdl`, `Cargo.toml`, `Cargo.lock`. No `src/` edit, no build, no
`scripts/live-test.ps1`, no process started or stopped, no other worktree touched. Line
numbers are from `cc3920d`.
