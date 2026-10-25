# Native BF16 normalization

Native CLI proof passed on 2026-10-25: basic 18163 and cancellation/resume 18164,
with exact observed-cohort restoration 18165. [Retained evidence](evidence/f8e6-18163-18164.json)
pins source `f8e6a7a4f`, fixture `ce8c801ab`, wheel SHA256 `cc264f59830342ebb3e7a68192879d711f2f3ce9058eb8ed995fe177d82d6ae3`,
Creator 51a and Runtime 237. This is native Writer proof; the supplied Runtime BF16
adapter was not installed into these runs.

All 42 cases passed exact byte/statistics checks, including all 63,488 finite F16
patterns, scalar/rank4/empty and rowwise inputs. Nine numerical/unsupported-input
refusals and seven preflight refusals passed. Unchanged reuse performed zero reads
and writes, with unknown reused quality. Independent normalization/quantization
journals survived reopen and refused both directions of pending-key overlap.

The 42-tensor calls took 9.034ms/8.693ms under 16KiB/64KiB working grants, reading
135,253 stored bytes (146,404 logical source bytes) and writing 141,458 bytes.
Cancellation joined in 0.115ms; resume read 64MiB and wrote 32MiB in 0.689s with a
262,136-byte reported working requirement. Process high-water RSS was80,302,080 /
85,282,816 bytes during the basic calls and153,329,664 bytes before/after resume.
Those process high-water values include fixture/reference/store work; they are
not exclusive conversion peaks. These are correctness/cost samples, not general
model conversion or throughput claims.

The separate integrated Runtime cohort also passed ordinary CLI **18221** with
Creator/TensorFS `4528e73a4` and Runtime `3c3ab093d`; restoration **18222** verified
the actual observed SDK and ready/idle state. [Integrated receipt](evidence/4528-3c3a-runtime-18221.json)
records all artifact hashes and the actual `write_bf16`/Output path. It preserved
42 selected conversions and the legacy146,404 logical-byte count while separately
reporting135,253 stored bytes. It wrote141,458 bytes with a512,000-byte native bound;
the public adapter call took0.140s, including worker/Output overhead. Process lifetime
RSS high-water rose86,196,224→86,458,368 bytes, not an exclusive conversion peak.
After close/reopen, all42 keys reused with zero reads/writes and unknown quality;
quantization then committed in the same Output. Finished reuse passed and a changed
normalization selection refused. The standalone native cancellation/reservation
receipts above remain separately scoped to their exact earlier artifact.

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
peak; byte counters are not physical disk or PCIe traffic. Native and integrated
Runtime receipts are linked above. This is a CPU conversion gate, not performance,
model quality, GPU serving, deployment or release qualification.
