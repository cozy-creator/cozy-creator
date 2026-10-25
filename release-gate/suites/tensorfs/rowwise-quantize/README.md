# Bounded rowwise source conversion

Real CPU default-CLI cell; installed TensorFS0.15.5 candidate, no private interpreter or SDK change:

```
cozy run /absolute/worktree/release-gate/suites/tensorfs/rowwise-quantize/run seed=209301 --on local --json
cozy run /absolute/worktree/release-gate/suites/tensorfs/rowwise-quantize/run mode=cancel seed=209302 --on local --json
```

It writes genuine fp8.rowwise(F8_E4M3/F8_E5M2) and int8.rowwise(I8) checkpoint parts with three
scale dtypes and three logical metadata dtypes. Native output uses exactly reconstructed F32,
compared with Runtime's existing Torch RowwiseDequant and production quantizers. Both ConvRot
and NVFP4 are compared at one/two/seven-row conversion budgets (162 actual conversions), with
exact payload/companion/max-error/saturation and declared2e-13 F64-stat tolerance. It verifies
stored code+scale read accounting (both NVFP4passes), inherited plain leaves and output integrity.
Signed codes, negative zero, zero scales, subnormal/finite code values and the last row's largest
scale exercise layout and global-maximum handling. No extra logicalBF16/F16 narrowing is allowed.

The same Writer basic/cancel/cancel-write fixtures remain required for ownership/regression.
This cell is source-ready but not run. Runtime native admission remains unchanged until real
proof passes; calibrated/probe/custom per-component callers still need native primitive stats
before all Python numerical producers can be deleted. No CUDA or performance claim.

The basic cell also checks F32 decoded source bits directly against the existing Torch decoder
and rejects6 malformed/nonfinite sources before publication (metadata errors before any reads).
`mode=cancel` observes actual bounded code/scale reads of a16MiB encoded source after a completed
first tensor, cancels, retries the same Writer at another buffer limit, and checks stored-byte
accounting/unchanged completed entry/integrity. It refuses a missed real cancellation window.

Partial installed qualification: CPU cancellation/retry **17610 passed** on mamiina with
TensorFS16c857cbf/cd5c, Runtime07d5e7054/b43 and TensorDfd4ff48fa. Basic **17609 failed before
native quantization** because the fixture handed Torch tensors to Writer.put_encoded's buffer
API. Fixture1459fb805 serializes those owned parts as immutable bytes, including BF16 scales;
no native code changed. [Exact partial receipt](evidence/16c857cbf-cancel-and-fixture-refusal.json).
Full source-matrix parity and Runtime rowwise routing remain pending the corrected basic run.
