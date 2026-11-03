# Frontend readiness with checked resource retirement held: prepared only

Feature db597b327e05df365edbe8a932ae508f6156ce2a adds a coordinator GPU cell rather
than changing product code. The existing source_failure frontend cell is an immediate
prepare error. It must not be cited as proof that the frontend waits when bytes are
Ready but selected Job resources have not finished checked retirement.

The new graph_retirement_wait_gpu.py uses only the existing real public C Source
provider and its mapped gate/Job retirement mutex. It first publishes known bytes,
completes the actual producer/consumer stream and observes real Job VR_BUSY while
retirement stays held. A distinct graph-construction Use then selects those Ready
Regions. Python profiling observes the actual public wait_ready method, without any
method/resource substitution; no warmup/capture body may enter or constructor return
while retirement is held. Warmup/capture bodies additionally assert the existing
helper's real CPU-atomic job_dropped count. No CUDA call is added by that observation.

The independent positive-control export/Use closes normally while capture waits.
Main-thread finally releases the actual mutex on its acquiring thread and joins the
constructor. The child must then capture, classify Pins0 and replay exact output without
re-running the authored model. Successful cleanup uses ordinary graph/Plan/Source close
and preserves any exact failed owners through the product's existing mechanisms.

THIS RECEIPT IS NOT A GPU RUN: checks are Ruff, format, Python syntax/AST/order,
actual matched-SDK --help import/argument handling, and unchanged product/native/provider
source. No mocked GPU/native owner, CPU retirement substitute, native build or model
suite rerun. Python method entry does not prove native C ABI admission timing. This
cell does not exercise late query/retirement errors, selected retirement on replay/refill,
full CUDA failure cleanup or consumer/device success. The deadline is an explicit
whole-child protocol, never elapsed-time-as-failure for an unbounded model execution.

Coordinator owns ordinary-Cozy execution, exact SDK/provider/GPU identities, whole-child
deadline and cleanup. Commands and scope are in GRAPH-COZY.md. No machine/rental/native
policy/merge/release changes occurred. Earlier proof and failed receipts remain preserved.
