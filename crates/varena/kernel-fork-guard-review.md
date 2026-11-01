# Kernel fork guard: actual GPU evidence and process-contract review

This is evidence for frozen implementation **d1b314a836d39c506de265753e10d13d57e6b843**, not a default change.
Documentation after that commit does not change either measured artifact. The default remains an actual PID check.

## Independent GPU audit

The ordinary Cozy runs used one RTX4090 (`GPU-74f2ac0a-1c52-2e52-a7a6-b207f1db7a89`), driver595.91.07,
Linux6.8.0-147/glibc2.36, Python3.12.12, Torch2.14.0+cu130 at08187d9e, and the original09684b61 shim.
Both sides use the same d1 source, compiler/features except the explicit guard option, eight sizes,
8192 warmup pairs and seven samples of400,000 alloc/free pairs. Within each arm, native Torch runs first
in a fresh child, then Varena in another fresh child. Python dispatch is included.

| Build / run | Native median ns/pair | Varena median ns/pair | Ratio | Raw 2× result |
| --- | ---: | ---: | ---: | --- |
| guard0 /25010 |773.9236075|1688.8420675|2.182181873|fail|
| guard1 /25012 |810.8456850|719.2870100|0.887082491|pass, experimental only|

Varena's median fell57.4095% in this one same-host comparison. Native medians changed4.7708% between sequential
arms, so this is not an estimate of universal speedup or full-model latency. Every timed Varena sample had zero
working-driver memory/event/context-join increments. Final managed live/mapped/pending/pool/event state is zero.
The actual guard1 child reports `kernel_wipe`, a4096-byte page, authenticated thread and marker equal to its PID.
It did not silently use PID fallback. `qualification_eligible=false` and `production_qualified=false` remain true
statements about this experiment even though its raw numeric threshold passed.

Run25013 passes all18 allocator/capture/stream/lifetime groups with zero final managed state. Run25014 passes
all six ordinary-fork callback checks with CUDA uninitialized. The separate exact-source CPU matrix proves
ordinary/raw forks, descendants, proper new-thread authentication, held/competing publication and genuine mmap
failure fallback. It also preserves the accepted shared-VM counterexample; no CUDA operation was run in that child.

Full original results and run documents are in
[`release-gate/suites/varena/evidence/kernel-fork-guard-20261101`](../../release-gate/suites/varena/evidence/kernel-fork-guard-20261101).
`independent-audit.json` recomputes medians/ratios, compares artifacts/cohorts and checks the counters. Neither old
failed receipts nor the counterexample is reclassified as a production pass.

## What the counterexample actually is

The CPU harness calls glibc `clone` with `CLONE_VM | CLONE_VFORK | SIGCHLD`, a separately allocated child stack,
no `CLONE_THREAD` and no `CLONE_SETTLS`. The child therefore has a different TGID, shares the parent's address
space and TLS base, and runs while the calling parent is suspended. Its inherited authenticated TLS cell and
nonzero shared marker allow the six zero-sized/metadata entry points. Default guard0 and failure fallback reject.
The test is not a direct call of POSIX `vfork` and must not be dismissed using only POSIX vfork restrictions.

