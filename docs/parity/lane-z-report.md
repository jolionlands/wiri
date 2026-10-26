# Lane Z — floating stack after delayed native raises (2026-10-26)

Scope: keep normal workspace floats above their tiles. No focus-layer actions,
workspace/monitor policy, input policy, display changes or dependencies. Engine
changes are a module declaration and replacement of the existing `raise_floats`
implementation, leaving the concurrent focus/workspace lane's call sites intact.

## Reproduction and cause

The reported failure was official `358b43c` H5 screenshot `h5-04-tile-focused`:
a tile (Windows Terminal) covered the red float after receiving focus. The ordinary
H5 scenario did **not** reproduce that failure in this lane's three baseline runs;
passing H5 alone was therefore insufficient evidence.

H45 in `scripts/live-phases/lane-z.ps1`, dot-sourced by `scripts/live-test.ps1`,
creates one tiled and two overlapping floating WinForms windows, each on its own
UI thread. After the tile is focused, its explicitly armed **one-shot 450 ms test
fixture timer** calls native `SetWindowPos(HWND_TOP, SWP_NOACTIVATE)` on that
thread. Before returning to its message pump it records both the actual inversion
and the tile's foreground identity. This is a real delayed raise, not a synthetic
WinEvent or an invocation of engine internals. Screenshots and `TopWindowAt` at
the red float's center distinguish a real covering tile from model state.

Saved baseline release binaries were built from the starting source `a499d202`;
this is a reproduction of the reported mechanism, not a rerun of the official
`358b43c` executable. The late raise covered both floats in **3/3** baseline H45
repetitions. For example, baseline run 1 records center HWND **6624192** (tile),
not **13111632** (red float), with one forced inversion and tile foreground true.

Two issues made the old focus-time raise insufficient:

- Another UI thread can consume a late raise after the foreground/layout repair.
- The local WinEvent constants mislabeled SHOW as `0x8003` and HIDE as `0x8004`.
  Actual SHOW is `0x8002`, HIDE `0x8003`, and **REORDER `0x8004`**; reorders were
  being classified as hides instead of triggering stack reconciliation.

## Repair and cost model

`backend/hooks.rs` accepts top-level REORDER on OBJID_WINDOW or desktop
OBJID_CLIENT, rejecting accessibility children. `BackendEvent::ZOrderChanged`
reconciles under a read lock without layout mutation or IPC state publication.

`engine/float_stack.rs` snapshots actual `GW_HWNDPREV` order, with a cycle guard
and a 4096-step bound. It considers visible, non-minimized, non-cloaked,
non-fullscreen normal-band floats of the active workspace, sorted by float MRU.
Only inversions produce placement requests:

- Lower an overtaking older float below its newer neighbor.
- Lower an offending tile below the actual lowest float.

Requests use relative HWND anchors and
`SWP_ASYNCWINDOWPOS | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE`. They do not wait
for an application's UI thread. Unlike global float promotion, even a late
completion cannot promote a float over a newly focused unrelated application.
Anchors are rechecked for visibility/normal band; unrelated foreground or inactive
workspace reports cannot promote hidden floats. The existing focus/layout repair
call sites use the same implementation. No HWND_TOPMOST is introduced.

There is **no new idle timer, polling loop, always-on thread, resting HWND or
crate**. Work runs on existing focus/layout events and native reorder
notifications. A satisfied stack submits no positioning calls; native feedback
therefore settles instead of continually restacking. H45 observes unchanged native
z-order-request counters on all three probes over a 900 ms settled interval.
The test-fixture one-shot timers/threads exist only while H45 is running.

Eleven added pure tests cover event IDs/object filtering, inert stacks, delayed
raises, MRU, unrelated/excluded HWNDs, float-only workspaces, and all 120
permutations of three floats/two tiles with both asynchronous completion orders.

## Verification and pass rates

All required release commands passed with `RUSTFLAGS=-D warnings` and `-j 2`:
`cargo build --release`, `cargo test --release`, and
`cargo check --release --all-targets`. **1210 library + 14 CLI tests passed**;
one documentation example is ignored. CI rustfmt passed before both runtime/test
commits; no Rust source changed afterward. The final documentation-only formatter
attempt was refused by the command auditor (shared script contents unavailable),
and a narrow access request was recorded; it was not bypassed.

