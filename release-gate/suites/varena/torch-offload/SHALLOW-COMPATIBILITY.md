# Data-free rebind compatibility

Actual GPU trace26537 attributes all496 fresh VAE reservations to
`aten._has_compatible_shallow_copy_type.default` during `_rebind`. This exact query
must not enter ordinary weight execution/materialization. The narrow fast path
redispatches that operator to PyTorch's native TensorImpl check with Python tensor
dispatch disabled, using the existing operands. It creates no surrogate/meta Tensor,
reservation, export or Use and does not replace PyTorch's result with a constant.
Other operators and unsupported Tensor-subclass refusal keep their existing paths.

The actual CPU client is:

```sh
OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 HF_HUB_OFFLINE=1 \
  python release-gate/suites/varena/torch-offload/shallow_copy_cpu.py
```

It compares40 real queries to ordinary PyTorch in both no-grad and inference mode,
both operand orders: dense/dtype/shape, meta, COO/CSR, quantized, same/independent
managed owners, and invalid arguments. Native false/error results must be preserved.
It also runs real tied/interior Parameter conversion and tiny Diffusers VAE casts
plus exact decode parity. `sys.setprofile` observes the original production call
chain; no operation, resource or method is substituted. Under inference mode the
baseline VAE casts issue184 compatibility dispatches and368 `_Version.tensor` calls;
the corrected query must enter none of `_Version.tensor`, `reservation`, `_metadata`
or `_Device.execution`. The actual Source derivation/casts continue unchanged.

CPU dispatch/value results do not qualify GPU reservation counts, packing or speed.
The coordinator must rerun the unchanged actual trace/model overlay and verify the
query no longer reserves/fills. Fresh final-downcast Versions may legitimately remain
unreserved until semantic intent refresh; do not force66 mapped VAE groups, relocate
existing Regions, invent memberships or add dtype-cache policy. Preserve original
26446/26475/26496/26537 receipts. Inference aliases are a separate feature04478537c;
the narrow compatibility source delta can be applied independently to frozen0af.
