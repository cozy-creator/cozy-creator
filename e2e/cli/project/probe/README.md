# Composed probe

The caller uses generated `Operation` references for the managed `shade` and `enlarge`
steps; its private `tint` stays an ordinary same-package target. The callee is a
`cozy-calls` dependency and is not imported or installed for local caller compute.
The existing CPU shade, conditional GPU enlarge, intermediate publication and intentional
failure behaviors remain unchanged.

After locking the adjacent callee, regenerate from this directory:

```sh
cozy-runtime --dir . calls e2e-probe-callee e2e_probe/callee_calls.py
```

The checked-in declarations were generated with Runtime `87c0db669` from the maintained
`../probe-callee` interface after its image-value migration. Recheck that exact interface
when changing the callee; do not edit declaration hashes by hand.
