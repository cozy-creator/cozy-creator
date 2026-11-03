# Automatic eligibility: preserve the original callable output contract

Automatic and explicit APIs have different promises. Explicit GraphSegment clones
output leaves independently. Automatic vt.offload must preserve a callable's original
aliases; no alias reconstruction is implemented. Output eligibility therefore rejects
input/parameter/buffer aliases, repeated Tensor objects and shared Storage across leaves,
including disjoint views and empty aliases. No clone, data read, reservation or Use occurs
in classification. The original cold result is returned untouched, never reconstructed.

Actual CPU client automatic_eligibility_cpu.py calls each real model body once and checks
its actual return identity/Storage before and after the production metadata helper.
Eight controls cover (x,x.view), duplicates, disjoint shared Storage, managed parameter,
buffer, empty duplicate, independent empties and independent normal outputs. C10 metadata
query profiling observes41 queries with zero Version.tensor/reservation calls. Output
weakrefs expire after caller references drop. Reviewed standard Identity is independently
refused for promotion even though module preflight passes, because its result aliases input.

Preflight rejects known inplace=True modules before capture. Unknown/custom mutable-
buffer/side-effect module classes are not promoted, even after an alias-free successful
cold call. Actual public local/global hooks and replaced forwards (including functools.
wraps annotations) do not grant purity. Actual recorded original/installed identities
are required from adoption; no __wrapped__ inference is trusted. The initial reviewed
set is conservative standard unmodified Torch implementation types; third-party/custom
module/control-flow/global changes need separate reviewed support, not a cold-pass claim.

Two real CPU clients pass, including unchanged shape/dtype/config/scalar frontier/key
controls. Exact AST of those earlier keys/frontier is unchanged; explicit graph source
hash1462072 is unchanged in the matching SDK overlay. Ruff/format/syntax/diff pass.
These helpers are NOT wired into automatic dispatch yet. No default automatic selection,
CUDA, native cache-pressure, full model coverage or alias reconstruction is qualified.
No fake native resources, GPU/rentals/machines/merge/release or other-owner file edits.
