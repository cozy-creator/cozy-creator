# Exact RMSNorm and first-use output projections

The exact RMSNorm and first-use output-projection cohorts below have bounded GPU qualification. Full-model performance remains separate.

`norm` runs the actual Torch RMSNorm installed under the package's real
`_AdaLNPrunedOutputTable` topology. It checks BF16 at32768×5376 and FP16/F32 at513×5376.
For each, an actual Runtime Loop executes the native whole-operation adapter, a
passthrough Torch dispatcher records the two real allocator requests (output and FP32
rstd), and the complete result is compared to the ordinary Torch operator. The dispatcher
preserves both actual inference and grad TLS. A native-derived managed cap then refuses
the complete minimum before output allocation, and the earlier measured backing sample
must remain available. No fake OOM or copied byte-policy decision participates.

There is deliberately no RMS split menu. Whole uses336MiB+128KiB at the largest H3
output chunk; splitting in half retains504MiB+64KiB. The actual150 prior calls were all
whole, despite Python advising1,409,286,144 bytes. This migration deletes that estimate
and retry path while preserving the existing kernel and math.

`projection` invokes the actual registered Runtime Processor for both dense and sparse
bootstrap H3 calls. An independent ordinary recipe executes on a separate real module
with identical deterministic weights, so it cannot teach the candidate's sibling group.
The candidate begins and ends with `_cozy_siblings` absent, while its fresh normal-tensor
attention output is consumed by one declared native projection. The actual Heads and
projection observations must both publish. Explicit `quantize_once` sharing and a
multi-consumer group learned from two real ordinary calls must still decline the new
path. No test assigns a sibling count. A later real input retires the learned operand
marker before exact allocator-baseline comparison.

Both cells preserve complete partial reports on ordinary errors, report all actual
request keys, and require exact working allocation/capacity/mapping/event/pool baseline
restoration. `cleanup` runs in a later ordinary request in the same PID and confirms
those keys were already retired. Numerical comparisons retain the existing provider
budget and also report byte equality; no full-model quality or timing claim is implied.

`prepare.py` freezes the six exact package modules and derives strict version/module/
extension/shim hashes from the real Runtime and TensorFS wheels. It requires the observed
GPU UUID (or an explicitly unbound template before rental). The package kernel-identity
receipt comes from actual CPU import of GLUE, GLUE_FP8 and STATS; all entries remain
identical to frozenfc4af. Root owns builds, installation and all GPU/rental actions.

Source evidence here includes the pinned Torch allocation implementation URLs/hashes,
full25405 call inventory, CPU construction/normalization with CUDA uninitialized, and
the redispatch TLS reproduction. The named GPU cohorts below now supply native CUDA census, output/refusal and cleanup evidence.


Run25583 showed that the fixture's manual processor installation omitted Sol's actual
module-forward site wrapper. The common constructor now validates the real module and
installs the real site before the producer, matching production attention._install_sites
for both reference and candidate. No private context variable is assigned and no Sol
contract is bypassed. CPU construction verifies the registered component/path and actual
forward wrapper; GPU projection qualification remains pending. The separate RMSNorm
cell and same-PID cleanup passed25578/25582 on136/a96.


Run25718 passed the first dense native and explicit-sharing comparisons (both exact),
then the learned-group setup incorrectly reused the preceding module. That explicit
sharing fallback actually chose legacy split level3; its window views can teach a
single-use group. The learned-group case now constructs a separate real module, asserts
an absent initial group, and retains both ordinary reads and the exact group2 refusal
checks. The fixture explicitly creates normal tensors with inference disabled and
grad disabled. No learned flag is assigned or guard relaxed; both records remain.


## Qualified bounded cohorts

RMSNorm25578/cleanup25582 pass on Runtime136/TensorFSa96: the three actual
BF16/FP16/FP32 whole outputs are exact, with exactly two allocator requests each
(output plus FP32 rstd), and the three minimum refusals preserve prior measurements.
The independent receipt records the actual byte requests and PID121066.

Projection25733/cleanup25734 pass on Runtime3043/TensorFSa96/package5d27/fixture700.
Both first normal-tensor dense and sparse-bootstrap processors publish actual native
Heads and declared-single-use output projections, with zero factor reads and exact
outputs. The four explicit-sharing and genuinely learned two-consumer cases retain
zero native selections and zero output relative L2. No sharing flag was assigned.
Three request keys were already retired in the subsequent request in PID232308.
All six cases restore17,039,360B live/20,971,520B mapped, with pending/pools/events0.

The source/input hashes and independent checks are under evidence/gpu-20261101.
Original failures25583 and25718 remain in the run receipts. This proves the bounded
513-token dense/sparse-bootstrap consumer and ownership boundaries; rolling sparse
projection and full-H3 performance remain separate qualifications.
