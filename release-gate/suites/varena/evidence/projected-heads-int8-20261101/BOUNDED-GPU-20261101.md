# Coherent INT8 Heads and phase Plan bounded qualification

Independent audit passes for Runtime candidate `3daaaeda979ca3da4b21647893dfcfa508e6cf29`, TensorFS `4dcf26183fafe6bd160a52856afd057eae43d184`, package `f1e8d2bbea1227a9035f6dec505cd2aba95456b5` and frozen fixture `31a53afb54d1ad255dd189607423d98f875f3323` on hitoshiki, GPU UUID `18d960a8-2283-19c9-c060-ef8a22fabe35`.

INT8 run **25314** and same-PID cleanup **25315** (PID 1065) pass all 17 cases. The three reference geometries execute the actual opaque `comfy_kitchen.int8_attention.default` operation from the pinned 0.2.40 Python source; the loaded native extension hash is `a82d02945ebb4e00b1bab815dd7b3f89209b45d002eb4c5f3cabe2118c048283`. All 25 output/seed comparisons are exact, relative L2=0, under the unchanged 1% budget.

The seven menus cover 8195 tokens at 56/28/7/1 heads, 513 tokens at 56 heads, and odd 55 heads whole/28+27. Every actual requested size and allocation class agrees with the independently enumerated buffers: 15 allocations except head 1's 14 (no GEMM workspace). The final same-device transpose copies and seed updates add zero allocations. Both CTA 64/128 and strided tail output are exercised.

Three minimum refusals allocate zero and preserve the seed. A distinct managed-capacity refusal occurs after the second real seed copy and first completed output chunk: the first output head is exact, readback adds zero device allocations, no legacy fallback runs, and the leaf observation is invalidated as `allocation_error`. This is an intentional instrumented failure, not a spontaneous kernel error.

Five complete processors cover direct calls at both token lengths, actual installed Sol exact and prefix-heavy sites, and an active native Plan. The planned call selects generation 1, preserves its READY 2 MiB Region, publishes both native Heads and output-projection observations, and preserves the sentinel. Explicit reclaim releases 2,097,152 bytes immediately afterward; reserved/resident/pinned bytes all return to 0. This proves generation forwarding, not file-backed model-weight inventory (qualified separately by the phase-plan fixture).

Working memory returns exactly to its nonzero library baseline: 17,039,360 live bytes and 20,971,520 mapped/capacity bytes, with pending/pools/event handles 0. Same-PID cleanup verifies all 7 Request keys retired.

The updated generation-forwarding regression cohorts also pass:

- Standalone projection **25317/25318**, PID 1219: six cases, three real native query checks, all numerical comparisons L2=0, refusal-before-allocation and same-process key retirement.
- Complete Tail **25320/25321**, PID 1354: five cases, fourteen real native query checks, all numerical comparisons L2=0, unchanged output on minimum refusal and same-process key retirement.

Both regressions restore the same working baseline and retire their two Request keys. No executable, native or packaged artifact was changed for this qualification.

This is bounded functional, allocation and lifetime evidence. It does not establish full-model adoption, performance, source-transfer improvement or full-clip quality. The matched full H3 pair remains a separate gate. Raw reports are under `outputs/varena-v2-p0-20261031/p3-shortage-int8-leg15-plan/attempt-001/`; reproducible audit scripts and exact file hashes are in `independent-gpu-audit-25314-25315.json` and `regression-audit-leg15.json` beside this note.
