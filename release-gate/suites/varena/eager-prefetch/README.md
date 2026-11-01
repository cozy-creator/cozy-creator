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

Source/static checks only so far. No GPU, IO-admission success or performance
qualification is claimed. The separate full H3 arm must compare complete workload
quality, denoise time, H2D/store traffic, prefetch counters and cleanup against the
frozen b222 cohort. Declining unknown speculation may lose overlap; it is not a
prefetch performance improvement by itself.
