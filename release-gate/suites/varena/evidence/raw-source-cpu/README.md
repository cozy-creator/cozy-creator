# Owned raw Source source/CPU evidence

Product: `507c23c74e69cf143ace96ca405a48676bd53501`.
Current fixture: `67ff347f582745778924018603a46687f7c97a3c` (current CUDA stream-memory
entry points only; product library unchanged).
Draft PR: https://github.com/cozy-creator/cozy-creator/pull/2429

- Complete TensorFS Python bindings plus native Source example: PASS,6.07s.
- TensorFS no-store library configuration: PASS,26.26s. This is not portable artifact proof.
- Seven-mode current GPU fixture source: PASS,5.75s; every GPU mode remains unrun.
- Actual CPU Writer→Checkpoint→Source: PASS against product507. All1,060,881 destination
  bytes match file data, inline recipe bytes and padding;1,048,593 logical source bytes.
  Overlap refuses before writes, and libcuda is not loaded.
- Existing joined host cancellation initializes12,288 padding bytes before piece checks.
  The first fixture assumed zero progress and failed; that receipt is preserved in outputs.
  The corrected fixture checks that exact existing behavior and untouched file/inline spans.
  The new raw GPU pre-submission cancellation contract is a separate unrun mode.
- Original native core and six async Fill cases are byte-identical to input3df797f56.

The independent review inspected the exact WIP hashes archived here over plan-only3cc562878.
It found no additional concrete source blocker. It is not a full review of507/67ff; later
metrics/timing refinements and the two added GPU cases have their separate compilation
boundary. Neither that review nor CPU bytes prove GPU lifetime or performance.

No GPU was touched or rented, no portable cohort was built, and nothing was merged/released.
Held-GIL progress, full models, other devices/processes and external P4 ABI adoption remain
outside these receipts. Commands used locked/offline Cargo, one job and nice19; the same
owned cache is retained for subsequent builds. Existing vendored faer warnings and the
no-store Event::wait_on dead-code warning are preserved in the logs.
