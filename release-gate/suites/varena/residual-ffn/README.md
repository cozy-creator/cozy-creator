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
