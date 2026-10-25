# Real local pressure and output ownership proof

2026-10-25 · RTX4070 Laptop · TensorD edda578b1 / Runtime f0d25c8e7 / TensorFS9e95ee586.
Normal install17821 selected the exact artifacts retained in the cohort receipt. All four
existing controllers passed with the strict cancellation-window assertions. No interval retry,
production delay, mocked callback, private SDK execution or injected error was used.

| Gate | Runs and observed result |
|---|---|
| Automatic external pressure |17824 was canceled by the machine pressure actor. ExactPID2963040, startticks20410115, exited via retained pidfd observation.798,097,408bytes became driver-free while foreign buffers remained live; the foreign process correctly used another128MiB. Admission reopened;17825 completed correct arithmetic in freshPID2968178.|
| Ordinary delayed output |17829 produced a lossless4096² image whose downloaded decoded-pixel SHA matched its actual GPU source.17830 began after A's GPU work and completed in the samePID2970899. B overlapped A's actual encoding by503.626ms.|
| Cancel during encoding |17832 canceled with no result/output. Accepted cancellation was inside encoding.begin/end and inside17833's GPU interval.17833 completed correctly in the samePID2970899.|
| Cancel during custody |17835 canceled with no result/output while its real480MiB file was in custody after encoding. Accepted cancellation was before output_finished and inside17836's GPU interval.17836 completed correctly in the samePID2970899.|

Pressure exit was observed354.571ms after the monitor pressure observation, or319.571ms after
its cancellation log. These are observation timestamps, not a claim about the exact kernel/OS
termination instant. A killed process did not need to emit Python cleanup or an Invoke reply.
This is process replacement and usable capacity proof, not recovery of a corrupted CUDA context.

The post-restart TensorD log contains no `kernel boot:` rows from the historical-generation
fanout after the exactedda start. It records retained readiness still holding, then actual
managed GPU work/native plan readiness. Older26-style reports before this restart are excluded.
This bounded log observation is not a guarantee of no future on-demand kernel preparation or
a startup performance benchmark.

Normal update17837 restored the known healthy baseline: TensorD4178, Runtimec1 wheel8588ce22…,
TensorFS e521 wheelca15dd65…. Its observed machine was ready with zero running/queued work.
The runner preserved the installed CLI bytes before adoption and used normal machine updates.

Per-cell compact JSONs retain exactIDs/PIDs, raw-receipt hashes, machine timestamps and assertions.
Their original extraction notes say restoration was pending; the later immutable
`lifecycle-cohort-17821-17837.json` supplies the completed restoration and final log boundary.
These are lifecycle/correctness gates, not comparative model benchmarks. Windows, multi-GPU,
standalone recovery, output service restart and idle-pending-lookahead pressure remain separate.
