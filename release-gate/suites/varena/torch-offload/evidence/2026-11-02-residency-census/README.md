# Private residency census: first diagnostic slice

Source/CPU qualification only. New private binding:
`tensorfs._ext._varena_public_residency_snapshot(gpu_handle, region_handles=[])`.
No C ABI entry point or public Python API is added. All native return values are
scalars or owned scalar vectors; no GPU owner crosses into the Python result.

The census reports anchor arena id, runtime weight granularity, each arena's
Region ids/requested/reserved/mapped bytes/Contents/pins/unjoined, fills/hits/
evictions, and separate finalizing-arena physical bytes. One `Entry.requested_bytes`
scalar preserves the logical size after the final Region object is dropped.
Optional existing Region handles map current model Versions to never-reused ids.
The client fixture uses weak references and never calls `reservation()`.

It reuses the existing anchor Gpu Lease and arena_inventory ownership. Region
handle links borrow Owned registry entries and copy only scalar ids; no temporary
Weight/Region Arc is cloned or dropped on the observer stack. Arena guards
are released before inventory Arcs on success and error; existing Arena final
Drop moves Resources to the native retirement worker. No new owner queue,
completion probe, Source callback, trim, GC, allocator policy, CUDA initialization,
or driver event call is introduced by the snapshot.

`consistency=sequential_arenas_not_atomic`: inventory generations bracket the
collection, with per-arena generation brackets too. Each arena's structural
fields and counters are copied under its state lock; Contents/pins/unjoined are
independent atomic samples (`state_locked_entry_atomics_sampled`). Finalizing
bytes are individual atomic samples. Matching generations do not certify atomic
multi-arena memory state or consumer completion. Existing handoff gaps return
Busy rather than a partial inventory.

Checks, with the shared cache explicitly released by its prior owner and one
nice19 CPU/jobs1:

- `cargo check --offline --locked -j 1 -p varena -p tensorfs-py --all-targets` passed.
- `cargo test --offline --locked -j 1 -p varena --lib census -- --test-threads=1`
  passed two CPU tests: retired mapped metadata retains requested versus rounded
  bytes, and an invalid handle refuses without loading libcuda.
- Python fixture syntax parsed. Actual valid-GPU snapshot and fixture execution
  remain unrun. No wheel was built or installed; frozen cohorts are unchanged.

## Concrete next-run recipe

After review, transfer only the census delta onto the coherent native tree.
A future matched native core + PyO3 artifact is required; this cannot be overlaid
onto the frozen wheel as a Python-only edit. No such build/install is performed
by this task.

In the existing two-image stock diagnostic, load the client fixture once:

```python
import runpy
capture = runpy.run_path('/path/to/residency_census.py')['snapshot']
```

Capture `capture(owner)` immediately after adoption and before/after each existing
adopted generation, outside its timer, without adding inference, trim, GC, or
synchronization. Serialize results after collection. The fixture does not retain
old Versions or claim their owner category: `old_owner_attribution='unknown'`.

Offline, deduplicate by `(arena_id, region_id)`. Partition mapped bytes into
current model ids, noncurrent non-Retired ids, Retired ids, other arenas, and
finalizing bytes. For mapped current ids, separate requested bytes from rounded
padding. Report unreserved current Versions separately. Do not sum nested intent
phases or Parameter aliases. Changed inventory generations flag overlapping
publication/retirement; even unchanged generations preserve the consistency limits
above. This first slice can identify dominant current padding, retired mappings,
or another arena, while leaving old Plan/export/queue attribution for evidence
that actually requires it.
