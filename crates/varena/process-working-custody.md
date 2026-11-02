# Passive Working lifetime and explicit Torch device activation

Owner: `/root/p3_owner_restart`, branch `codex/xs138-p4-process-custody`.
Frozen inputs: native4c8cdacb1, Python129d98967 and PyArena e8da50e9a.
Root owns public ABI ServiceLease adoption; external Source/offload files are untouched.

`Owner::retain_working_lifetime()` binds the already-ready service to the actual
initialized Working root. Its private passive hold is separate from execution
clients and their reported/request state. It chooses no domain, changes no
mode/priority, creates no participant and votes for no activity. A passive-only
service reports Unknown. Real declared clients still determine their own activity
and H observations. No byte policy or cleanup thread is added.

Working's process-lifetime registry retains the hold while Ready/Pending physical
storage or the installed allocator can survive a public handle. Releasing the last
execution client is nonfinal while this hold exists; try_close then means that
client's lease ended, not that the participant vanished. Root's public Gpu open
must bind the hold after Ready acquisition and before publishing the handle.

Public `tensorfs.varena.torch.install()` defaults to device0 and activates it.
Explicit None is not a public CPU-only mode. Runtime uses a private allocator bind,
then validates its explicit Start owner and binds custody before Torch CUDA init.
The existing C++ callback-table layout remains unchanged. A bound-only native
allocation entry refuses an unactivated device before native initialization or
allocation; it adds no per-allocation IPC or Python callback.

Runtime's CPU/MPS/Hello/import-only paths remain unchanged. CUDA without negotiated
configuration, current multi-rank CUDA and CUDA-granted no-Torch startup are refused
before GPU/group work. Their qualification gates stay open; this is not a claim
that all remaining policy/consumer lanes are complete.

Required proof: actual host passive/client states, no extra Unknown execution
client or H invalidation, cold unbound native refusal without loading CUDA/NVML;
then real Ready/Pending Working storage after public close, a new client's reuse/
donation, default standalone activation, and explicit Dedicated Runtime startup.
All GPU work remains root-owned and unrun in this lane.
