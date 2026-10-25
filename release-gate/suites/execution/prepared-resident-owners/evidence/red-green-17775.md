# Real capture observer regression: red17760 to green17775

The actual PreparedRepeat regression passed on Runtimec1ac7cded. Retained red17760 on
unfixed559 changed capture status1→2 during the host-only pause, before convolution or native
submission. Green17775 kept status1→1, completed two iterations from one native host graph
launch, and matched eager output exactly (`max_abs=0`). The real watch resumed after capture:
successful samples increased59→60. The fixture's final public callback and owner close returned.

Manual watched run17743 remains a second red reproduction: capture invalidated before its
convolution while the real watcher thread exited. This manual mode is intentionally outside
PreparedRepeat's production pause scope.

| Evidence | Exact source and artifact |
|---|---|
| Red17760 | Runtime559080567, wheel6150de986ced21cc03d04c9ff4d1a6b62b7c99f35a247038c6c8911372007d40; RTX4070 Laptop, Torch2.14.1+cu130, TensorFS0.15.6. |
| Green17775 | Runtimec1ac7cded, wheel8588ce22f35872fddd76695217ea949e50eaf506ef0b75a8fbbb0b0eab7b7eb8; RTX3060, same Torch version and TensorFS artifact. Normal update17773 installed these exact bytes; observed TensorD4178. |
| Fixture | Creator729e462f2; prepared_convolution.py SHA7251eb50799f6a6bbffb7d8a696f44c62e6e02b8549bdfdd158a93f63a9056e4 in both red and green. |

Hardware and seeds differ. This establishes a reproduced defect and passing fixed behavior on
another supported consumer GPU, not a same-card causal/performance comparison. The concurrency
follow-up for simultaneous same-attempt callback creation remains unapplied and is excluded from
the result. Combined Runtime4429's native-quantization adapter changes are likewise excluded.

Green17775's recorded execution13.263seconds includes setup, eager/warmup/capture, the intentional
0.3second pause, observer resumption and result handling. It is not convolution-kernel latency or
full-model inference latency. Full30-step SDXL17776 remains a separate qualification; no completed
image or29-native-step claim is included here.

Immutable compact receipts: `red-17743-17760.json` and `green-17775.json`. They retain exact
public/executor IDs, hashes, artifact/install chain, device facts, raw statuses and boundaries.
Raw event streams remain unchanged in `local-canonical-20261025` and `capture-green-20261025`.
