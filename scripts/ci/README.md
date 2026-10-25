# CI and release plumbing

## CI (`.github/workflows/ci.yml`)

- **format**: one `cargo fmt --all -- --check`, before spending Windows runner time.
- **build**: native x64 on `windows-latest`, native ARM64 on `windows-11-arm`.
  Both verify the runner architecture and Rust host, then check all targets with
  `-D warnings`, test, and build release binaries, with locked dependencies and
  two Cargo workers. There is no redundant ARM cross-build. Identical `RUSTFLAGS`
  across check/test/build avoid flag-induced rebuilds.
- **live**: experimental, `continue-on-error`, PRs and manual CI dispatch only.
  Downloads the matching build job's binaries instead of compiling again. Uses
  Windows PowerShell 5.1/.NET Framework (the harness embeds C# WinForms code).
  Artifacts are uploaded even on failure or a desktop skip.

`Swatinem/rust-cache` caches the Cargo registry/git sources and target directory,
including OS/toolchain/lockfile inputs. Each native build has its own target key;
CI saves on main only (PRs can restore); release jobs reuse those matching native
caches read-only. Live jobs have no Cargo work to cache.
Superseded CI runs are cancelled; one architecture failing does not cancel the
other architecture's diagnostics.

### Runner availability and GUI support

Checked with `curl.exe` against current GitHub sources:

- [Hosted runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
  and its [public/private runner table](https://github.com/github/docs/blob/main/data/reusables/actions/supported-github-runners.md)
  list **`windows-11-arm`**, ARM64, including free standard runners for public
  repositories. It is not a self-hosted or larger-runner label.
- [x64 image inventory](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md)
  and [ARM64 inventory](https://github.com/actions/runner-images/blob/main/images/windows/Windows11-VS2026-Arm64-Readme.md)
  list the .NET/desktop runtimes and PowerShell tools. Windows Terminal is not a
  documented installed dependency; probe `wt.exe` instead of assuming it exists.
- [Runner image maintainers' desktop/foreground investigation](https://github.com/actions/runner-images/issues/14049#issuecomment-5217338266)
  confirms **interactive desktops on both images**, including ARM64 `WinSta0`.
  ARM64 can nevertheless refuse `SetForegroundWindow`; this is not proof of a
  headless session. Wiri's real foreground/hotkey assertions remain enabled and
  may expose this limitation. Do not paper over their failures as software skips.
  [Reports of ARM privacy/OOBE and other foreground windows](https://github.com/actions/runner-images/issues/14069)
  also make clear that a clean desktop/input environment is not guaranteed.

An installed desktop stack is not a guarantee of an unlocked, accessible input
session on every allocated VM. `live-ci.ps1` checks UserInteractive, nonzero
session, matching Default input/thread desktops and screen capture before
starting anything. If unavailable, it records `status: skipped` and the reason;
**a skip is not a live-test pass**. It never unlocks/logs in via RDP or schedules
an interactive task. Run mode refuses non-GitHub/self-hosted machines and any
pre-existing wiri process. Config and runtime data are isolated inside the
checkout's `.scratch/live-profile`.

With **`WIRI_LIVE_CI=1` only**, a missing `wt.exe` skips just the explicit H2
Windows Terminal spawn, records its gate as `null` and explains it in `ci_skips`.
All other spawn tests and cmd.exe console phases B/F/H9 still run (classic
console coverage is not Windows Terminal minimum-width coverage). Normal laptop
runs retain the original requirements and summary format. CI also fails fast on
unexpected PowerShell errors. The adapter requires every declared summary field,
checks typed boolean/count gates and step exit codes, and rejects undocumented
nulls/skips. Informational timing maxima are not new performance budgets. Fixture
checks run in the native build jobs without desktop automation. No ffmpeg is
needed: CI runs `live-test.ps1`, not the optional video recorder.

Evidence artifact contents:

- `.live/summary.json`: full completed gate summary plus CI status/failures, or
  an explicit skip/error result; `running` means an interrupted/incomplete run.
  `passed-with-skips` means all executed gates passed but WT coverage was skipped.
- `.live/ci-preflight.json`: desktop/software probe details.
- `.live/harness/`: screenshots, logs, step outputs and the original harness
  summary/DONE marker when the test ran. No screenshots are invented for skips.

Local fixtures and read-only capability check (neither starts wiri, opens test
windows or injects keys):

```powershell
powershell -NoProfile -File scripts/ci/test-live-summary.ps1
powershell -NoProfile -File scripts/ci/live-ci.ps1 -ProbeOnly -Out .scratch/live-probe
```

Do not run the full desktop harness on a shared build host just to validate CI.

## Draft releases (`.github/workflows/release.yml`)

- **resolve**: on a `v*` tag push, or manual dispatch with an existing `v*` tag,
  validates a filename-safe exact tag before checkout and resolves its commit SHA.
  A manual input cannot select a branch. Tags must include these CI helpers.
- **build**: native x64 and ARM64 jobs verify the runner architecture and Rust host,
  use separate Cargo caches and identical warning-denying flags, check all targets,
  run native release tests, and build both release EXEs. Packaging fixture tests
  run without executing their synthetic PE files. `package-release.ps1` verifies
  each real EXE's PE machine type, then puts
  `wiri.exe`, `wiri-ctl.exe`, `default_config.kdl`, `README.md` and `LICENSE` (when
  present) at the ZIP root. Fixed entry ordering/timestamps make repeated packaging
  of the same bytes reproducible under the same runtime; Rust builds are not promised
  to be reproducible.
- **publish**: downloads both packages, recomputes SHA-256 from the downloaded
  bytes, and attaches `wiri-<tag>-<target>.zip`, each `.zip.sha256` sidecar and a
  combined `SHA256SUMS` to a **draft** release. Only this job has `contents: write`;
  it checks the tag has not moved and refuses to change a published release.
  Reruns replace only these named draft assets; uploads for one tag are serialized.

The release workflow never publishes automatically. Packaging validation uses
only local fixtures (no Rust build or executable launch):

```powershell
powershell -NoProfile -File scripts/ci/test-package-release.ps1
```

## Validation before pushing

Parse workflow YAML and run `actionlint .github/workflows/*.yml` if available.
Use Windows PowerShell's `System.Management.Automation.Language.Parser` to parse
helpers and inline PowerShell steps without executing them. Keep downloaded
validators, fixtures and evidence under git-ignored `.scratch/`.

Only GitHub can validate runner allocation, native ARM execution, cache hits,
artifact transfers, release-token permissions and actual hosted GUI gates. After
pushing, inspect both native jobs, the live summary/skip notes and screenshots,
and a disposable draft release from both a `v*` tag and manual dispatch. Do not
publish it until ZIP contents/architectures and SHA256SUMS have been checked.
