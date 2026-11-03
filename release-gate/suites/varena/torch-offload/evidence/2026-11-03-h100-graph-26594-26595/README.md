# H100 graph results26594 and26595: exact scope and fixture correction

Original operator files are read-only under the absolute paths/hashes in manifest.json.
This is receipt inspection plus a local CPU bug reproduction, not another GPU run.

26594 PASSES the actual small frontend client: definitions retain no weight Pins,
three same-VA trim/refill cycles, native escaped-alias protection, independent earlier
outputs, enclosing Plan phase restoration, immutable old dtype definitions and Conv
stride/padding/dilation invalidation with old-output preservation/original-tuple reuse.
The input graph hash1462072/helper7fbb79e8/offload2597ec31 and frontend script02718db7
are independently checked against eb315 Git objects and current product files. This
closes only the named normal small-model path, not generic CUDA failure/lifecycle or
full-model graph memory/performance gates. Native core is191c7444 /extension24119b9e,
NOT the old auditedb40f host-build artifacts; do not conflate those cohorts.

26595 FAILS child1 in preparation_ordering worker output.copy_(value), with the actual
InferenceMode thread-local error preserved in scoped-capture-26595.stderr. Caller mode
is not inherited by threading.Thread. This happens before gated unrelated-producer
admission/custody assertions; it is not an allocator defect or passing cleanup proof.
Its bound fixture SHA2cdf0aa1 is the old eb315 file. Captured-Working amendment764afaaa
and its source-only receipt2513 were NOT in this run. No new claims are attached to it.

Fix8f9f4fbd2 adds explicit torch.inference_mode() to BOTH fixture worker contexts.
Product adapter/helper/native/provider sources remain unchanged. The new CPU client
thread_inference_cpu.py reproduces the exact real error without worker mode and checks
exact copy plus caller-state preservation with it. Source AST verifies the actual two
CUDA fixture worker scopes include the context; that is not full CUDA execution.
Ruff/format/syntax/diff pass. The fixed current script includes the captured Working
proof controls and needs a new, correctly bound operator GPU run.

Other prepared cells remain tail/reset/Source error/capture escape and the new Ready-
bytes/resource-retirement-held frontend test. Late query/retirement ERROR propagation
and uncertain native capture/Plan failure cleanup remain unqualified. Automatic whole-
model graph selection, ordered shared pools and fit/performance need their own work.
No machine, rental, native policy, merge or release action was performed by this lane.
