# Native BF16 normalization

Use the installed default-home Cozy CLI only during the primary Varena agent's
coordinated validation window. These entrypoints run CPU arithmetic through the
ordinary scheduler; `--gpus=1` is the agreed GPU cap, not GPU numerical proof.

```sh
cozy run /absolute/path/to/bf16-normalize/run mode=basic seed=103 --on local --gpus=1 --json
cozy run /absolute/path/to/bf16-normalize/run mode=cancel seed=103 --on local --gpus=1 --json
```

After the Runtime BF16 adapter is installed, run `mode=basic runtime=true` with
the same CLI flags. The native candidate must expose `Writer.normalize_bf16` and
the existing explicit FP8 producer. Runtime mode also needs the additive
`Output.prepare_bf16` and BF16 source-read counters.

`basic` constructs real stored checkpoints and compares BF16 bytes to frozen
Runtime9bf8b6b1f carrier conversion, independently widened rowwise payloads and
the historical F32 subtraction/F64 error accumulation. It covers positive and
negative halfway ties, signed zero, scalar/rank3/rank4/empty F16/F32 tensors,
all63488 finite F16 bit patterns, and27 FP8/I8 rowwise combinations across three
scale dtypes and three logical source dtypes. Four empty rowwise tensors verify
metadata handling without source reads. Two workspace grants prove byte and
resume-identity invariance. Fixture arithmetic never imports current native
normalization or production Runtime conversion as its numerical oracle.

It refuses malformed resolved selections, wrong source dtype/shape, duplicate or
missing keys, unknown fields, insufficient output/workspace grants, nonfinite
source values, BF16 overflow, isolated F32 subnormal quality loss, unsupported
plain BF16/tensorwise/rank3 rowwise sources and F32 rowwise reconstruction
overflow. Every failure leaves the selected tensor unpublished. A no-op selection
does not bind a later conversion. Changed source/full selection refuses on resume.
Mixed normalization and quantization bind separate journal identities; disjoint
keys survive reopen in either call order, and overlapping keys refuse.

`cancel` creates one64MiB F32 fixture from a1MiB window, waits for a real first
tensor completion and actual reads into the second, checks busy commit/abandon
refusal, cancels and joins, reopens the journal and finishes the remaining tensor.
The completed output is checked in1MiB windows. Before BF16 resumes, FP8 conversion
of the unwritten second key must refuse the other operation's persisted reservation
with zero source progress. A second real interruption reverses the operation order
using the same stored fixture; BF16 cannot take FP8's unwritten reserved key. Both
reservation probes happen after Writer reopen. A missed cancellation window fails
the gate explicitly. No model files are downloaded or generated on the controller.

`runtime=true` uses the real Runtime Output and write_bf16 adapter with the actual
request Context/Telemetry. It checks historical selected-count/read-count behavior,
actual stored-source reads separately, interrupted all-keys-present resume before
quantization, finished identity validation and unknown reused measurements.

Results retain exact native/oracle hashes, completed source/logical/output byte
counters, declared/required conversion workspace, wall/process CPU durations and
process lifetime `ru_maxrss` before/after. RSS is not a per-operation allocation
peak; byte counters are not physical disk or PCIe traffic. No native execution
receipt exists at introduction. This is a CPU conversion gate, not performance,
model quality, GPU serving, deployment or release qualification.
