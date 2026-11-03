# Run 27056 frontend diagnostic

Prepared diagnostic; model/CUDA execution is unrun. The real red remains run
27056 (CUDA719): Creator `9d959e31d6fc58a2d4c1f7371bc3a2b6b658c6de`, Runtime
`c059294c874183499ddadba5becc1d255a4e0763`, client SHA256 `e7c4eed6fc34a427355cf6fd7b7735bea16f4470b06866bc51bbfa27eed1c652`,
6 GiB managed cap, 1024x1024, 20 steps, two calls.

## Ownership and scope

This directory owns the successor wrapper, scalar host profiler, binder and CPU
host controls. `up0-replay-diagnostic` is the separate native helper owner. No
product offload function is replaced or edited. The only allocator hook is the
original fixture's one-time helper installation after the real allocator bind.

Development context is current master500b6de7 coherently merged with exact
RC627f6978; see `ownership.json`. It is not the execution cohort. Cherry-pick
frontend feature commits only if composing another diagnostic checkout; do not
adopt the development-context merge as the executed9d source.

## Observation contract

The original `_AutomaticGraphs.dispatch` code object is profiled unchanged.
There is no diagnostic dispatch wrapper, body call, fallback or retry.

- Start the native window at `unet.up_blocks.0` invocation2, including its
  preparation, warm body, capture and replay. ATen stages2/3/4 mean warm body,
  capture and preparation/replay respectively.
- Keep the primitive native ring active with ATen stage0 across the gap.
- Trace up0 invocation3. Record the actual next automatic boundary's name and
  enter/normal return/error, then close outside confirmed exclusions.
- Original `ReturnEvidence` owns the single per-thread profile slot through a
  subclass. Its capture/replay completion classifier is unchanged. Unconfirmed
  exits or changed profile ownership prohibit dump, JSON and native fact reads.
- Persist the self-describing format3 `.native.json` first, then scalar
  `.host.json`, then independent `.returns.json`. A phase-stats refusal cannot
  erase already written native rows. Preserve original exception propagation.

Marker IDs100–203 match the native header. `return` means normal Python/C host
return, distinguished from unwind by original bytecode return offsets or real
`c_return`/`c_exception` events. It does not prove GPU completion. Copy/clone
markers are per actual Tensor method call, not one aggregate stage guarantee.

Graph identity is a weak Definition-control incarnation. `plan_handle` is the
ABI wrapper handle, never the native PlanID. Stream and pool are taken from the
actual currently borrowed epoch/definition/scope. Missing identities use0/-1
with a reason. No tensors, storages, epochs, owners or frames are retained in
trace rows or pending spans. No graph-facts polling, added CUDA query, sync,
launch blocking, body retry, cap change or numeric change is introduced.

`sys.setprofile` observes the installing inference thread only. Rust-created
Python callback threads are explicitly unavailable; no callback is wrapped to
manufacture visibility. Join existing native admission/action records separately.
A cache-retire return/decline is logical only. Working kind16 is PRE-unmap intent;
completed physical release, native PlanID mapping, graph/static/output VA census
and GPU faulting instruction attribution remain unavailable in this first mode.
A full native ring, marker ordinal0, host error/overflow, unavailable exit proof,
wrong boundary, or missing following-boundary completion makes the diagnostic
incomplete. A successful model run is not required for evidence completeness.

## Bind without installation or mutation

Use a NEW output path. `bind.py` refuses an existing path and validates all wheel,
client and helper/header identities before copying fixture files. It neither
installs wheels nor runs the model. First mode must explicitly select original9d:

