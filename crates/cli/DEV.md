# Dev-only CLI features

Ours, not the official interface: in no `--help`, and refused unless `~/.cozy/config.toml` has, at
its top (above any `[section]`):

```toml
dev = true
```

Anyone may turn them on; the Hub does not gate them. Mark a new one in `src/dev.rs` (`DEV_ONLY`)
and list it here.

| Command | What |
|---|---|
| `cozy machine rent <sku> --provider runpod\|vast` | Quote and buy on that provider only. The Hub prefers a datacenter holding one of our NFS drives; the quote names provider and datacenter, and `cozy machine show` (`--json`: `rental.provider`, `rental.datacenter`, `rental.nfs_drive`) says where it landed and whether the drive mounted. |
| `cozy run <fn> --attention-capture SPEC` | The Runtime writes what its attention kernels receive, on the machine (xs-171's corpus): SPEC is JSON or `@FILE`, `{"dir": "/abs/path", "sites": ["<component>/<module path>" fnmatch, ...], "calls": [ordinal, ...], "heads": 4, "seed": 0}`. Under `dir/<request id>/`: `census.jsonl` (every call's shape, band and kernel) and one safetensors of the kept heads' Q/K/V per matching call. The run, and every child it calls, is never answered from the call memo. Needs TensorD and TensorFS of the same build. |

## Rentals follow the open RC (cl-922)

With `dev = true`, each rental this computer orders runs the open RC's newest CI builds: the Runtime (with its
kernels), TensorFS and the `cozy` a machine runs. Each push to `rc/<x.y.z>` in cozy-runtime and cozy-creator puts them
on that repository's rolling prerelease `rc-<x.y.z>` (`rc-dev.yaml`, a few minutes after the push). Calcifer moves a
rental to them once it is ready, before its first run, and again as the RC moves, only while it has no work. The open
RC is the minor after this cozy's release, or an RC build's own (`0.26.0-dev+g1a2b3c4d`: 0.26.0).

- `cozy machine show <m>` (and the panel) says `rc 0.25.0 current|behind|held|none: …`; a run on dev builds says
  `machine: <m> runs RC builds: Runtime <sha> · TensorFS <sha> · cozy <sha>`.
- An update by hand that names software holds a rental where it is, the RC's own builds too. `cozy machine update <m>`
  with nothing named moves it to the RC again. `--runtime`, `--tensorfs` or `--tensord` with the RC's version
  (`0.26.0-dev+g1a2b3c4d`) takes that build from the prerelease, and holds it there as the RC moves on.
- Packages built for the RC Runtime are PEP 440 prereleases (`0.10.0rc1`): the Hub's unversioned default and every
  released client skip them; a rental on the RC's Runtime takes the newest one.
