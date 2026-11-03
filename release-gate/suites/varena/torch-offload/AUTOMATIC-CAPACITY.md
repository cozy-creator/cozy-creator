# Full automatic controller capacity control

`automatic_capacity.py` is a real public vt.offload client, independent of SDXL
activation. It complements the primitive594 test: the actual controller must
measure cold, attempt a private pool, cancel the refused preparation, leave its
preparation observation, and execute eager ONCE. No method/allocator is patched.

The real model is eval nn.ReLU with no weights. After cold completion, its input
anchors a Working page. Native trim drops unused backing; the fixture sets the
native process limit to the exact observed mapped_bytes. A separate private pool
cannot coexist, while a tiny eager result can reuse the input's existing page.
This uses native facts only, with no guessed reserve, multiplier or resizing.
The controller's successful native peak must stay unchanged. No Python byte
policy is added to product code; the explicit cap is isolated fixture setup.

The refused request must show capacity_refusals1, captures/replays0 and one real
forward body. Restoring the native limit allows a NEW request to capture from the
same peak; the following cached request executes no Python body. Finally an actual
unsupported complex ReLU must propagate after one body, without eager retry.
Profiling counts real forwards without installing module hooks or fake resources.

```sh
# Real host public-path/body-error integration, NO admission/graph simulation:
python release-gate/suites/varena/torch-offload/automatic_capacity.py --device cpu
# Coordinator's guarded ordinary-Cozy GPU child; matched required typed SDK:
python release-gate/suites/varena/torch-offload/automatic_capacity.py --device cuda:0
```

CPU result is actual eager/error evidence only. CUDA admission/capture/replay
assertions are PREPARED/UNRUN in this lane. This does not supersede the strict
two-block first gate or the separately owned real four-mode native overlap tests.
If the real cap does not produce the specified refusal/fitting eager combination,
preserve the failed receipt; do not change the target or fabricate a refusal.
