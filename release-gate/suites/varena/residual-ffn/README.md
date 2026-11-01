# Complete H3 residual FFN proof

Development-only paired cohort: the candidate Runtime supplies `ResidualFFN` and its native
executor; the package supplies its original current `_Tail.run`, `_FFN` and three compiled
GLUE kernels. `prepare.py` copies four exact package modules with their original namespace
and hashes every source. No fixture kernel or numerical replacement is permitted.

The actual Cozy entrypoint creates real int8 NativeLinear modules, baked LoRA hooks, a real
Diffusers FeedForward/SwiGLU and norm, and runs the actual package tail through a real Request
and denoise_loop. Whole and constrained even chunks compare against the current full tail;
the odd row count exercises odd final lengths at aligned starts. Minimum refusal must precede
allocations/output writes. M=1 remains legal. A following entrypoint in the same PID verifies
request-owned menu retirement. This is not full-model quality or performance qualification.

Source and CPU checks do not establish GPU acceptance. Root owns wheel preparation, GPU
execution, exact identity input, and subsequent full-model matched package comparison.

The independent review found no concrete product or fixture blocker. A preexisting unrelated
Runtime `_RECENT` quantized operand remains outside-live: planned NativeLinear calls do not
run ordinary shared-operand retirement. This fixture starts without that owner and does not
claim equal cache retirement or performance for it. Actual full-model headroom remains a gate.

Product source candidates: Runtime `fe5a783da`, API-only control `1aca1d6cd`, package
`9e7490c0`. Freeze by running `prepare.py` with the clean package root, actual candidate Runtime
and TensorFS wheels, `key-check/candidate-v2.json`, exact GPU UUID, and a new output directory.
The resulting `project/` contains the exact four original package modules; `input.json` comes
from wheel bytes and kernel/source receipts. Both GLUE sets are hashed, and the GPU entrypoint
loads the package's actual device-selected set, as H3 does.

## GPU evidence: 2026-11-01

Tested fixture **3939db65f** with Runtime **fe5a783da**, TensorFS **9ccb36a24** and
package **9e7490c0** passes actual tail run **25084** and same-PID cleanup **25085**
on columbina RTX4090. Whole 8195, half 4098 + 4097 and quarter 2048 + final 3 rows
are bit-exact to current `_Tail.run`; M=1 has relative L2 zero. All 14 independent
automatic native workspace checks match (33,554,944 B for the large Stream-K
shapes, zero for 2048/GEMV shapes). Every accepted operation publishes one native
child observation and one parent choice. The componentless parent remains
explicitly `execution_key_unknown`; it is not fabricated as a model phase.

Minimum refusal required 2,097,152 B with 2,097,151 B available, changed no output,
allocated nothing and published no child. Final live 17,039,360 B and capacity /
mapped 20,971,520 B match the initial library-owned baseline; pending bytes, pools
and events are zero. Following request in PID 190641 finds both native site keys
already retired. Built-in baked hooks read no factors; unbaked, nonexclusive,
output-alias and unaligned-index declarations decline before output writes.

The unchanged standalone projection fixture also passes **25086 / 25087** with
the extracted helper: whole/half/quarter exact, actual automatic workspace exact,
baked and unbaked hook outputs exact, minimum refusal before allocations, matching
baseline and same-PID retirement. Full structured results and original run
records are under `evidence/2026-11-01`; `independent-review.json` records hashes
and qualification boundaries.

A later fixture-only assertion now refuses to execute the minimum case if an
incoming slab offers fully reusable scratch (`required_bytes == 0`). The tested
3939 artifact proved a positive 2 MiB requirement and is preserved unchanged.
This assertion changes no product source; its source checks pass, and the GPU
claim remains for 3939. Full new-package H3, other GPUs/providers, captured tails,
post-acceptance fault injection and unrelated incoming quantized-cache ownership
remain outside this proof.
