# Real Runtime eager RAW-prefetch cutover

An ordinary Cozy GPU app uses its injected Context and actual executor Request /
Memory, creates a real TensorFS checkpoint, and loads a two-block model through
Runtime's existing `Declared` / `Weights` / `Memory.load` seam (the stock-memory
suite's source contract). Each block is one fixed contiguous FP32 elementwise
addition over a 4 MiB weight. No Context, Request, reader or launcher is mocked;
no existing model package is bypassed to claim full-model evidence.

The normal `denoise_loop` case has an unknown entry key: native admission must
record a decline, issue no prefetch, and ordinary demand must read both blocks
and produce the exact expected output. Separate fixed-program cases explicitly
enter a native phase whose complete shape/dispatch identity is known before
execution, synchronize the real stream, then mark completion. Its first pass
trains a sample but cannot prefetch; its second pass must exercise Runtime's real
next-block hook, native read admission, checked settlement and demand reuse.
These explicit fixture keys do not claim that adaptive production Runtime loops
already provide committed entry keys.

Additional real mechanism cases use the existing APIs directly at quiescent
boundaries: a queued-validity cancellation after admission but before source IO,
the actual idle host-page pinning path followed by reclamation and demand, and a
real scale-by-two fill callback. Kept-host/transformed speculation must decline;
ordinary host demand and fill callbacks must still work. Model close must retire
its measured phase key, and CUDA allocations must return to the settled executor
baseline with zero pending bytes, graph pools or resident weight pages.

The executor's real int8 readiness boundary is joined before taking the baseline,
so its asynchronous library probe is not confused with fixture-owned allocations.
The payload binds Runtime's residency source, SDK versions, TensorFS extension/
shim and the owned GPU UUID. Generate it from exact wheels outside frozen source:

```bash
python make-payload.py /absolute/runtime.whl /absolute/tensorfs.whl \
  GPU-OWNED-UUID /absolute/results/payload.json
cozy run /absolute/frozen/eager-prefetch/gpu --on OWNED_MACHINE --gpus=1 \
  --input /absolute/results/payload.json --json
```

## GPU qualification: 2026-11-01

Run **24961** passed all seven cases, PID **47629**, on RTX 4090. Tested fixture
`d6c3b91eb` used Runtime `c05c3ac32` (`0.36.18+dev.gc05c3ac3`) and TensorFS
`0e05a2539` (`0.17.16+dev.g0e05a253`). [Complete evidence](evidence/2026-11-01.json)
includes exact source/extension/shim hashes and GPU UUID.

| Case | Observed boundary |
| --- | --- |
| Unknown Runtime entry | One attempt/decline, zero admissions/starts; 8 MiB demand reads; no parent publication |
| First fixed-program pass | Successful 8 MiB sample, no prefetch |
| Second fixed-program pass | One admission/start/completion; 8 MiB total source bytes including demand reuse |
| Queued invalidation | One admitted Fill canceled before start; zero source bytes; subsequent demand correct |
| Kept-host source | No native attempt; demand consumes 8 MiB host bytes with zero store-byte growth |
| Transforming source | No native attempt; real fill executes twice during demand |
| Model retirement | Named phase key already forgotten when independently queried |

Initial and final live working bytes were exactly **17,039,360**, with two executor
library allocations, zero pending bytes and zero graph pools. The successful app
also asserts correct outputs and zero resident weight pages; those assertions are
not separate returned fields. The receipt audit independently checks all reported
counters and lifecycle records.

AST/Ruff, schema/import and offline-lock checks passed. The known entry key belongs
to this fully specified program, not the adaptive production Runtime loops. This
is Runtime IO/admission/ownership proof. The separately named full H3 arm remains
the quality, denoise-time and H2D/store-traffic acceptance gate; losing speculative
overlap may still regress performance.
