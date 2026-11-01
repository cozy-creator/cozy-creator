# Actual phase-region intent proof (unrun source)

The fixture uses the executor's actual Request/Memory arena. Native cases compare
currently resident weights with the full copied Region set at two managed capacities,
exercise generation refusal before execution, alias deduplication and explicit
retirement. Runtime cases use an actual file checkpoint and NativeLinear consumer;
the first incomplete pass is unplanned, and subsequent established regions activate
the native intent without materializing all weights. No working execution key or
eager-prefetch eligibility is claimed.

Root supplies strict actual wheel/module/GPU identities and owns GPU execution.
No mocked CUDA owner, source or fabricated Context is allowed. Source/CPU checks
are not GPU acceptance; full model no-worse gates remain open.

Run `runtime` first, then `lifetime`, then `primitive`, followed by `cleanup` using
primitive's exact returned PID/keys. The cross-arena exclusion case is last because
foreign reservation teardown follows native ownership. `runtime` verifies its model
keys are retired at model close; it does not invent request-owned keys.

`lifetime` runs real native queries on another thread while the last external Plan
owner disappears. Its 64 reservations have no separate Region owner. The fixture
waits for actual native intent/reservation retirement and records progress; this is
bounded concurrent stress, not proof of every scheduler interleaving. No error or
kernel is mocked.

CPU source proof loaded all 33,587,200 stored bytes through an actual checkpoint and
Runtime hold into eight genuine NativeLinear modules, with CUDA uninitialized.
Native source+binding cargo checks and paired Runtime mypy11→11/Ruff12→12 pass with
no new findings. GPU proof is still pending. Use `prepare.py` after fixture freeze
with the actual Runtime190fe and TensorFS4dcf wheels and exact GPU UUID; it writes
an immutable project/input/provenance set in a fresh output directory.

## GPU mechanism evidence — medusa, 2026-11-01

Exact native4dcf / Runtime190fe / fixture281c passes **25281–25284**, independently
checked against the actual wheel/module/GPU hashes. Real file-backed NativeLinear
passes choose8193 rows at224MiB and4097 rows at208MiB, beside48MiB actual weight
backing. All eight leaf observations publish each pass. Warm passes add zero fills,
store bytes or host bytes; working cache is explicitly trimmed, so this proves
weight/source retention rather than activation-cache timing.

The native primitive declares32MiB of weights while only4MiB is resident. At the
same70MiB cap, unplanned whole32MiB scratch leaves four regions resident; planned
half16MiB scratch retains all eight. At86MiB the planned whole choice retains all.
Phase-change and retirement tokens refuse before allocation; capture/cross-arena
exclusions pass. Source revision and a real failed Fill retire intent while the
escaped failed alias remains valid. Model keys retire at close; a following request
in PID169520 finds the primitive key already retired.

Concurrent last-owner selection/drop completes three native queries and retires64
reservations. This is observed stress coverage, not proof of every scheduler
interleaving. Final live17,039,360 B / capacity and mapped20,971,520 B match the
existing library baseline; pending/pools/events are zero. Numerical assertions pass
within the existing prebuilt budget; bit identity is not inferred from missing
numerical fields. Full raw results and independent hash audit are committed under
`evidence/2026-11-01`.

No full H3 performance/quality, multi-arena ordering, eager-prefetch eligibility,
smaller-card or complete P2 qualification follows from these mechanism cells.
