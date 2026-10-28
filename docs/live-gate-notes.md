# Live-gate notes: timing races between the harness and the event-driven runtime

Written 2026-10-28 after the gate-hardening pass on `wiri/shine-20261024`
(`a2c9550` → `f4157d3`). Every fix below came from a live-cycle failure whose
product behaviour was proven correct from daemon logs and per-gate evidence
files; the gate, not the product, was wrong. Recorded here so the next person
touching these gates starts from the diagnosis instead of rediscovering it.

Cycle evidence base: tridaris interactive-desktop cycles at `9d846ae`
(764/5), `a2c9550` (767/6), `bf6815b` (767/2), `d01caee` (767/2),
`843a839` (767/2), `c0a9794` (768/1) and `f4157d3` (769/0), plus the hosted
x86_64 live-desktop job passing at `a2c9550` and `f4157d3`.

## The pattern

The event-driven runtime (perf pass `0ebdebf`) replaced fixed polling with an
event-armed main loop. Focus, warp and overlay effects now land **after** the
IPC reply returns — under a loaded desktop (20+ managed windows, window
enumeration bursts) the foreground-notification chain took ~900 ms from
`FocusWindow` reply to warp. Any gate that samples state once, immediately
after a command, races that chain. Three gates lost the race in different
ways; the fix in each case is to wait for the effect and assert the stable
end state, never an intermediate one.

## h25 warp gates (`scripts/live-phases/lane-b.ps1`)

- Symptom: `h25_warp_tracks_next_focused_window` failed on tridaris three
  cycles running while passing on the hosted x64 runner at the same commit.
- Evidence: the daemon logged `Warping pointer ... point=(103,189)
  destination=(1121,364)` 897 ms after the IPC reply, but the gate had already
  sampled the cursor once and recorded it unmoved; in the previous cycle the
  warp destination was the centre of the *pre-settle* rect (x=1121) while the
  gate measured the settled rect (centre x=1158, 18 px tolerance) — two
  independent races.
- Root cause: `warp_geometry_ready` (src/input/focus.rs) deliberately waits
  out the native layout settle, so the warp lands late *and* aims at the
  rect the daemon saw at warp time.
- Fix (`843a839`): poll up to 2500 ms for the cursor to come to rest inside
  the focused window's rect, re-measuring the rect each 25 ms sample, and
  record the elapsed wait in the evidence. The invariant is "pointer tracks
  focus", not "pointer at the centre I measured".

## h15 id-action setup (`scripts/live-test.ps1`)

- Symptom: `h15_an_action_can_name_its_window_with_id` failed on the hosted
  x64 runner at `9d846ae` and on tridaris at `c0a9794`, passing in between.
- Evidence: `h15-fullscreen-id.evidence.json` recorded `setup_pass=False`
  with the native foreground already correct at ms=36 while the daemon's
  managed focus and event stream stayed on the previous window for the whole
  3 s setup window; the action itself worked (`fullscreen_change_count=1`,
  `foreground_after` = expected).
- Root cause: the same foreground/event-chain lag as h25, hitting the setup
  precondition (native focus == managed focus == stream focus) rather than
  the warp.
- Fix (`f4157d3`): widen the setup poll from 3 s to 8 s. It breaks out as
  soon as the three views agree, so a healthy desktop pays nothing.

## h60 idle frame timer (`scripts/live-phases/lane-t.ps1`)

- Symptom: failed in three consecutive cycles with three *different*
  evidences — 38/38/38 frames with identical rects; 37/38/38 (one trailing
  armed-tick frame); 37/37/37 with the strip settled 103 px from its held
  position.
- Root cause, in two parts: the gate counted `dnd edge frame` debug lines in
  the lane's shared log, so an absolute baseline (`tFrames0`) carried the
  previous probe's lines; and it required the strip to sit within 2 px of
  its *held* position after release, but releasing the drag settles the
  column (220 → 117), which is layout work, not timer work.
- Fixes (`a2c9550`, `bf6815b`, `63fd9c1`): assert only the timer invariant —
  frames idle == frames out, and the post-release rects frozen across three
  samples — and keep every count in the evidence for diagnostics.

## h79 action vocabulary (`scripts/live-phases/lane-c2-binds.ps1`)

- Symptom: both h79 gates failed with "did not list the action vocabulary"
  though `wiri-ctl action-names --json` exited 0 with the full 128-name array,
  reproducible outside the cycle with the same environment.
- Root cause: Windows PowerShell 5.1's `ConvertFrom-Json` returns a top-level
  JSON array as a single scalar object; `@($s | ConvertFrom-Json)` has
  `Count` 1, so the `< 50` vocabulary check threw. (A cold-start timeout was
  suspected first and ruled out; the 10 s timeout was still raised to 30 s.)
- Fix (`a2c9550`): strip a BOM, `ConvertFrom-Json -InputObject`, unwrap when
  the single element is itself an array.

## h75 hotkey overlay (`scripts/live-phases/lane-p.ps1`)

- Symptom: failed two cycles, passed two, and on the hosted x64 runner.
- Evidence: `-match` pinned the *first* "toggling cheatsheet with N rows"
  line in the daemon's stderr — a stale 17-row line from an earlier daemon —
  while the fresh daemon had logged the 1-row hide-not-bound toggle, and the
  overlay window `Find` ran once after an 8 s poll that had already seen the
  log line.
- Fix (`63fd9c1`): take the last regex match, poll the window find inside the
  loop (it may appear after the log line, at render delay), widen to 15 s.

## Open question: event-chain latency under load

The h25 evidence shows a user-visible effect (cursor warp) landing ~900 ms
after its command reply under a 20-window desktop, and h15's focus stream
lagged 3 s. The gate fixes make the harness tolerant of this; nothing yet
establishes the latency is *acceptable*, and `p_hotkey_*` (20 ms median)
does not cover it. If focus/warp feels sluggish on busy desktops, profile
the EVENT_SYSTEM_FOREGROUND → input-thread chain in src/input/focus.rs
before adding more polling elsewhere. Also worth noting: the h79 fixture has
zero chord headroom (26 keys + F1-F6 = 32 keys × 4 modifier prefixes =
exactly 128 bindable actions), so adding one bindable action requires
widening the fixture in the same change.
