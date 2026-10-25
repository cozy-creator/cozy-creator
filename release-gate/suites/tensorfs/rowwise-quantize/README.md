# Bounded rowwise source conversion

Real CPU default-CLI cell; installed TensorFS0.15.5 candidate, no private interpreter or SDK change:

```
cozy run /absolute/worktree/release-gate/suites/tensorfs/rowwise-quantize/run seed=209301 --on local --json
cozy run /absolute/worktree/release-gate/suites/tensorfs/rowwise-quantize/run mode=cancel seed=209302 --on local --json
```

It writes genuine fp8.rowwise(F8_E4M3/F8_E5M2) and int8.rowwise(I8) checkpoint parts with three
scale dtypes and three logical metadata dtypes. Native output uses exactly reconstructed F32,
with valid scale shapes `(rows,)`, `(rows, 1)` and `(1, rows)` (the format requires a row
element count, not an exact scale rank),
compared with Runtime's existing Torch RowwiseDequant and production quantizers. Both ConvRot
and NVFP4 are compared at one/two/seven-row conversion budgets (162 actual conversions), with
exact payload/companion/max-error/saturation and declared2e-13 F64-stat tolerance. It verifies
stored code+scale read accounting (both NVFP4passes), inherited plain leaves and output integrity.
Signed codes, negative zero, zero scales, subnormal/finite code values and the last row's largest
scale exercise layout and global-maximum handling. No extra logicalBF16/F16 narrowing is allowed.

The same Writer basic/cancel/cancel-write fixtures remain required for ownership/regression.
Installed native CPU parity passed in run17698; Runtime routing is a separate follow-up.
Calibrated/probe/custom per-component callers still need native primitive stats before all Python
numerical producers can be deleted. No CUDA or performance claim.

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
Those partial attempts did not establish full source-matrix parity or Runtime rowwise routing.

Follow-up basic **17682 failed** after its 162 conversion assertions because the refusal fixture
incorrectly treated `(rows, 1)` scales as malformed. The canonical format accepts any shape with
exactly `rows` elements. The corrected fixture covers those valid shapes above and refuses an
actual `rows + 1` scale count with a matching physical buffer. Native source/artifact is unchanged;
the failed cell is preserved separately from the passing rerun below.

Corrected basic **17698 passed** all162 conversions and6 refusals on riki through the default
CLI: TensorFS16c857cbf/cd5c, Runtime diagnostic14aeb7061/15ec and TensorDfd4ff48fa/bce5. Fixture
1d32ba567 checks exact F32 decoder bits and output parts; SSE/energy tolerance is2e-13, with exact
maximum error and saturation counts. Each ConvRot group reads48,888B and writes49,140B; each
NVFP4 group reads97,776B (bothpasses) and writes27,324B. Five malformed metadata cases read0B;
nonfinite scales refuse after1,040B, with no entry published. Runtime diagnostic use does not
qualify that wheel for release. [Exact compact proof](evidence/16c857cbf-cpu-parity.json).
