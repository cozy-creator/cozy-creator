# Why run17776 has no denoise_native_steps row

Read-only source tracing; no code changes. Relevant Creator paths are unchanged between
installed TensorD4178a2a11 and the canonical branch at inspection.

The metric is lost at TensorD's existing receipt/live projection, not because the package
uses another metric API or because four visible rows imply ring overflow:

1. SDXL `_prepared.py:124` emits `tel.metric("denoise_native_steps", completed_steps)` after
   checked native completion. Runtime `author/_services.py:1421–1424` admits it against the
   metric budget and adds a `kind="metric"` observation to the Attempt EventRing.
2. `Attempt.emit` sends the stamped observation to the live progress sink and retains it.
   Runtime's final Invoke reply includes `record.ring.rows()` plus cap counters
   (`internal/executor.py:3548–3549`). The Runtime-side bounded ring therefore has an explicit
   transport for such a metric; metric names are not filtered at this boundary.
3. On the live path, `protocol/src/executor.rs:448–455` recognizes only progress/log Kind;
   metric decodes as Unknown. `Frame.log` accepts Log only (`:657`), and `calls::observe`
   returns when the non-log frame has no stage (`tensord/src/calls.rs:533–546`). The metric
   therefore does not become an ordinary CLI event/log row.
4. On final measurement persistence, `tensord/src/gpu_service.rs:3716–3722` selects observation
   names beginning only `attention.`, `gemm.`, `kernel.` or `request_path.`. The generic
   `denoise_native_steps` name is excluded. This exactly explains run17776's four retained
   attention confessions at seq20–23.

Thus neither retained ordinary event stream nor run-show measurements proves the value of this
specific generic metric. No alternate durable generic-metric destination was found on the
reviewed successful-run path. `record_invoke` records memory/allocator metrics, not all package
observations; failed-run triage is a different boundary and does not recover this successful run.

The narrow future decision is to expose a bounded direct execution observation through an
existing measurement field or preserve the relevant metric in the existing observation
projection. Do not infer that a new telemetry subsystem is needed, rename package metrics into
an unrelated kernel namespace merely to bypass the filter, or mutate the retained proof to
contain a value that was not recorded. Root owns any design/source follow-up.
