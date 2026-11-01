# Actual int8 projection split consumer

This ordinary Cozy GPU app exercises the exact mma.sync path used by H3's
`split(attn.to_out[0], attended.flatten(2,3), Cut(1))`. It constructs the actual
`Int8ConvRotNativeLeaf` over real encoded CUDA tensors, calls public `author.split`
inside public `denoise_loop`, and verifies the executor's real Request identity.
It does not construct a fake Context/Request, monkeypatch dispatch or launcher,
or bypass a model package to claim full-model evidence.

The input is BF16 `[1,8193,7168]`, with 5376 output channels: H3's actual projection
widths and a bounded ragged row count. The unchanged ordinary whole NativeLinear
call supplies the numerical reference. Quality uses the existing native prebuilt
relative-L2 budget, `prebuilt.BUDGET=1e-2`; exact equality is reported, not required.

The app first joins the executor's real primed int8 opener, then records a strict
live-allocation baseline. It directly compares native query results with the
actual automatic GEMM allocator callback (full, half and small GEMV shapes).
Stream-K must actually request nonzero workspace. The public consumer must then
select whole/half/quarter row chunks and publish each leaf only after the genuine
eager phase completes. The limited capacity is obtained from native accounting
of each complete allocation vector, never from a Python size-class estimate.

A cap one byte below the legal output-only minimum must refuse before output or
scratch allocation with a typed no-retry error and invalidate the enclosing phase
even when that refusal is caught. This requires the paired native admission-failure
invalidation followup. A componentless phase cannot prove reusable named-model
parent identity; that remains a real-model gate. A real zero-delta LoRA hook then
checks both modes: baked leaves may use the closed adapter without calling the
factor getter; unbaked leaves perform their LoRA work on the existing path and
must not publish a closed projection sample. The hook is never removed to force
the supported path.

All tensor owners must return to the strict settled executor baseline. A following
cleanup entrypoint must reuse the same PID and find prior request keys already
retired. Process exit is not cleanup proof.

## Run and evidence

Build the new Runtime GEMM binary; an old binary lacks the required query. Generate
payload outside the frozen source from the exact Runtime and TensorFS wheels:

```bash
python make-payload.py /absolute/runtime.whl /absolute/tensorfs.whl \
  GPU-OWNED-UUID /absolute/results/payload.json
cozy run /absolute/frozen/linear-consumer/gpu --on OWNED_MACHINE --gpus=1 \
  --input /absolute/results/payload.json --json
```

The payload hashes all changed Python paths and the actual GEMM binary, plus the
TensorFS extension/shim. This fixture requires the actual SM89 route. Preserve
run receipts and the returned detail. Copy the payload and add `expected_pid` and
`keys` from the first result, then run `/cleanup` on the same machine. Require both
CLI completion and the assertions.

## Qualified GPU result: 2026-11-01

Runs **24925** (consumer) and **24926** (cleanup) passed in the same process,
PID **1694**, on the RTX 4090 / SM89. Tested fixture `55759abf0` used Runtime
`b2225f7f0` (`0.36.18+dev.gb2225f7f`) and TensorFS `0e05a2539`
(`0.17.16+dev.g0e05a253`). [The complete evidence](evidence/2026-11-01.json)
includes exact module/extension/shim hashes, GPU UUID and returned records.

| Actual query rows | Selected dispatch | Queried workspace | Actual workspace request |
| --- | --- | ---: | ---: |
| 8193 | GEMM config 5, Stream-K | 33,554,944 bytes | 33,554,944 bytes |
| 4097 | GEMM config 1 | 0 | 0 |
| 3 | GEMV | 0 | 0 |

Public `split` selected row chunks **8193 / 4097 / 2049**, with tails
**8193 / 4096 / 2046** and observed leaf backing **180 / 118 / 104 MiB**.
Each leaf published only after the genuine eager phase completed. Each phase
recorded one native choice. All three outputs matched the ordinary whole native
reference exactly (relative L2 zero; the declared acceptance budget remains 0.01).
The half-size choice needed 116 MiB of additional backing because it reused a
24,576-byte slot in an already-backed 2 MiB slab; its full leaf backing was 118 MiB.

The minimum case refused at **90,177,536 required / 90,177,535 available** bytes,
with zero output/scratch allocations and `varena_no_retry=True`. Its caught
refusal invalidated the parent with `split_select_error` and published no sample.
The baked zero-delta hook read no factors and published a native leaf; unbaked
execution read factors three times, used the existing path, and published no
closed projection sample. Both returned relative L2 zero.

Initial and final live bytes were exactly **17,039,360**, with two executor library
allocations, zero pending bytes and zero graph pools. The next request in the same
PID found both prior request keys already retired (`phase_forget` false).

Failed run **24923** remains preserved. Its operation was blocked by the fixture
calling `phase_forget` while its unknown-key parent phase was still active. Only
fixture cleanup moved after `denoise_loop` closed that owner; Runtime and TensorFS
were unchanged for the passing rerun. No assertion or quality threshold was relaxed.

AST/Ruff, actual schema import and offline lock checks also passed. This qualifies
the recorded operator, workspace, selection, completion, refusal, hook and cleanup
boundaries. The componentless parent is intentionally unknown and unpublished;
reusable named-model parent-key behavior, full H3 integration and performance
remain separate gates.
