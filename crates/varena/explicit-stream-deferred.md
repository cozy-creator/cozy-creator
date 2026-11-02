# Eager sole-explicit-stream deferred frees

Owner `/root/master_guard_spec`, XS-138. Fresh master `b41612781`, reviewed native
parent `1eb499475`; prerequisite integration `2a3ff202a`. This candidate changes
the eager Working free/reuse decision, narrow public-ABI routing and real-client qualification.
It does not change the offload adapter, async Fill/readiness or installed cohorts.

Outside capture and outside a prepared pool, a freed allocation whose complete
consumer set is one explicit stream can remain Deferred and reusable only on that
same live stream. It keeps its capacity charge and is not globally reclaimable.
The existing allocation fast path may reuse it in stream order without recording
an event. Before other-stream reuse, unmap or donation, existing polling records
the real tail and requires completion. A failed record retains existing poisoning.

The existing default-stream path is unchanged. Capture/pool branches and
multi-stream frees retain their existing behavior. Unknown foreign consumers are
not admitted by the new condition. Explicit sentinels0/1/2 are not generalized.

The existing stream contract remains: Torch-pooled streams live for the process;
owners of destroyable streams stop submissions, synchronize, call
`working::retire_stream`, then destroy the handle. This change adds no registry.
Retirement still checks completion, removes references and marks the origin retired
before raw-handle reuse. It may not infer completion merely from a freed tensor.

The public C/Rust borrowed-stream contract is different: checked public free or
drained ABI retirement permits stream destruction, and there is no public
retire_stream entry point. Both public Allocation release paths therefore select
the immediate-fence mode of the same Working free transition. Only internal/Torch
callers governed by the existing retire_stream contract admit ordered deferral.
The initial unqualified source draft omitted this distinction; independent review
identified it before any GPU execution. Public checked/free and deferred-retirement
cells now destroy the borrowed stream without using the private seam, then trim
and allocate on another stream.

Required actual GPU cells: repeated same-stream allocation/free/reuse without
new driver events; delayed other-stream reuse/consumer protection; capture/pool
behavior; explicit trim/donation only after a real fence; destroyed-stream
retirement and later allocation. Existing frozen performance receipts motivate
the work but do not qualify it. CPU cargo checks establish source/build only.

No GPU cell, paired benchmark, install, release or speedup is claimed at this
initial draft. Exact fixture commands and results follow in the release-gate
suite; the GPU operator owns hardware execution.
