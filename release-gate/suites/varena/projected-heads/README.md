# Actual H3 projected-head regression

The fixture constructs actual package `_Heads`, encoded Q/K/V and output projections, real Rows/baked hooks, compiled norm/RoPE and key-mean entries, and the installed INT4QK ABI inside an actual Runtime Request/phase. It exercises whole/28/7/1 heads at 8,195 tokens, plus 55 heads split 28+27 at 513 tokens, covering both CTA-K geometries and strided output tails.

The instrumented mechanism proof records real native allocation counts and capacity classes for forwarded Torch operations while the accepted leaf is active. It compares these with independently enumerated allocations and checks that seed division/copy and baked hooks allocate no additional device storage. Full processor calls verify seed bookkeeping and the existing standalone output projection.

Minimum-admission refusal must precede all output/seed writes. A distinct failure cell synchronizes after the second actual seed copy (and first output chunk), then requests a real allocation larger than its managed limit. It proves the observed partial writes and that no legacy/backend fallback runs after acceptance. This is explicitly instrumented functional evidence, not a timing benchmark. Each executed case logs its measurements before assertions; a second same-process `cleanup` request verifies request-owned native keys retired.

`prepare.py` freezes the package modules and creates a flat Cozy input from exact wheel bytes. CPU construction and static checks pass; GPU qualification is pending. Frozen native base is the independently qualified bounded-trim artifact b20f699c; this branch changes no allocator source.
