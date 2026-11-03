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

## Recorded host/source result

Product `dcbc141b22c054efc56ee092bf3782a0050bbfc0` and client
`21872669f79a84c4c9f7a063b6be09cf046f94d0` have a real task-private full native
extension build (263.67s, one affinity CPU, nice19, offline). The installed-source
host package imports the new class through the actual public facade and passes
host mode. Extension/core/std hashes and exact commands are in adjacent evidence.
No shim or CUDA AOT catalog was built; these are host artifacts, not portable/GPU
release artifacts. The unchanged safety/advisory guard passes.

All six product preimages are byte-identical on audited989 and graph771. Review
confirms one constructing site, unchanged entry ordering/token mutations, unchanged
execution/split refusal source, nonpoisoning behavior and C ABI INVALID5. Root also
integrated the feature into graph2bf8fd329 and paired83ebdad55; those integrations
are separate artifact/GPU gates. No old SDK fake class or source-unit state hook
was used. The actual four-pair overlap matrix remains prepared/unrun here.
