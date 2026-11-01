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
