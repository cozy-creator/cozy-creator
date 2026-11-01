# Preserved Runtime eager ownership experiment

An ordinary Cozy GPU App uses its injected Context, executor Request and public
`denoise_loop`. Its guard token is issued by the actual Loop observation; the
fixture never constructs a Request, fabricates a Placement or sets completion
flags. A lease opened by the step stays open until the Loop completes or aborts.

It covers empty metadata owners, imported LM with no live engines, actual graph
domain/cache/binding owners, concurrent preparation and request registration,
and real pending work on aborted or changed completion streams. Creation
refusals must precede allocator/driver work. Retirement must prove queued work
complete before releasing the claim. The settled executor's library allocations
remain in the strict before/after live baseline.

This guard is policy-neutral and **not acquired by production**. Root's subsequent
source review selected a smaller window-removal cutover that keeps existing
native pin/loading/fence and graph-pool protections, without this ownership-mode
dependency. This source is preserved as an unqualified experiment, not a required
step for that cutover. It does not claim H3 component/Placement eligibility or
full-model execution: that must use the actual package and executor construction.

No GPU run has been performed. Syntax, fixture Ruff and offline lock checks pass.
The paired Runtime guard is draft #1992, tested source to be supplied by root.
Generate provenance from actual wheels outside this frozen source:

```sh
python make-payload.py /absolute/runtime.whl /absolute/tensorfs.whl GPU-UUID /absolute/payload.json
cozy run /absolute/fixture/gpu --on OWNED_MACHINE --gpus=1 --input /absolute/payload.json --json
```

Inspect both the run status and result `passed`. Retain any original failure and
its exact cohort before changing a fixture or product; no mocked GPU result is
accepted.
