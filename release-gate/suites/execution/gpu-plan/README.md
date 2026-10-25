# Prepared execution gates

`repeat` runs isolated candidate-wheel cells for resident GPU continuation, request ownership,
cancellation, retirement and concurrent observers. These cells use the default Cozy CLI with
`--gpus=1`; they do not qualify a full H3 pipeline or streamed native execution.

`external_pressure` is different: it runs `Context.plan(PreparedRepeat)` directly in the installed
executor. The controller starts a finite independent CUDA application outside TensorD's process
family. A second Cozy call would count as owned demand and cannot establish external pressure.
The managed request holds 512 MiB of native storage and waits as an observer. TensorD's existing
monitor must cancel it; ordinary Context cleanup joins the GPU and drops the prepared arena.
The foreign application then fills and uses another 128 MiB before releasing its context.

Use the existing `agent-tools/.local-stack.lock` for the complete candidate installation,
qualification and verification window. Preserve other owners' jobs. With the selected cohort
already installed, run:

```sh
python3 external_pressure_gate.py \
  --candidate-python /absolute/path/to/isolated/foreign-helper/bin/python \
  --output /absolute/path/to/receipt-directory \
  --max-foreign-mib 7168 \
  --expected-tensord FULL_SOURCE_SHA
```

The helper bounds its requested allocations and leaves observed free space; this is not a hard
process or machine memory cap. It exits on its finite deadline, and the controller cancels only
its own managed run on failure. No daemon, reparenting, direct service HTTP or mocked demand is used.

[Run 17243 evidence](evidence/external-pressure-17243.json) qualifies automatic active yielding on
one RTX 4070 Laptop GPU. The foreign helper held 6,489,106,023 bytes; managed cleanup released
562,036,736 driver bytes (536 MiB), and the helper used a further 134,217,728 bytes. Observation to
native release was 1,417.951 ms. TensorD subsequently cleared its admission block.

Hub run IDs differ from TensorD's local request IDs. `cozy run show` supplies their association in
`measurements.memory.stats.varena[].stats.request`; the checker uses that identity. The original
17243 checker failure is retained, and its saved real receipts pass the corrected validator.
`gpu_pressure_cancel` is logged after cancellation returns, so it is not a cancellation-start
timestamp. The reported latency starts at the actual pressure observation. No global preemption,
future failed-allocation prediction, idle-offload, Windows or multi-rank claim follows from this cell.
