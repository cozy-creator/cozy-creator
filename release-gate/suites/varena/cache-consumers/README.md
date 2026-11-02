# Real cache consumers

This lane uses actual ModelRegistry, file-backed Backend, Placement and component scopes
for DerivedCache; it does not fabricate a ScopeHost or infer lifecycle from a mock model.
The first case targets late Region creation inside nested scopes. Scope entry before the
first cache value must still protect the cache until the outermost scope ends. Later native
requests must decline active ownership, discard idle ownership, preserve escaped storage
and permit a real rebuild.

The LM follow-up must use an actual supported small causal model and the real Runtime
engine/generation/cache path. Idle full-cache retirement is separate from live-sequence
preemption or public paged-KV interior-hole adoption. Earlier native Cache hole/recommit
proof and graph-owner25587–25591 do not qualify those model-specific paths by themselves.

Source/CPU/GPU and installed/released evidence are distinct. Root owns all GPU/build actions.
