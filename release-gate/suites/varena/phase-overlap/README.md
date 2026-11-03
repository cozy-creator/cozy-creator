# Typed observation overlap

`tensorfs.varena.torch.PhaseOverlapError` is the exact native RuntimeError
subclass for the existing `state.phases.active` refusal in phase_enter_inner.
Catch it only around admission, before the authored body runs. It does not
permit ignoring a different failure or retrying an already-entered body.

The empty-key and expected-plan-generation checks still precede the overlap
check; active-split/committed-execution checks retain their existing Invalid
classification/order. Token increment, prefetch/sample mutation and every other
admission rule remain in place. The new variant keeps Invalid's nonpoisoning
behavior and C ABI INVALID5. No new C ABI status/callable or allocator policy.
The public alias intentionally requires the same-cohort extension; no hasattr or
text-match compatibility path exists.

One real release-gate client serves both proof boundaries:

```bash
python prove.py --mode host
python prove.py --mode gpu --device 0 --gpu-uuid "$ACTUAL_GPU_UUID"
```

Run in a fresh child with the exact rebuilt core/extension/shim/Ruststd and
recorded source/artifact hashes. Host mode inspects the real native class and
public alias, and checks actual pre-install/unknown-mode refusals. It never
creates an active phase or claims overlap execution: nonempty native phase entry
initializes CUDA. GPU mode exercises eager/eager, eager/preparation,
preparation/eager and preparation/preparation. Each keeps a prior real successful
sample and original outer token intact through the rejected entry, checks empty
key/mode/wrong-token false positives, executes one actual eager body after the
refusal, closes the original token, then proves the next entry succeeds with no
token consumed by refusal. No active state is mocked or patched.

Committed-generation and active-split refusal source paths are unchanged; this
bounded client does not add another plan/split harness. Existing real execution
clients remain their separate gates. GPU execution and automatic-controller
integration are operator-owned and unqualified until actually run.
