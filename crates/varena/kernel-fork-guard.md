# Default-off kernel fork guard experiment

Owner `/root/p2_core_resume`. New branch from fresh `origin/master` plus immutable0e05;
no profiler, allocator cache, production-default change or GPU action in this lane.

## Guard design

Feature `kernel-fork-guard` is disabled by default and forwarded by tensorfs-py.
Default builds retain actual-PID validation. Non-Linux and failed/unsupported setup
retain the same path. The experimental Linux path is armed during existing process
binding, before CUDA: allocate one private anonymous page, write the owner PID, apply
MADV_WIPEONFORK, make the page read-only, then publish the fully prepared pointer.

The armed check reads the page before TLS or any existing Varena mutex. Zero means an
inherited fork and returns the same error; it never rearms. A nonzero marker also requires
per-thread initial authentication using the actual PID. Proper new TLS threads therefore
authenticate once; this does not fix the documented copied-TLS/shared-VM counterexample.
Publication uses atomic compare/exchange, not a lock/OnceLock that a child could inherit
mid-initialization. Before publication the original PID guard remains authoritative.
Losing unpublished pages are unmapped; the published mapping lives for the process.

Kernel review: Linux v6.8 `madvise.c` rejects file/shared mappings and sets VM_WIPEONFORK;
`dup_mmap` skips copying those pages. `copy_mm` instead shares the existing mm for CLONE_VM,
so no wipe occurs. This directly supports both the proposed normal-fork guard and its
explicit limitation. [madvise implementation](https://raw.githubusercontent.com/torvalds/linux/v6.8/mm/madvise.c),
[fork implementation](https://raw.githubusercontent.com/torvalds/linux/v6.8/kernel/fork.c).

## Real CPU proof matrix, before GPU A/B

| Case | Actual mechanism | Required evidence |
|---|---|---|
| Default feature off | Existing PID path | Compile and ordinary/raw-fork checks; no kernel-page default |
| Armed parent | Actual mmap/madvise/mprotect | Read-only private VMA, nonzero marker, parent healthy, no CUDA |
| Ordinary fork | libc fork | Child marker zero and all inherited operations reject before locks/CUDA |
| Raw fork | SYS_fork, no libc handlers | Same rejection, proving no atfork-only reliance |
| New parent threads | Real pthread/std threads | Initial actual-PID authentication, then healthy checks |
| Fork from another thread | Authenticated and unauthenticated fork threads | Child rejection independent of the install thread/TLS state |
| Unpublished initialization | Hold a real prepared read-only page before publishing; fork on another thread | Child uses original PID fallback and rejects; parent later publishes and remains healthy |
| Concurrent publication | Competing real prepared pages/publications | One live guard; no inherited initialization lock; parent/child results correct |
| Real setup failure | Fresh CPU process temporarily sets RLIMIT_AS=0 around direct native bind, then restores it | mmap genuinely fails; backend remains PID fallback and fork rejection still works |
| Descendant forks | Child forks again through raw syscall | Wipe/rejection persists; parent is not poisoned |
| Shared VM / copied TLS | Separate-TGID CLONE_VM counterexample retained explicitly | Not qualified; no claim this guard detects it |

The native CPU harness will invoke actual Varena guards and zero-sized
entry points, report fixed child records through pipes and use raw `_exit` in children.
It will not open CUDA; parent stats/loaded-library observations and source paths provide
that boundary. Error strings will not be formatted/allocated in multithreaded raw-fork
children. Every child is waited/reaped, with bounded outer execution.

The known counterexample will not be hidden by a pass label. A copied authenticated TLS
plus a shared address space can retain both marker and authentication despite another
TGID. Ordinary vfork child library calls are outside POSIX; that is not a claim of vfork
support. [madvise contract](https://man7.org/linux/man-pages/man2/madvise.2.html),
[clone contract](https://man7.org/linux/man-pages/man2/clone.2.html),
[vfork restrictions](https://man7.org/linux/man-pages/man2/vfork.2.html).

Only after CPU proof and root review: isolated uninstrumented same-host A/B against0e05
with the unchanged seven ×400,000 benchmark and existing fork/allocator/split/lifetime
regressions. This is an experimental feature, not production qualification or a silently
narrowed process-identity contract.

The standalone `fork_guard_cpu` proof currently targets Linux x86_64. The experimental
page implementation on other Linux architectures remains unqualified; default builds
still use actual PID checks. No profiler is included in this branch.

Initial actual CPU matrices passed with the feature off and on. Armed ordinary/raw forks,
descendants and both fork-thread authentication states rejected all six zero-sized/metadata
operations; the parent remained healthy with no CUDA library and zero working/driver stats.
The real mmap-failure fallback and held/competing publication cases passed. The armed
shared-VM/copied-TLS case actually accepted the calls while default/fallback rejected them:
that counterexample is retained as `qualified=false`, never production qualification.

Experimental Python builds expose `_varena_kernel_fork_guard_status` as cold metadata.
The existing benchmark records the actual backend after warmup, validates an armed marker
against its PID, and labels the feature's result `qualification_eligible=false`. The timed
loop and 2× comparison are unchanged; a fallback result is not an armed-guard speed result.
