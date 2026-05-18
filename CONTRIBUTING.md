# Contributing to wiri

PRs and issues welcome. This is a personal experiment that grew into a
public project — expect rough edges and don't be surprised by big diffs.

## Quick orientation

- `docs/SPEC.md` — full architecture
- `docs/DOCUMENTATION.md` — module reference
- `docs/PROGRESS.md` — implementation log
- `docs/PARITY.md` — wiri vs niri, feature by feature
- `docs/QUICKSTART.md` — 3-minute install + run
- `docs/TROUBLESHOOTING.md` — common issues

The repo layout is in `docs/SPEC.md` § Architecture.

## Development loop

```pwsh
git clone https://github.com/jolionlands/wiri.git
cd wiri

cargo build --release
cargo test --release --no-fail-fast
cargo check --release --all-targets    # warnings are errors in CI
```

To iterate on a running daemon:

```pwsh
wiri-ctl quit                          # stop the running instance (if any)
cargo build --release
.\target\release\wiri.exe -v 2>wiri.log
```

There's a helper at `scripts/dev-restart.ps1` that wraps all three steps.

## Code conventions

- **Rust 2021 edition.** Use `anyhow::Result` for fallible boundary code,
  `thiserror` enums for typed errors that callers branch on.
- **Logging** via `tracing`. Use `info!` for milestones, `debug!` for state
  transitions, `warn!` for recoverable surprises, `error!` for things that
  break user-visible behaviour.
- **No `println!` in library code.** The `wiri-ctl` binary is the only place
  `println!` belongs (it prints to the user). Tests can use `println!` freely.
- **No `dbg!` in committed code.** Use `tracing::debug!` instead.
- **No new `unsafe` without a `SAFETY:` block comment** explaining why.
- **Doc comments are user-facing.** Avoid leaking task-tracker labels
  ("Item 4:", "Agent E:") into doc strings — they outlive the PR.
- **Match arms exhaustive.** When adding an `Action` enum variant, search
  every `match action {…}` and add an arm — `cargo check` won't catch
  non-exhaustive matches that fall through with `_ => {}`.
- **ARM64 quirks.** Don't use `MOD_NOREPEAT` — `RegisterHotKey` rejects it
  on aarch64. wiri uses software debouncing at 125 ms instead.

## Tests

Tests live in `#[cfg(test)] mod tests {}` blocks inside the file they cover.
Engine tests use `BackendHandle::default_for_test()`. Layout tests should
build a minimal `TilingEngine` rather than the full app.

CI runs the full suite on every push under `RUSTFLAGS=-D warnings`. PRs
that don't pass locally won't pass in CI.

## Commit messages

One-line subject ≤72 chars, then a blank line, then a body explaining the
**why**. Reference any issue or PR. Co-authoring tag at the bottom is fine.

```
feat: support per-output layout overrides

The wider goal is config-driven multi-monitor differentiation —
laptop screens want narrower columns than the external 4K.

Closes #42.
```

## Reporting bugs

Use the bug-report template at `.github/ISSUE_TEMPLATE/bug_report.md`.
Especially helpful: the verbose log captured with `RUST_LOG=wiri=debug`
and the output of `wiri-ctl list-monitors`.

## Suggesting features

If it tightens niri parity, link the niri doc/commit. If it's wiri-original,
explain the workflow you have in mind. See `.github/ISSUE_TEMPLATE/feature_request.md`.
