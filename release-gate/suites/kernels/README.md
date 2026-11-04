# Kernels

Paul, 2026-11-04: "prior to a release, all of our kernels should be exercised at least once, so we don't have any nasty
surprises again." Every kernel the Runtime ships runs once on each family it claims, through its real launch path, on
small tensors, against a slow reference. No weights, no renders.

- `inventory` reads what this card claims from the code: `attention.KERNELS` on this row, every `TritonKernels`, each
  `_kernels` module, each `@triton.jit` launch. A name with no exerciser fails its own cell, so a new kernel can't go unnoticed.
- `gpu kernel=<name>` runs one, in its own run, so a fault ends only that executor and names it. Attention takes a long-key
  tile, several heads, every head dimension its row admits, and must beat every answer a kernel can give without computing
  (V, V's mean, zero). A sparse kernel must really take its sparse path (`attention_sol.observing`).
- `big:<name>` runs one call past 2^31 elements: heads that must agree, and sampled rows against exact attention in float64.
- `check.py` fails an entry no cell launched. `analyze.py` fails any Xid or crashed probe in the machine's log and record.
- On a card with two GPUs every cell runs again as GPU 1 of two, which is what a group's follower is. H3's own group walk is
  `multi-gpu`'s.
- `ci_kernels/static_check.py` needs no GPU, torch or Triton: every `TritonEntry` against its kernel's parameters and
  every launch's argument count. `inventory` runs it on the Runtime it is on, with each entry's launcher order, and fails;
  `--packages` runs it on that tree. Run it by hand on a tree of your own.

Run it before every promotion, one gate per family `pick.sh` names:

```sh
release-gate/suites/kernels/pick.sh
release-gate/gate.sh <card> --suite kernels --runtime <wheel> --establish
```

`cells.sh` copies `minimax-h3`'s `glue.py` beside the package, so its own kernels run here too, by its `checked`.

Not covered: each package's fused plan beyond the Runtime kernels' own axes, `_execution`'s native graphs
(`suites/execution/conditional-body`), H3's real group walk (`multi-gpu`).
