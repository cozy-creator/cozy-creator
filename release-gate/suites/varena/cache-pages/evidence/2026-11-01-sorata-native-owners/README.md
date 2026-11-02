# Standalone native owner and registry-concurrency proof

Independent audit passes for ordinary Cozy runs25511 and25512 on Sorata, GPU
`GPU-846689c6-daf3-15b1-580e-f2a78bdf6748`. Both execute the statically linked9b98 helper
(SHAd10ddaf1...), not the parent a96 extension. The parent7ad/a96 SDK performs orchestration
and GPU identity checks only. The helper loads CUDA and neither Python nor Torch.

Run25511 holds the first owner's real mutex. That owner declines once; the second owner
releases exactly2MiB. Cache backing falls4→2MiB while all6MiB of Pending, Ready-Unjoined and
Cold-Unjoined weights remain physically resident. Context joins and event waits remain0.
Only the actual2MiB submitted fence is advertised as pending release, even though the three
weight pages all remain in the pending/unjoined accounting class. The original first Cache
and all three weight pages are fully read after the test opens its actual CUDA host gate.
Both callback owners then retire, and all tracked physical/working/Cache/pool/event state
returns to zero. Two returned actions are not confused with two pages of physical credit.

Run25512 completes128 actual2MiB Arena create/fill/full-read/drop passes while2,009 native
reclaim/resident scans and1,311 Peer publications overlap the creator's observed active
window. All threads use a start barrier; no product probe or fabricated OOM is involved.
There are no deferred publications in this sample. Final native physical accounting is0.
This bounded concurrency pass is not a claim about every possible scheduler interleaving.

Both raw run details equal the separately extracted results. All1,183 archived source
files, the exact binary and prepared project, actual child PID/start-ticks, pidfd exit and
ELF GLIBC<=2.28 provenance were checked. Both children exit0. Parent driver free after25511
is60,817,408 bytes below its pre-child reading; no physical release is inferred from PID
exit, and no whole-driver equality is claimed. Native zero accounting is a separate fact.

These cells prove explicit native owner requests, nonblocking completed-only collection,
and registry concurrency. They do not qualify allocator-triggered recovery, Python/GIL,
Runtime graph retirement, the later coherentdfb integration, or Cooperative/Foreign credit.
Those later fixtures and v3 service accounting remain separate.
