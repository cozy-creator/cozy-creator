# Scoped capture helper: host scope only

Actual CPU/native-host clients passed with the preferred audited b40f91 SDK/core,
plus this lane’s exact current Python feature overlay. All eight exit codes are
recorded in scoped-final-host-manifest.json. No CUDA initialized; graph_bindings
executes actual Torch/Diffusers bodies and alias ownership, not CUDA graphs.

Source checks verify that capture/routing/footprint/error cleanup and replay remain
byte-identical after the preparation block, and that the helper has only its four
preexisting Ruff diagnostics (imports and the unchanged BaseException cleanup).
Changed adapter/clients pass Ruff. Five changed Python files compile. The caller
review is GRAPH-CALLERS.md (three active Runtime and nineteen direct fixture sites).

The helper is now a hard cut: prepare_pool outside capture, capture stream waits
caller/preparation stream, synchronize capture stream only. Other producers remain
explicit caller obligations. Native prepare_pool may recover/trim/wait under pressure;
no globally nonblocking claim is made. Native admission still atomically refuses
unrelated active producers. No permanent synchronization opt-out exists.

GPU cells graph_capture_sync_gpu.py, graph_frontend_gpu.py and graph_epochs_gpu.py
are prepared/unrun and remain coordinator-owned. They are required before claiming
actual stream ordering, capture/replay/eviction/refill, failure-tail safety or savings.
Private Working pool backing remains resident per definition. No byte-policy/native
changes, model downloads, machines, rentals, merges or releases occurred.
