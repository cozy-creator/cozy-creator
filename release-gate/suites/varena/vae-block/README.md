# Real fused VAE allocation proof

This fixture will first qualify the prepared cuBLASLt provider against the actual six
biased dense projections of MiniMaxH3VideoTransformerBlock, then the installed fused
block and selected Kitchen INT8 attention. Runtime candidate source is tracked in
Runtime #2028. No GPU qualification is claimed by this initial source checkpoint.

Preserve FP16 autocast, both FP16/FP32 incoming residuals, FP32 norms/gates and the
existing kernels. Query actual full/ragged geometry before admission, allocate every
caller buffer before writes, and retain context/bias/workspace owners through completion.
Minimum refusal must precede writes; accepted failures must not replay application code.
Library internal resources remain separately observed, not declared closed by warming.

Root owns every native build, installation, GPU invocation and rental. Fixture source
and strict payloads are frozen independently of the product artifacts.


The first executable entrypoint is `dense`, with31 cases:24 actual nn.Linear outputs
(six genuine block projections, FP32/FP16 source parameters, tile batches1 and3),
two native managed minimum refusals, two invalid-capacity binding refusals, two real
post-write allocator refusals, and one retained-context close refusal. The reference
is the actual Torch module under FP16 autocast; all casts and execute-buffer counts
are observed. A stock biased linear prepares Torch's retained library workspace
before the baseline, with both snapshots recorded. This does not establish closure
of library-internal allocations; separate driver evidence is still required.

The minimum cap is installed inside the actual Loop callback after its own setup.
The late-failure cell deliberately requests more than the hard managed cap after
the real GEMM has written output. It checks one execute call, numerical output and
preservation of the prior successful sample. This is an instrumented failure boundary,
not a claim that production needs an undeclared post-write allocation. The full
failed report is preserved even if final cleanup also fails. Context/descriptor/bias/
workspace owners survive through actual stream completion; later same-PID `cleanup`
checks Request-owned key retirement.

`prepare.py` freezes exact clean source and derives strict module/extension/shim
hashes from the actual Runtime/TensorFS wheels, including `_dense.abi3.so`. It accepts
an explicitly unbound GPU UUID before rental. All native build, link and GPU checks
remain pending. The complete fused-block entrypoint is the next implementation step.
