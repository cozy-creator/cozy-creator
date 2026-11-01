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

AST/Ruff and offline lock verification are source checks. GPU execution, complete
query-versus-run parity, real H3 integration and performance remain pending.