```bash
frontend_source=/home/fidika/cozy/.worktrees/cozy-creator/xs138-27056-frontend-diagnostic/release-gate/suites/varena/torch-offload/up0-replay-frontend
python3 "$frontend_source/bind.py" \
  --execution-base original9d-c059 \
  --sdk-wheel /home/fidika/cozy_v2/outputs/xs138-final-admission-build-20261103/worker-sdk-9d959e31-001/wheels/tensorfs-0.17.18+dev.g9d959e31-cp311-abi3-linux_x86_64.whl \
  --runtime-wheel /home/fidika/cozy_v2/outputs/xs138-resume19-build-20261103/runtime-c059-001/source/dist/cozy_runtime-0.37.9+dev.gc059294c-cp312-abi3-manylinux_2_28_x86_64.whl \
  --frozen-bin /home/fidika/cozy_v2/outputs/xs138-rental-pressure-9d959e31-c059-20261103/fixture-001/project/ci_xs138_rental_pressure_resume19/pressure/bin \
  --helper-handoff /home/fidika/cozy_v2/outputs/xs138-27056-diagnostic-preparation-20261103/ORIGINAL9D-HANDOFF.json \
  --out /home/fidika/cozy_v2/outputs/xs138-27056-frontend-bind-20261103/bin-001
```

The helper handoff must contain the updated workspace-getter header and its
matching immutable binary; attempt002 is historical and cannot satisfy this
contract. Controlled guard wheelb91 is separately selectable, labeled, and never
silently substituted for original wheel4e70c846.

The operator supplies this bin to the successor fixture, preserving the original
model files/configs and ordinary parent/child invocation. Its child command is:

```bash
python -B /path/to/new/bin/automatic_sdxl_gpu.py \
  --model /path/to/the/unchanged/model --output /path/to/new/evidence \
  --graphs auto --steps 20 --height 1024 --width 1024
```

Use the explicitly installed/bound original SDK and Runtime artifacts; the
wrapper checks every package member against the selected wheel before inference.
It checks exact Torch2.15/cu132 build identity, records only the two relevant
child environment variables before Torch import, and obtains the actual native
latched workspace flag before CUDA initialization. An unexpected true flag is
reported and refused without changing the environment. Other original settings
remain at pinned defaults (device0, seed777, guidance5.0, original prompt).
There is no extra managed-cap argument; the unchanged client itself sets6 GiB.

Operator GPU/rental/release authority is separate. Never use this prepared
fixture or its CPU checks as CUDA719 resolution, GPU completeness, publication
or release evidence.

## Host validation boundary

`host_controls.py` loads the actual original9d Python/native package and the
exact helper. It observes real normal-None returns, Python exceptions, CPU Tensor
copy/clone calls and a real copy shape error; confirms borrowed CPU tensors die;
checks that another Python thread has no inherited profile; verifies native
markers survive the stage0 gap; and persists trace before the real public API's
invalid-device phase-stats refusal. No CUDA resources, fake epochs, synthetic
allocator events or mocked native methods are used. The host code registrations
are explicit controls and do not execute or emulate model dispatch/capture.

Source checks additionally verify the actual9d copy/clone call sites, no product
diff, no replacement dispatch or graph methods, and no added sync/facts/cap calls.
`host_window.py` separately adopts a real two-Linear CPU pipeline through its
supported components mapping. Three actual forwards traverse the ORIGINAL
automatic dispatch with no injected controller state or frame IDs. It verifies
selection of invocations2/3, following up1 closure, exactly-once body counters,
unchanged outputs, unknown CPU device/stream metadata, and nine released weakrefs
while the tracer remains alive. This CPU dispatch/window control passes. Actual
GPU capture/replay paths and Rust callback coverage remain unrun.

Actual host controls passed on original wheel4e70c846 plus helper00348b74e90.
See `validation.json` and the durable receipt directory it names. The helper
header is2551e703; the older helper002/header4ecd pair is preserved historical
evidence. The host container exited0 and is independently confirmed absent.

A prepared ORIGINAL9d bundle exists at
`/home/fidika/cozy_v2/outputs/xs138-27056-frontend-bind-20261103/bin-001`; manifest
SHA256 `1f1b6c685a3c8516cd5f69f465f9317c1c57f10040145b4a604d4230e3bf30c9`.
It is bound to immutable `ORIGINAL9D-HANDOFF.json` and remains GPU-unrun.