Linux explicitly supports these independent sharing choices. `MADV_WIPEONFORK` applies to a private mapping copied
into a fork child; sharing the existing memory space provides no copy to wipe. Fresh TLS with a different TGID
would instead hit this experiment's first-thread PID authentication and reject. These are distinct cases.
[Linux clone contract](https://man7.org/linux/man-pages/man2/clone.2.html),
[Linux madvise contract](https://man7.org/linux/man-pages/man2/madvise.2.html),
[Linux6.8 copy_mm/dup_mmap implementation](https://raw.githubusercontent.com/torvalds/linux/v6.8/kernel/fork.c).

Shared TLS in this serialized task shape is not universally invalid libc behavior: glibc's `posix_spawn` uses
CLONE_VM and CLONE_VFORK without CLONE_SETTLS, with an allocated stack and controlled signal/error handling.
Its source explains why there is no concurrent parent/child TLS access. That proves the operating-system pattern
is real; it does not authorize arbitrary callbacks into an initialized CUDA allocator before exec.
[glibc2.41 posix_spawn implementation](https://raw.githubusercontent.com/bminor/glibc/glibc-2.41/sysdeps/unix/sysv/linux/spawni.c).

Ordinary `pthread_create` uses CLONE_THREAD and CLONE_SETTLS with a new thread descriptor. Rust's Unix
`std::thread` path calls pthread_create; GNU libstdc++ `std::thread` reaches it through gthr-posix. These are
same-process threads with independent TLS, unlike the counterexample. This source review supports the tested
normal-thread path; it is not a universal guarantee about every custom runtime or libc.
[glibc pthread_create](https://raw.githubusercontent.com/bminor/glibc/glibc-2.41/nptl/pthread_create.c),
[Rust Unix thread creation](https://raw.githubusercontent.com/rust-lang/rust/1.91.0/library/std/src/sys/thread/unix.rs),
[GNU std::thread](https://raw.githubusercontent.com/gcc-mirror/gcc/releases/gcc-14/libstdc%2B%2B-v3/src/c%2B%2B11/thread.cc),
[GNU POSIX thread adapter](https://raw.githubusercontent.com/gcc-mirror/gcc/releases/gcc-14/libgcc/gthr-posix.h).

## CUDA operation support versus diagnostic rejection

NVIDIA documents CUDA contexts as tied to a host process and allows a context to be current in multiple threads
of that process. Device pointers/events are directly usable by other threads of the same process, but require
explicit CUDA IPC for another process. Sharing CPU VM/TLS is not that import protocol. Varena's process-local
arena does not implement cross-process allocation/context transfer. Thus the demonstrated child did not execute
a supported inherited-CUDA operation; it demonstrated a weaker early-rejection diagnostic for a valid Linux task
shape. No consulted NVIDIA source grants CUDA context inheritance through CLONE_VM.
[CUDA13 multiple contexts](https://docs.nvidia.com/cuda/archive/13.0.0/cuda-c-best-practices-guide/index.html#multiple-contexts),
[CUDA13 IPC contract](https://docs.nvidia.com/cuda/archive/13.0.0/cuda-c-programming-guide/index.html#interprocess-communication).

POSIX vfork children are separately restricted to exec/_exit behavior; invoking a general allocator there is
outside that contract. This does not make the Linux clone counterexample disappear.
[vfork contract](https://man7.org/linux/man-pages/man2/vfork.2.html).

## Recommendation and exact proposed contract

**Recommend the documented normal-process fast path as a separate contract decision, not silent equivalence to
all actual-PID checks. Keep this PR default-off until that decision and its public contract are committed.**
The speed result justifies continuing that path; it does not justify claiming universal different-PID rejection.

A defensible supported-client contract would state:

- One Varena allocator/context belongs to one host process. Calls may come from ordinary same-process threads
  with correctly initialized independent TLS (including the tested pthread/Rust paths).
- After allocator binding, an ordinary/private-VM fork child cannot use inherited Varena state. The kernel marker
  enforces rejection before inherited locks, including raw fork without libc atfork handlers.
- Spawn helpers may perform their normal pre-exec OS work but must not call Varena/CUDA in a separate-TGID
  shared-VM child. A successful exec starts a new process image, with fresh allocator state.
- Custom runtimes which share or copy authenticated TLS across process identities are unsupported by the fast
  guard. The API must not promise they will fail cleanly before a lock or driver call. Keep the actual-PID build
  for applications that require that stronger rejection guarantee or cannot establish the normal-thread contract.
- Missing/failed WIPEONFORK setup and non-Linux targets retain actual PID checks. No source-mode flag may advertise
  kernel protection when the recorded backend is fallback.

This proposed exclusion concerns operation support and the diagnostic guarantee separately. It should be stated
in the C/Rust/Python public contract and tested build documentation, rather than hidden in a performance fixture.
A controlled Cozy executor can adopt that documented contract if its launch/thread paths satisfy it; a generic
public allocator must not be labeled behavior-equivalent to the current strict-PID default. No implementation,
feature default, supported-client promise or qualification flag was changed by this review.
