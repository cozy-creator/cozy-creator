# Categorical machine mode through ordinary update

This is the cl-694 setter, implemented on a new task branch from fresh master53da
plus lifecycle616. The sampler27e/Runtime907 and lifecycle616 frozen worktrees are
unchanged. No installed CLI, daemon, SDK or machine setting was changed by local
implementation.

`cozy machine update NAME --mode shared|dedicated` sends the existing typed
`MachineMode` category. The ordinary machine update endpoint validates it and records
it in the durable request. Mode-only work is a real update: it does not resolve the
Hub's latest software and does not return through the unchanged-software shortcut.
After the existing drain, the updater edits requested mode and development limit
together, validates the full TOML and atomically replaces the config. The ordinary
restart rereads `machine.mode`; no hot mutation of an active Memory is introduced.

The CLI reports the resulting mode and requires a live post-restart status matching
the requested category. The software `to` record does not carry mode, so its
unreachable-machine fallback is explicitly unavailable for a mode request. Existing
mode reports that are empty remain unknown rather than implying Shared or Dedicated.
An older worker rejects the unknown update payload field before draining; install a
supporting worker through the existing route first. There is no separate daemon or
new transport protocol: Calcifer already carries opaque update payload JSON.

The paired real qualification is in `release-gate/suites/varena/machine-mode`.
It uses ordinary update/show/run commands, holds the SDK unchanged, and compares the
live machine mode with actual Runtime Start configuration and the native participant
row. It restores the original effective mode through the same route. Source/build
proof does not qualify restart, CUDA execution or actual mode propagation.

Priority remains open: `cooperation_settings` still supplies priority0 at Start.
Categorical active/idle priority propagation needs its own implementation and real
ordering evidence. This setter adds no byte policy and does not complete Cache owner
shrink, foreign-pressure GPU qualification or legacy TensorD controller retirement.
