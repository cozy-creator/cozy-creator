# Kernel readiness publication (CPU)

`kernel_ready` makes a kernel serve once its opener verified it. In a request, `ready` opens it on a
background thread, and an explicit `wait` may open the same slot at the same time (attention's
`_opened` beside `_waited`). Each mode runs two real openers of one kernel through the installed
Runtime in an ordinary `cozy run`: every opener is a 64×64 Torch matrix product on CPU, every
failure a real read of a file that does not exist. CUDA stays uninitialized.

| Mode | Required |
|---|---|
| `late-success` | A wait makes the kernel serve, then the older background opener succeeds: the same kernel object still serves, the epoch does not move. |
| `late-error` | As above, but the background opener's read fails: the kernel stops serving, `failed()` is that `FileNotFoundError`, the epoch moves once. |
| `dropped-slot` | The waiter that opened the slot fails and drops it while a second waiter of the slot succeeds: the second waiter's kernel serves, the epoch moves once. |

```sh
cozy run /abs/worktree/release-gate/suites/execution/kernel-ready-publication/cpu \
  mode=late-success seed=202610270301 --on local --json
```

The gate's cells are `kernel-ready-*` in [`../core-cells.sh`](../core-cells.sh). This proves registry
publication only: not CUDA module lifetime, and not the cause of any GPU crash.

On `local` (CPU, cozy 0.13.0, TensorFS 0.17.1) all three modes failed on canonical Runtime 0.34.0 (runs
19574–19576) and passed on cozy-runtime#1839's candidate 028eecca9 (runs 19578–19580), whose wheel differs
from canonical only in `kernel_ready.py`. [Receipt](evidence/local-19574-19580.json).
