# Kernels

Paul, 2026-11-04: "prior to a release, all of our kernels should be exercised at least once, so we don't have any nasty
surprises again." Every kernel the Runtime ships runs once on each family it claims, through its real launch path, on
small tensors, against a slow reference. No weights, no renders.

The exercisers are the Runtime's own, two unstable built-ins of its App (`cozy_runtime.bench`, code in its
`internal/bench/kernels.py`), because a package can't import the Runtime's internals (the package contract, since 0.43.1).
`drive/` is public API only: each cell runs one of its functions, which calls a built-in with `call()`.

- `kernels-inventory` reads what this card claims from the code: `attention.KERNELS` on this row, every Runtime
  `TritonKernels` set, each `_kernels` module, each `@triton.jit` launch: one in a set where this row declares the set, one
  in no set on every row, none another jit function calls. A name with no exerciser fails its own cell, so a new kernel
  can't go unnoticed. An entry whose launcher reads its signature out of order fails the inventory (run 28135's Xid 31).
- `kernel-<name>` runs one, in its own run, so a fault ends only that executor and names it. Attention takes a long-key
  tile, several heads, every head dimension its row admits, and must beat every answer a kernel can give without computing
  (V, V's mean, zero). A sparse kernel must really take its sparse path (`attention_sol.observing`).
- `big:<name>` runs one call past 2^31 elements: heads that must agree, and sampled rows against exact attention in float64.
- `kernels-static` (`static_check.py`, on the runner, no GPU): every `TritonEntry` against its kernel's parameters and every
  launch's argument count, in the Runtime the machine runs and in `--packages`.
- `check.py` fails an entry or jit function no cell launched. `analyze.py` fails any Xid or crashed probe in the machine's log and record.
- On a card with two GPUs every cell runs again as GPU 1 of two, which is what a group's follower is. H3's own group walk is
  `multi-gpu`'s.

Run it before every promotion, one gate per family `pick.sh` names, and one `--only kernels-largest`:

```sh
release-gate/suites/kernels/pick.sh
release-gate/gate.sh <card> --suite kernels --runtime <wheel> --establish
release-gate/gate.sh <card with 24 GB or more> --suite kernels --only kernels-largest --runtime <wheel>
```

Cost, measured 11-04: sm86 3 min $0.01 (RTX 3060), sm89 5 min $0.04 (RTX 4060 Ti), sm120 7 min $0.15 (RTX PRO 4500), sm120 x2
7 min $0.07 (RTX 5060 Ti), sm90 8 min $0.86 (H100 at $6.65/h), sm100 about $2 (B200, not run yet). A real Regenerate at 1440p takes about 7
min of pod time past its 2 min download: `kernels-largest` is about $0.20 on an RTX 5090. Big cells need 24 GB free; a smaller card says `not
run`, and a family's big cells need one such card once (A40, RTX PRO 6000, H100).

Not covered: a package's own kernels (minimax-h3 holds its glue against float64 before it serves, `glue.checked`, so every
H3 render checks it), each package's fused plan beyond the Runtime kernels' own axes, `_execution`'s native graphs
(`suites/execution/conditional-body`), H3's real group walk (`multi-gpu`).
