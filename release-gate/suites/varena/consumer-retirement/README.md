# Source consumer retirement (GPU prepared, unrun)

Run `prove.py` in a fresh, ordinary Cozy one-GPU child with the matching installed SDK and Runtime. Bind `input.UNBOUND.json` from the actual installed files and scheduler-selected GPU UUID. The client rents, installs and downloads nothing. The parent must retain the real child exit/stderr and require its `passed=true` result; an outer completed request is insufficient.

```bash
python -I -B prove.py --input ACTUAL_INPUT.json --case private
```

Cases are parallel qualification cells, each in its own process:

- `private`: a real Fill/commit, two independent DLPack Storages sharing the same private owner, stream retirement/destruction while an alias survives, a later legitimate consumer, and final idle-thread cycle GC. Actual physical reclamation is required.
- `typed`: public Source/Use/Region handles close while a typed Tensor retains its Pin; retire/destroy its consumer stream before idle-thread cycle GC, then require reclamation and no new public quarantine.
- `reuse`: the private case must observe an actual equal raw handle after checked destruction/recreation. It tries at most32 owned streams; absent observed reuse is explicitly unqualified, never replaced by a fake handle.
- `capture`: actual control→body→control fork/join capture, temporary Use and distinct Storage destruction while canonical Storage remains owned, full replay/readback, then checked stream retirement/destruction and idle-thread cycle GC without retaining physical weight backing.
- `template`: the same ownership case with `CUDAGraph(keep_graph=True)`, deliberately template-only, matching Runtime's relevant close boundary.
- `failed-template`: real32MiB allocation refusal in an8MiB prepared pool, unchanged keep_graph=True template cleanup, then delayed GC after actual stream destruction. No numerical retry or forced reserve increase.
- `fill-abort`: an abandoned Fill remains unreadable; after its consumers complete, a fresh explicit Fill must initialize and commit all bytes before readback/reclamation.
- `capture-refusal`: active capture must refuse stream retirement without publishing completion. Genuinely dropping the final private owner in that capture must retain its exact Pin through process exit. This intentional negative cell does not claim graceful cleanup.

The client uses Runtime's actual `accel.nonblocking_stream`/`destroy_stream`, retires every live handle before any destruction, and does not query destroyed handles. It checks actual artifact/module hashes, CUDA profile, UUID, independent Storage identities, output bytes and physical Region residency. It makes no full-model, hardware-matrix, benchmark or universal-safety claim. The original SDXL idle-GC reproducer remains an additional combined Runtime/SDK gate.

Source compilation/review and a real host extension build have passed separately. None of these GPU cells has been executed by this source lane.
