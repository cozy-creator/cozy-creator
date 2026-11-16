# Typed calls

A CPU chain run with `cozy run` on `local`: caller → child → leaf → the leaf's private self call. Each callee is an
ordinary dependency, installed in the caller's one environment and called through a generated `Operation` reference
(`child_calls.py`, `leaf_calls.py`, written by `cozy-runtime calls <distribution> <module>`).

The `typed-calls` CLI shard (`../typedcalls.sh`) copies the three packages, pins them to the run's candidate Runtime
and TensorFS wheels, and runs:

- `references.py`: builds the leaf wheel, locks the caller, checks the locked hashes, regenerates both reference
  modules (Python 3.12 and 3.13 must agree, as must the leaf's wheel and source interfaces), and requires
  `stale_call_reference` for a leaf whose request default or bound changed.
- `run.py`: the cases below, keeping every command, watch record and export under `$OUT/typed-calls`.

| Case | Expected |
|---|---|
| media | PNG 4×3, pixel (0,0) RGBA (7,11,19,95); `prime`, `squared=289` from the memoized leaf, a 16-hex xxhash fingerprint |
| assets | the same, with the image passed through `assets=Files(...)` to the child's `pictures` |
| failure | the caller catches `ChildCallError`: `typed_fixture_failure` and the child's request id |
| model one/two | two tiny local F32 checkpoints chosen by `models={...}` read 1.25 and -2.5, with different checkpoints |
| private | a direct run of the leaf's private operation is refused `callable_internal` |

Every result reports that the child and its xxhash dependency are installed in the caller's environment, and that the
child and leaf ran on the caller's interpreter.
