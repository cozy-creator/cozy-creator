# Keeper-cohort fixture startup

Owner: `/root/p3_owner_restart`; fixture-only successor from product08a0d955e.
No product files or frozen historical receipts are changed.

`donor.py` now privately binds the allocator, creates its explicit native Owner,
checks actual sharing/priority, retains Working custody and only then activates
Torch. It does not install default Shared0 ahead of the requested Dedicated/lower
priority configuration. Its no-manual-ask/publish/service assertion scope and
resident-value/driver/cleanup assertions remain unchanged.

`shared_python_child.py` preserves the raw native/CUDA-only final-client capture
refusal and original-table row-gone assertions. It now asserts passive_holds0
through that subcase. The real Runtime constructor adoption/mismatch subcase runs
afterward on a distinct service generation, where it explicitly requires the
new passive hold and continued row after all execution clients close. New detail
label: `raw_owner_final_close_then_runtime_custody`. This does not claim a Runtime
keeper disappears before process exit or qualify a no-Torch Runtime Start adapter.

Manual requester/peer primitives need separate treatment: requester.py,
mode_requester.py and p0-working/cooperative.py currently select public Torch
before their explicit manual Peer. Adding an automatic publisher silently would
change their participant/counter/controller scope. Their old frozen receipts
remain valid only for their named old cohorts; do not use these unchanged inputs
as keeper-cohort GPU regressions until native-only allocation/bootstrap is adapted
with an explicit fixture scope. No assertion is weakened to manufacture a pass.

Source/AST/lint checks are separate from actual GPU execution; all migrated GPU
inputs remain unrun. Root owns final cohort packaging and GPU use.
