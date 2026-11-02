# Public native asynchronous Source

Owner /root/async_integration_review, branch codex/xs138-p4-public-source, based on
fetched master0888d1026 plus coordinator d00fa7042. Root approved the contract; this file
records the implementation scope. External P4 worktrees and api.py remain untouched.

- Keep common ABI1 functions. Remove old vr_v1_region_new and by-value vr_source_v1;
  use distinct vr_v1_region_from_source with a const vr_source_provider_v1 pointer and
  exact size. The new per-fill object is vr_source_job_v1. No compatibility adapter.
- Prepare gets no destination and enqueues no GPU work. It returns a fully owned job
  with immutable fixed producer streams on success. Failure transfers nothing: the
  persistent provider retains every failed-preparation resource for checked retirement.
- Core creates native Fill custody before exposing dst. Submit returns only after all
  host submission stops. OK means every byte was initialized/enqueued; a closed error
  aborts. Panic/unproven closure quarantines the actual owner set.
- Job and persistent provider retire return OK only when inert or ownership safely
  transferred, BUSY for retained pending state, and a permanent error for quarantine.
  Their final drop callbacks are host-only, inert and never wait for the GIL.
- Actual cleanup uses the existing typed per-device native queue. The ABI queue only
  observes host Source/Fill tickets, so retirement_stats cannot hide pending/failed
  Source work. No extra public ticket functions or memory-policy callbacks are added.
- No Weight/ABI/capture mutex crosses a provider callback. Core Region state supplies
  exclusive Fill admission; explicitly ordered Pins use prepare_on/order_on.
- Rust exposes unsafe provider/job contracts with retained stream owners, owned failure
  handling and panic containment. A normal Rust Drop must preserve unreleased GPU owners;
  checked retire is the only GPU cleanup path.
- First Python adoption is a separate native TensorFS-backed adapter coordinated with
  core_restart. No raw Python CFUNCTYPE Source callbacks are introduced. Existing host
  bytes remain their safe checked copy+commit path until a replacement is qualified.

Asynchronous failure is observed through existing operation errors and retirement_stats.
Every Fill has an internal metadata observer before begin, including a Prepared/Writing
failure; ordinary closed Source errors may clean up successfully, while quarantined
producer/resource failures stay observable. Persistent provider cleanup has its own host
ticket and cannot silently disappear when the last Region/Use/Plan drops.

Source/header and real fixtures are required before GPU qualification. Root owns GPU
execution and rentals. Keep the PR draft; no merge or release authorization.
