# Lane C2 — config, bindings, keyboard and session configuration

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

## Validation

Final warning-denied release build/test/all-target check and the complete locked
live harness are recorded here after integration. H66-H69 evidence lives under
`.live/lane-c2`; no live pass is claimed before that run.

## Cost and limits

No idle timer, polling loop or always-on thread is added. Config validation and
completion allocations occur only on load or command invocation. Balloon display
is not visually asserted. Windows search deliberately has no machine-wide config
fallback. Keyboard/system settings are tested synthetically and dry-run only on
the shared laptop; output-priority behavior on multiple displays is synthetic.
