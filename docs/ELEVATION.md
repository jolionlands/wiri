# Elevation and logon startup

## Scope and current behaviour

This is a design note, not an elevation implementation. wiri does not gain a task,
`uiAccess` manifest, automatic UAC prompt or administrative token from this change.
The default remains a normal-integrity (normally medium) desktop process.

`wiri.exe --register-autostart` writes the `wiri` value under
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. It is a `REG_SZ` command
line, now always enclosing the executable path in double quotes, including paths
without spaces. For example, the value for a spaced installation path is:

```text
"C:\Program Files\wiri\wiri.exe"
```

The value is still NUL-terminated UTF-16LE, with no extra arguments. Quoting avoids
ambiguous executable selection; it does **not** elevate the process. Existing
entries are not migrated automatically: re-run `--register-autostart` with the
updated executable to replace the old value. `--unregister-autostart` still removes
only that Run value, not any manually created scheduled task.

### Default limitation

Windows User Interface Privilege Isolation (UIPI) restricts a normal-integrity
process from interacting with higher-integrity applications. Moving, resizing,
focusing or closing an elevated window, and keyboard/mouse hook behaviour while
it is foreground, must not be assumed to work. Registering a global hotkey does
not grant cross-integrity window control. UAC's secure desktop and the lock screen
are outside wiri's normal interactive desktop, not supported management targets.
Users who keep the default should expect elevated applications to need manual
window management; autostart quoting does not close that parity gap.

## Options compared

| Option | Launch and prerequisites | Benefit | Cost / limitation |
| --- | --- | --- | --- |
| Document normal integrity (current default) | Ordinary launch or the per-user Run value; no admin setup | Portable, least privilege, no signing/install requirement | Higher-integrity windows remain a documented limitation |
| Scheduled task with highest privileges | Explicitly provision a task **at logon for the same user**, **run only when that user is logged on** (interactive token), with **Run with highest privileges** | An administrator's elevated token can manage ordinary elevated windows on that interactive desktop | Requires authorized admin setup; gives the entire manager administrative authority, not just UI access |
| Signed `uiAccess` release in a trusted installation | Authenticode signature trusted by Windows, a manifest with `asInvoker` and `uiAccess="true"`, and a protected install location such as Program Files under the normal UIAccess policy | Allows policy-permitted cross-integrity UI interaction without requesting general administrator rights | Needs signing, a trusted installer/update chain and secure ACLs; not suitable for an unsigned portable or development build; not unrestricted access |

### Scheduled-task design constraints

This would be an explicit opt-in, not a silent replacement for Run startup.
"Highest privileges" uses the highest token available to the task's principal;
it does not turn a standard user into an administrator. Administrator consent or
credentials are required to provision an elevated administrator launch. A SYSTEM
or service launch, or "run whether the user is logged on or not", is not the
interactive window-manager solution: it may run outside the user's desktop.

A future implementation must avoid duplicate Run/task launches and provide task
removal separately. Use an explicit executable path and a separate arguments field
(no shell wrapper), with a protected binary, dependencies and task definition.
The task is not a UAC bypass and does not provide access to the secure desktop.

Running the whole manager elevated increases the trust placed in configuration,
includes, `spawn-at-startup`, binds and IPC commands: spawned applications normally
inherit that elevated token. Before shipping this option, review those inputs and
child-token policy. Review named-pipe DACLs **and** mandatory-integrity policy and
client authorization; a same-user client or owner-only DACL is not by itself an
adequate privilege boundary. Do not loosen IPC permissions just to make an
unelevated `wiri-ctl` connect. Per-user/session naming remains a separate concern.

### Signed uiAccess design constraints

Setting `uiAccess="true"` alone is insufficient. Windows must validate the signature
and trusted installation requirements; a user-writable checkout or per-user portable
folder does not satisfy the normal secure-location policy. Do not disable that
policy as an installation shortcut. Protect binaries and updates against replacement.

UIAccess relaxes specific UI restrictions; it is not an administrative token, a
filesystem/registry privilege grant, a guarantee that every Win32 API succeeds, or
permission to manage the secure desktop. Focus-stealing rules and application-specific
restrictions still apply. Configuration, spawn and IPC trust need review here too:
a UI-capable process is still security-sensitive. Validate actual hook and window
operations on supported Windows versions rather than claiming parity from the
manifest alone.

## Decision and verification boundary

Keep normal integrity as the documented default. A future interactive scheduled
task is the smaller deployment change for users who deliberately accept a fully
elevated manager; signed uiAccess merits evaluation for a UI-only, least-privilege
release once signing and trusted installation are available. Neither option is
implemented or enabled here.

The tests in `src/hooks/startup.rs` exercise only pure command-string construction
for spaced, unspaced, Unicode and UNC executable paths. They do not open or write
registry keys, create tasks, launch processes or test elevation. All four formatting tests
passed in the final Lane D release suite (918 unit tests total), with `RUSTFLAGS=-D warnings`
for the required release check/test/build commands. Logon behaviour, a real Run-key write and
elevated-window interaction are not live-verified.

Before implementing elevation, use an isolated interactive test account to verify
medium/high-integrity move, resize, focus, close and input behaviour; child token
inheritance; IPC authorization; logon/logout and duplicate-instance handling; and
clean removal. Cover standard and administrator accounts and x64/ARM64. Treat
secure-desktop isolation as a boundary to preserve, not a feature to bypass.

See [the parity plan](parity/README.md),
[the Windows hazards in Appendix C](parity/niri-gaps-config-input-ipc.md#appendix-c-windows-only-hazards-with-no-niri-counterpart),
and the remaining Phase 3 work in the [Phase 1](parity/phase1-report.md) and
[Phase 2A](parity/phase2a-report.md) reports. The historical unquoted-Run observation
is addressed by this formatting fix; elevated-window support is still outstanding.
