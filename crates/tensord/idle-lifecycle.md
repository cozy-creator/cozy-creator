# Cold Cooperative Start and idle CUDA contexts

Control53b merges fresh masterc729d6962 with qualified workerafa. Sampler27e and its
paired Runtime907 are unchanged. The isolated worker comparison holds the existing
a96/7ad SDK constant; a direct comparison against workerafa also includes intervening
master changes and is not a one-change comparison.

The omitted boundary was in `make_room`: its first-call path ended idle non-warm
executors after offloading. Cooperative mode correctly bypasses the byte controller,
but also bypassed that context retirement. `keep_idle` retains one session per
environment/application/degree/lane. The intermediate five-minute `release_idle`
loop is installed only for non-rental machines. Sequential rental work can therefore
keep several idle CUDA processes until explicit retirement, an SDK restart or rental
termination. Varena cannot return their CUDA contexts by unmapping its own pages.

The candidate restores a categorical lifecycle boundary before a genuinely cold
Cooperative executor's Start. The caller already owns the lane. Under membership
then session locks, it claims the least-recently-used idle session wholly inside that
lane which belongs to no explicit warm set. Locks are released before terminating
the process group. Active sessions are not in the idle map; import-only parents are
in the separate zygote map. A warm update and removal have one custody ordering.

The ordinary exact process termination path retains sources until exit. The helper
also verifies every captured PID/boot/start-tick birth, records
`cooperative_idle_exit`, and refuses Start when termination fails or an exit is not
proven. A killed process stuck in the kernel remains unproven; no time threshold or
requested termination grants memory. No driver/free/peak reading chooses victims,
and no byte amount or package retry is added. Reusing the existing requested executor
does not enter this cold-start path.

The whole-device `before_bytes` metric is the first asynchronous NVML reading from
`Peak::watch`. Its thread is launched before Start/Load, but there is no first-reading
barrier; the actual read can race startup. The25405/25347 difference therefore does
not by itself attribute the extra initial sample to older contexts. The lifecycle
gap above is independently demonstrated by source. Real sequential-worker/model
receipts, exact retirement notes and actual driver observations must establish the
effect before a performance claim.

`cargo check --offline -p tensord` passes at nice19 with one job. The real ordinary
sequence is in `release-gate/suites/varena/idle-lifecycle`; staging/AST are CPU-only.
GPU execution, warm/active custody and model/performance qualification are pending.
