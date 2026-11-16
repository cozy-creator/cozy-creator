# Composed probe

The caller calls the callee's `shade` and `enlarge` through generated `Operation` references (`e2e_probe/callee_calls.py`);
its private `tint` is a same-package target. `e2e-probe-callee` is an ordinary dependency. Regenerate after changing the
callee's interface, from this directory with the callee locked:

```sh
cozy-runtime --dir . calls e2e-probe-callee e2e_probe/callee_calls.py
```
