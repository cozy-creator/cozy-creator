# Attention corpus

Real Q/K/V for the attention suite (xs-171 M-B; tracker `cozy-runtime/xs-171`, `outputs/kernel-tables/measurement-design.md`
§3.1). Each model that declares 8-bit attention runs its bench request on one pod with the dev-only
`cozy run --attention-capture SPEC` (`crates/cli/DEV.md`), twice with one seed:

1. `ac-<model>-census`: the census alone, every attention call's site, shape, band and kernel (`census.jsonl`).
2. `ac-<model>`: the capture of what `corpus.py pick` chooses from that census: the first, middle and last site of each
   block family and every site its family's kernel doesn't serve (the exact paths), at the first and last call of each
   layout phase (dense, sparse), 4 heads each (the two with the largest key max/RMS, two seeded), at most 4 GiB a model.

The captures come back over `cozy machine ssh` into the gate directory's `corpus/`, one directory per model with each
process's `<request id>/` (census plus one safetensors a captured call), and are the gate's artifact. `corpus.py check`
holds each capture to its census row (shape, BF16, finite) and runs one short site's BF16 SDPA against FP32 (BF16
rounding, at most 0.01). `corpus-summary.md` tabulates calls, sites, shapes and bytes per model.

```sh
gh workflow run release-gate.yml -f hub=prod -f suites=attention-corpus -f establish=true -f publish=false -f packages_ref=<open rc> \
  -f cozy=<dev-build run> -f tensorfs_wheel=artifact:<dev-build run>/tensorfs-wheel-<sha> -f runtime_wheel=<cozy-runtime dev-build run>
```

The capture needs TensorD and TensorFS that carry `attention_capture` (cozy 0.25) and a Runtime with the capture hook
(Runtime xs-171 M-A). One RTX 4090, about an hour, about $1.5. Re-capture only when a model's attention inputs change.
