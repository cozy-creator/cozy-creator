# Standalone native consumer GPU evidence

The exact corrected native product c3bf579 and helper/controller30c09e4 passed four
bounded cells on Hitoshiki, GPU-18d960a8-2283-19c9-c060-ef8a22fabe35. The portable
binary SHA351d9fcbb8b36442191eedb2553ea0edc4b0daaaf10393258ad598ce18209476 is byte-identical
to the earlier b4ea861 build; only the parent controller gained its required Torch
dependency. Native consumers execute this independent ELF and load neither Python
nor Torch. Readback checks every requested byte.

Run25306 is preserved as a pre-fixture startup failure: the original controller
omitted Torch and the worker refused its missing memory-policy acknowledgement.
Run25308 uses the corrected declared dependency and enters the actual fixture.
The raw run result and separately extracted detail were compared exactly for each
passing run; all reviewed child exit/ownership/readback records are preserved.

| Run | Actual proof | Boundary |
| --- | --- | --- |
|25308 smoke|Auto/manual/plain each fully read67,108,864 bytes; normal PIDs774/782/789 exited0 with live/mapped/pending/pools0.|No Python/Torch in native processes; parent Python remains ordinary Cozy orchestration.|
|25309 donor death|Manual PID803 published536,870,912 idle bytes; metadata-only watcher825 observed exact Waiting round22 for requester810. Owned PID/birth/pidfd checks preceded SIGKILL inside the actual501.446ms allocation. After3 physical OOMs, the driver's retry allocated/read402,653,184 bytes.|Recovery reported peer_reclaim_exhausted after500.948ms with released=completed=0. Success came from actual physical availability after death, not ledger capacity credit or manual donor service. Filler allocated/read22,150,119,424 bytes.|
|25310 simultaneous automatic consumers|Actual allocation intervals overlapped23.357ms. Right PID840 read/freed805,306,368 bytes; left832 retained typed exhaustion after6 physical OOMs/500.922ms recovery. Both demand/recovery/withdrawal states settled0; all canaries survived.|One real allocation progressed; both did not succeed. Right's donor answer was Refused/completed0 while active. This is overlapping role safety plus one actual progress result, not successful peer-byte donation.|
|25311 optional lifetime|Actual post-CUDA fork child867 returned exact Inherited for facts/close and exited0; parent859 remained active. Real CUDA capture600ms kept live=mapped2,097,152 bytes and product working-driver count30→30; Owner stopped but did not retire until capture ended. Full2MiB canary survived both.|No Arena weights: this does not prove last-Arena teardown or absence of every external CUDA call.|

All expected normal children exited0 and their terminal working live/mapped/pending/
pools were zero. Only the exact owned death-test donor exited via SIGKILL. Watchers
loaded no CUDA. Controller actions issued no ask, service, withdraw or clear_want.
A canary verification and metadata snapshot do not fabricate successful donation.

This is one GPU and one execution of each cell, not all scheduling interleavings,
a full model benchmark, public P4 API completion, release or deployment evidence.
The upstream Runtime pressure suite is a separate qualification receipt. Root owns
rental cleanup and provider settlement; this directory makes no settlement claim.