The complete required locked desktop invocation against runtime commit `1e0c358`
finished with `DONE` on TRIDARIS (one 1366x768 display, work area 1366x728):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\live-test.ps1 -Bin target\release -Out .live -LockWaitMinutes 120
```

The unchanged `scripts/live-report.py` rules count **500 pass, 0 fail, 79
information entries**. `hidden_json_present_after_restart` is correctly false.
No numeric IPC step return code is nonzero; idle layout positioning is zero,
including the ordinary-form and terminal settled gates. The existing informational
`h9_overlapping_terminal_pairs` is **2**; this is not an all-clear claim about
Terminal minimum-width behavior or a new gate. No existing assertion was weakened.

For repeated evidence, the harness accepts `-FloatRuns 3` (default remains one),
retaining every H3/H5 snapshot and screenshot under `float-runs/N`, and H45 under
`lane-z/N`. Targeted repetitions reused the exact harness scenario text, mutex,
isolated profile, native fixtures, FocusForm, WaitForeground and CloseFocused;
they were not replacement assertions.

| Scenario | Before | After |
| --- | --- | --- |
| H3, 22 gates/run | 66/66 (3/3 complete runs) | 66/66 (3/3 complete runs) |
| H5, 15 gates/run | 45/45 (3/3 complete runs) | 44/45 (97.8%; 2/3 all-green runs) |
| H5 above-focused-tile boolean | 3/3 | 3/3; see foreground caveat below |
| H45, 8 gates/run | 21/24; delayed-layer gate fails 3/3 | 32/32 (4/4 runs: three targeted + full suite) |
| H45 forced inversion/foreground precondition | 3/3 | 4/4 |

**Retained repeat caveat, not a floating regression:** after H5 repetition 3's
successful focus of tile 5313528 at `17:37:17.939Z`, a newly opened Terminal
23922668 became foreground at `17:37:18.568Z`, before the old assertion's 1.8 s
sleep finished. Thus `h5_overlapping_tile_got_the_focus` is false in that repeat;
the float remains above the tile, but that sample cannot establish sustained tile
foreground. Both other H5 repeats and every deterministic H45 foreground gate
pass. No foreign process was stopped. An additional repeat was queued but its
command timed out waiting for the shared desktop lock; it is not counted as a run.
This lane does not change the old focus fixture or the concurrent focus policy.

The full suite's existing one-shot perf observer records **0 ms process CPU** over
30.010 s (counter resolution, not proof of literally zero CPU), **244.084 context
switches/s lower bound**, unchanged input tick, **0 positioning lines**, and
0 collector errors. These information-only observations include pre-existing
runtime wakeups; they are not causal before/after performance estimates. No load,
stress or soak test was run.

### Local raw evidence (ignored, retained)

- `.live/summary.json`, `DONE`, `float-runs/1`, `lane-z/1`, `perf/idle.json`:
  completed full post-fix suite.
- `.scratch/evidence/before/float-runs/{1,2,3}`: complete H3/H5 baseline snapshots
  from an otherwise interrupted full harness attempt (not claimed as a full pass).
- `.scratch/evidence/z-before/lane-z/{1,2,3}` and
  `.scratch/evidence/z-after/lane-z/{1,2,3}`: completed targeted H45 comparisons,
  including `observed.json`, per-run gates and screenshots in their parent output.
- `.scratch/floats-after/float-runs/{1,2}`: the other two post-fix H3/H5 repetitions,
  including the Terminal interruption trace in `wiri.stderr5-run2.log`.
- Early diagnostic/incomplete harness attempts are retained separately, not pooled
  into the comparison or represented as complete full-suite passes.

## Interrupted-baseline cleanup handoff — pending tool access

The completed `.live` run quit cleanly and left no recovery journal. However, the
earlier timed-out baseline retained
`.scratch/evidence/before/profile/local/wiri/hidden.json`; its original contents
are also backed up as `.scratch/evidence/before/hidden.before-recovery.json`.
A final read-only check found 18 recorded HWNDs still live and hidden (one gone).
Their PID/class identity must be validated by normal recovery, not assumed from
HWND existence. **Recovery of this interrupted baseline is still pending.**

The attempted safe cleanup acquires `Global\\wiri-live-test`, refuses foreign
daemons, starts an owned daemon with the saved in-worktree profile and unique
`WIRI_PIPE`, and quits only that child. Command review refused the first commands
as truncated; the shorter complete command was refused because the scope checker
treated `$r/profile/roaming` as outside the worktree. No recovery daemon was
started. A narrow access request for the saved profile directories was recorded;
the refusal was not bypassed. An operator must complete identity-validated recovery
under the desktop lock before discarding this journal. No foreign daemon or
window should be forcibly stopped to do so.

## Remaining limits

This is eventual event-driven Win32 stacking, not compositor-atomic stacking:
a native raise may be briefly visible before its asynchronous repair. A hung
window cannot be forced to consume its own placement request; responsive tiles
can still be lowered beneath a hung float. Continuous hostile self-raising and
multi-display behavior were not live-tested. Dedicated focus-layer actions remain
the other lane's work. The old shared-desktop H5 focus observation can still be
interrupted by an unrelated newly opened application, as explicitly recorded above.
