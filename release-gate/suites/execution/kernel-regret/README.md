# Kernel regret

Times every prebuilt GEMM tile, and every dense attention candidate of the card's family
(`attention.DENSE`), at SDXL, Qwen-Image and H3 product shapes. Each is compared against the
pick of the Runtime's own rule: GEMM config -1 (`choose()`), and the first kernel of each
attention cell that opens. The cell measures; it has no pass bar.

Opt-in only, through `cells=`: no suite or `core-cells.sh` sources it. It opens every
candidate inside its own request, so `analyze.py` reports those builds and self-checks as
request-path blocking and the gate it rides on reads FAIL. Run it on a measurement session,
never on a promotion gate.

- Timing: alternating rounds in rotated order, median device time. Each sample runs at least
  2 ms. Weight copies cover twice the L2, so no call reads its weights warm.
- Bits: each int8 tile's output is compared bit for bit with the rule's (`same_bits`). FP8
  tiles are recorded too.
- Output: the `KERNEL-REGRET` line and the reply carry `ms` per kernel, `best`, `regret` (the
  rule's time over the best's, minus one) and `over_3pct`. A shape that does not fit, or
  whose K the op does not take (int8: whole 256-wide ConvRot groups, so SDXL's 640-wide
  inputs run 16-bit), is `skipped`.

`choose()` (cozy-runtime `kernels/csrc/gemm/`) changes only where the regret is above 3%, with
the numbers in a comment (magnitude study, `outputs/opus-takeover-20261027/magnitude-study`).
Attention regret only informs the `DENSE` order; it is not the GEMM rule.

```sh
cozy run /abs/worktree/release-gate/suites/execution/kernel-regret/gpu seed=202610270321 --on <machine> --gpus=1 --json
gh workflow run release-gate.yml ... -f only=...,kernel-regret -f cells=release-gate/suites/execution/kernel-regret/cells.sh
```
