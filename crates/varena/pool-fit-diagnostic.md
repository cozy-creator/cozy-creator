# Diagnostic-only capture pool fit facts

This feature is separate from frozen production SDK25798054. It is intended only for the next selected reproduction of run26862. It adds no public API, CUDA query, wait, growth, retry or sizing decision.

`fit_diagnostic::runs` is the existing ordered segment/block scan and eligibility predicate. The real allocator still selects capacity>=the rounded request and prefers a strictly smaller candidate, keeping its prior tie/iteration order. The complete successful mutation tail is byte-identical to257. Only `best=None` emits `VARENA_POOL_FIT_FAILURE_V1`, while the same Working lock remains held; stderr refusal cannot replace the original None/OutOfMemory outcome.

The JSON contains the actual requested/rounded bytes, pool ID, requested stream, mapped pool capacity, sum and largest contiguous run eligible under the existing predicate, live capacity, blocked capacity including live blocks, and the pool's routing/prepared/reference/native-capture-guard fields. The latter is native ownership metadata, not a new driver capture query.

At most64 actual block ranges and16 stream handles per range/pool are printed. Total counts and truncation are explicit; aggregate capacity and largest-run figures scan every block. Each retained range includes actual life, ready/ordered predicates, origin stream, origin-retired flag, reusable stream and consumer/event counts. Inactive space is not silently treated as reusable: only the allocator's exact eligibility contributes.

Host `cargo check -p varena --locked --offline -j1` passed. GPU execution is unrun. Bind a distinct diagnostic SDK/core and matching helpers, preserve the original26857/26862 receipts, and retain the original Python workload/limits/assertions. Do not promote this report as a production policy or change reservations from its numbers without establishing the actual cause.
