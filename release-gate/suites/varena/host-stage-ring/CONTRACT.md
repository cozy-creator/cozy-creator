# Bounded frontend host-stage ring for 27056

Status: implemented diagnostic for review, with bounded host/source controls. Product/kernel/GPU behavior is not qualified by this document.

Owner: external P4 session01a198c8-91aa-7a31-8e38-1e6d91cd48f4. Branch `codex/xs138-p4-host-stage-ring`; durable worktree `/home/fidika/cozy/.worktrees/cozy-creator/xs138-p4-host-stage-ring`. Fetched master base500b6de76d6e19e0e432ece04be81d6ee31ca268. No integration/context commits are imported. Required external product contexts are9d959e31d6fc58a2d4c1f7371bc3a2b6b658c6de and, only if explicitly selected by root/operator,627f6978f3469332312a53918fe5a68a48f7e0ec (9d plus the separately qualified cached-workspace guard). Exact complete Python cohorts will be sealed; no absence/old-ABI fallback.

## Exact primitive event format (proposal)

An 8192-row preallocated array of unsigned64-bit integers. Each committed row has20fields in this order:

`seq, monotonic_ns, thread_id, event, boundary_id, dispatch_attempt, segment_call, segment_id, graph_object_id, plan_handle, parent_plan_handle, pool_a, pool_b, stream, use_handle, cache_entry_id, native_cache_owner_id, tail_event_id, status, bytecode_offset`

Zero means unavailable/not applicable; identities are opaque host IDs/public handles, not dereferenced addresses or a CUDA executable handle. Names/source hashes/event definitions are separate primitive metadata. No Tensor, Storage, frame, Owner, graph, Plan, Use or product bound-method reference is retained in the ring or identity tables. The collector retains only its own bound profile callback. A later existing product-facts return can supply a scalar native cache-ID mapping; the collector does not call facts to obtain it. Record sequence and commit occur under a private nonblocking collector lock. Contention, overflow, unexpected types or profile/lifecycle loss mark evidence unavailable; they never alter the original return/error.

Events cover selected dispatch enter/return; GraphSegment construction and call enter/return; prepare_use and wait_ready normal returns; vt.replay host return; _finish_epoch copy/pool-tail return; epoch release; parent intent restoration; callback enter/decline/return and owned-definition close; following preparation phase entry/return/error. Status distinguishes CALL,NORMAL_RETURN,UNWIND,YIELD,DECLINED and UNKNOWN. None alone is never success: normal Python return requires a RETURN_VALUE/RETURN_CONST instruction; generators are checked against YIELD_VALUE and final normal return separately. Normal return is not GPU completion.

Count true selected-boundary dispatches, not nested capture bindings. Arm at up_blocks.0 dispatch2 (capture preparation plus first replay) and retain through its dispatch3 replay and the following boundary/error. Record relevant callback and successor stages while armed, including the following component/intent cleanup. On normal progress, stop data recording only at up_blocks.2 dispatch3 entry; on failure, retain through final unwind. Control/lifecycle tracking continues until process-end collection. No additional authored call, retry, synchronization, byte target or trim. An optional one-stream checkpoint is a separate future experiment, not baseline behavior.

## Observation and exclusion

Use exact-source/code-identity-checked sysprofile observation. Product functions remain original code and are called once. Guard capture_begin/capture_complete and replay_scope using checked normal returns, retaining unknown-exclusion state after ambiguous/error cleanup. No graph_facts/native API polling, JSON or native reads inside exclusion. Defer copying/serialization until confirmed exit; otherwise emit only a pre-serialized unavailable marker and preserve the original failure. This is scoped frontend evidence, not an atomic native admission capability.

CPython3.12 real host control found that threading.setprofile_all_threads observes ordinary threading.Thread but misses a fresh _testcapi.call_in_temporary_c_thread callback. Therefore that mechanism alone cannot claim native CacheOwner callback coverage. Fixture-only solution proposed to root and implemented for review: install one _GraphCacheEntry._shrink proxy BEFORE model adoption; it installs this same owned profile on that callback thread only around ONE invocation of the sealed ORIGINAL function, restoring its previous empty slot in finally. It adds no native call and preserves its boolean/exception. A nonempty foreign profile is not replaced: evidence becomes unavailable and original callback still runs once. No shared marker/native ABI or product file edit. If this proxy is not acceptable, callback coverage must remain explicitly unavailable rather than fabricated.

The launcher owns a single profile slot and refuses conflicting instrumentation; it does not silently stack the old ReturnEvidence profiler. Preserve its checked-completion semantics in this new collector. Root/native owner's independent C++ allocator tracer can be composed separately after their launcher/profile contract is checked; this lane adds no allocator instrumentation.

## Delivery and gates

Only this new diagnostic directory changes. The e7c numerical body/6GiB/settings/assertions remain identical and execute once. Prepare a deterministic source/cohort seal, CPU profile controls with actual functions, foreign-C-thread callback control, exception/None-return/yield/unknown-exclusion/ref-lifetime/overflow/profile-ownership checks, and analysis of the SAVED27056 packet to demonstrate the missing interval. Real GPU execution remains root/operator owned and UNRUN. No SDK build/install or hardware action in this lane.

A draft PR is opened with this plan before implementation; later commits add only the fixture/diagnostic files and their bounded host/source receipts. Feature commits exclude every9d/627 context prefix. Prior model/pressure receipts remain frozen.
