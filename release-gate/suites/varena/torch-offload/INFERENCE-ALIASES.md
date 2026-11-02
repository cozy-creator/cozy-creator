# Managed aliases in inference mode

Control26475 failed at `weight.detach()` inside `torch.inference_mode()` before
Plan retirement. Actual CPU Torch reproduces the same error: `Cannot set
version_counter for inference tensor`. Slice, narrow, view and split also expose
it. The wrapper returned by alias dispatch must support sharing a version counter.

Only immutable `_Weight` metadata-shell construction temporarily disables
inference mode. Materialization and computation retain the caller's mode; this
is not a switch to `no_grad`. Existing Source/Region/version and alias ownership
are unchanged.

Run against the installed feature cohort:

```sh
OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 \
  python release-gate/suites/varena/torch-offload/inference_alias_cpu.py
```

The real CPU client covers eight alias operations in both no-grad and inference
mode, noncontiguous/interior weights, exact inference, immutable escaped aliases,
tied Parameter identity, dtype conversion and model close. A separate model is
created/adopted/converted entirely inside inference mode. Actual computation
must still produce inference tensors in the inference-mode control. No CUDA is
initialized and no models are downloaded.

The evidence directory records the failing baseline and passing feature plus
audited-native-overlay integration receipts. CPU success does not qualify CUDA
control26475, physical reclamation or graph execution; these remain coordinator
GPU gates. The task-context commit is not an adoption dependency: compose the
small `_Weight.__new__` change onto the coordinator's public adapter.
