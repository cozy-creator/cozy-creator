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
