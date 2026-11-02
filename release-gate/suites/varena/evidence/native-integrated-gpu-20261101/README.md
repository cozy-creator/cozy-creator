# Coherent native/Runtime GPU checks

All 22 cells passed on hitoshiki, one RTX4090, driver595.91.07, Torch2.14.0/cu130:
native a96c7c3e8, Runtime7adc1e82c, workerafa6f9229. The native artifact combines
the normal-process guard, phase-region Plan, physical-shortage service and Cache
primitive; Runtime combines the native consumers with automatic owner lifecycle.

| Behavior | Runs |
| --- | --- |
| Python Cache binding | 25373 |
| Allocator, speed, native split mechanics | 25375,25376,25378 |
| Runtime Plan, lifetime, primitive, same-PID cleanup | 25380–25383 |
| Eleven shortage cases and same-PID observation | 25385–25396 |
| INT8/INT4 Heads and same-PID cleanup | 25398,25399 |

The unchanged allocator benchmark passes its 2× bar: native421.369075ns versus
Varena364.4486625ns median host pair, ratio0.8649155435. Both have seven400,000-pair
samples. Varena uses the actual `kernel_wipe` backend under `one_process/1`, makes
zero driver-memory calls in the timed samples and ends with zero live/mapped/handles.
This is a raw allocator microbenchmark, not model speed.

Heads repeats all17 cases and25 exact numeric comparisons, with request-key
retirement in the same PID. The pressure cells retain physical driver refusals,
actual donation/retry, GIL-held progress, stopped donor/writer, requester withdrawal,
local-cap/VA/admission and capture/replay exclusions. Plan retains its actual
file/model, capacity and last-owner lifetime checks on this combined core.

Python Cache executes in the parent a96 extension; its e545 standalone helper is
not invoked in25373. Cache pages move6→4→6MiB with middle-hole/full readback and
end with every Cache-owned field zero. Working live/mapped match their baseline;
working pending increases by2KiB in that cell, so it does not claim all-working
quiescence. The other standalone Cache/death/choice receipts are separate cohorts.

Compressed original run JSON, exact result JSON and invocation receipts are indexed
by hashes in `manifest.json`. Independent audits are included unchanged. No prior
failed receipt was relabeled. The Ready-Cache split improvement d3 is not in a96.
Foreign pressure, owner callbacks, the public shared-core ABI and broader hardware
remain open. Full coherent H3 execution25405 is running separately; these cells do
not inherit another binary's full-model performance or quality result.
