# Exact shallow-copy compatibility: CPU proof only

Source feature e093f208f3c0b1018519b9a545038cf1a15017f2 is a five-line exact-operator
fast path, independently clean-applicable to frozen0af and preferred auditedb40f.
No old public/native snapshot is an adoption dependency. Original26475/GPU traces
remain preserved; no GPU/machine/cache owner was modified.

The real CPU baseline reproduces inference-mode compatibility dispatch under _rebind.
Tiny VAE two casts:184 queries,368 Version.tensor calls. Fixed:184 queries,zero
Version.tensor/reservation/metadata-Tensor/execution-scope calls beneath the query.
All40 actual native compatibility/error comparisons match ordinary Torch in both
modes and operand orders, including false meta/COO/CSR/quantized cases. Real casts,
tied/interior identity, escaped old Versions and exact VAE decode pass.

Nine actual CPU/native-metadata clients pass on owned auditedb40f SDK/native artifacts
plus current Plan/graph/alias Python overlay and only this compatibility source hunk.
These are real model/ABI operations with profiling, not injected/native substitutes.
The native metadata host guards do not prove valid CUDA readiness, consumer completion,
physical reclamation, same-VA graphs or GPU Source error handling.

No GPU reservation, packing, speed or event reduction is qualified here. Coordinator
must run the same Python-only overlay/trace/model counter boundaries. Final downcast
Versions can correctly stay unreserved until semantic intent refresh. No forced66
mapped VAE groups, source-cache policy, byte policy or fence weakening was introduced.

Commands/source identities/return codes are in metadata-final-manifest.json; original
red/green logs and all earlier receipts are retained. Sparse/quantized warnings come
from constructing real negative controls, not adapter failure.
