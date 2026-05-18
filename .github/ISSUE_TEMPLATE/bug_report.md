---
name: Bug report
about: Something wiri does (or doesn't do) that you think is wrong
title: ''
labels: bug
assignees: ''
---

**What happened**

A clear description of what wiri did that surprised you.

**What you expected**

What you thought should happen instead.

**To reproduce**

1. Start wiri with `…`
2. Open these windows: `…`
3. Press `…`
4. Observe `…`

**Environment**

- Windows version (run `winver`):
- wiri version (run `wiri --version`):
- Architecture: `x86_64` / `aarch64`
- Active monitors (paste `wiri-ctl list-monitors` output):

```
<paste here>
```

**Configuration**

A minimal `config.kdl` that reproduces the issue. If you can't reduce it, attach
the full one — but please redact anything personal.

```kdl
<paste here>
```

**Logs**

Tail of `stderr` captured with verbose tracing:

```
RUST_LOG=wiri=debug .\wiri.exe -v 2>wiri.log
```

```
<paste relevant lines here>
```

**Anything else**

Screenshots, video, or extra context.
