# Varena host-process contract: one_process/1

Varena state belongs to one CUDA host process. This contract covers the Rust core,
the C-facing allocation callbacks, TensorFS Python bindings and Torch integration.
Each consumer must preserve ordinary process and thread-local storage semantics.
It does not provide CUDA context transfer, allocation IPC or a generic sandbox for
arbitrary Linux task construction.

## Supported use

- Calls may originate from the owning process's ordinary threads with correctly
  initialized, independent TLS. pthreads and the tested Rust/Linux thread path meet
  this shape; GNU C++ std::thread delegates to pthreads.
- Register the allocator before CUDA initialization. A private-VM fork child must
  not use inherited Varena state. Ordinary fork, raw SYS_fork and descendant-fork
  attempts are rejected before inherited allocator locks; children do not rearm.
- Spawn helpers may perform their normal OS preparation and then exec or _exit.
  Before exec, a separate-TGID child sharing the parent's VM/TLS must not call
  Varena or use inherited CUDA objects. After successful exec, the new image may
  initialize its own allocator and CUDA context.
- Do not copy, share or restore another process's authenticated TLS state and then
  invoke Varena. Calls in that unsupported configuration are not guaranteed to
  reject cleanly before a lock or driver operation. A stronger diagnostic is
  available through the strict-PID build described below.

Linux permits the shared-VM task shape, and glibc uses it for controlled
posix_spawn helpers. Valid task creation does not authorize inherited allocator
use. NVIDIA permits direct CUDA pointer/event use across threads of one process;
a different process needs the documented CUDA IPC mechanism, which this allocator
does not implement. [Linux clone](https://man7.org/linux/man-pages/man2/clone.2.html),
[glibc spawn](https://raw.githubusercontent.com/bminor/glibc/glibc-2.41/sysdeps/unix/sysv/linux/spawni.c),
[CUDA13 process-local objects](https://docs.nvidia.com/cuda/archive/13.0.0/cuda-c-programming-guide/index.html#interprocess-communication).

## Backend and build selection

The `kernel-fork-guard` Cargo feature is a Varena default. On Linux it publishes a
private, read-only MADV_WIPEONFORK page during process binding, before CUDA. The
kernel supplies zeroed contents to ordinary/private-VM fork descendants. The hot
check reads that marker before TLS or inherited locks. Every proper new thread
first authenticates its real PID; subsequent calls use its TLS authentication.

The `strict-pid-guard` feature overrides the kernel feature, including when Cargo
unifies transitive features. It checks the actual PID on every operation and
excludes guard-page setup. TensorFS Python forwards the feature. This is a build
choice, not a mutable runtime policy or ordinary user-flow setting. Disabling
Varena defaults also preserves the actual-PID implementation when no dependency
re-enables the kernel feature; the explicit strict feature is the reliable
transitive override.

Non-Linux targets and Linux setup failures keep actual PID checking. A failed
mmap/madvise/mprotect does not publish a partial guard or fall back to a cached PID.
The immutable page is published atomically only after all setup succeeds, and a
fork during publication still encounters the original PID path or a wiped page.
[MADV_WIPEONFORK contract](https://man7.org/linux/man-pages/man2/madvise.2.html),
[Linux6.8 VM copy/share implementation](https://raw.githubusercontent.com/torvalds/linux/v6.8/kernel/fork.c).

`varena::process_guard_status()` and the private Python
`_varena_process_guard_status()` report actual cold metadata. Reading status neither
arms a guard, opens an arena nor initializes CUDA. The schema is
`varena.process_guard/1`, contract `one_process/1`; backends distinguish
`actual_pid`, `unarmed_pid`, `kernel_wipe`, `fallback_pid` and `inherited_zero`.
Failure stage/errno and the actual page/authentication state remain visible.
Status does not certify a release or claim `production_qualified=true`.

## The deliberate diagnostic difference

The preserved experimental CPU counterexample creates a separate-TGID
CLONE_VM|CLONE_VFORK child with the parent's already-authenticated TLS and a
separate stack. The kernel guard accepts its zero-sized/metadata calls; strict PID
checking rejects them. That behavior is not hidden or presented as equivalent.
No GPU operation has been tested in that unsupported child. The normal-process
contract excludes those allocator calls; it does not pretend the Linux mechanism
itself is invalid.

The earlier experimental source/CPU/GPU receipts remain on frozen d1b314a83 and
PR2371. This normal-contract candidate requires its own coherent build and GPU
qualification. The benchmark retains the same workload and2× limit; its status
now names this contract rather than inheriting the old experiment's exclusion tag.

## Separate ledger inheritance rules

The P3 shared-memory ledger's independent OFD row/publication locks and atfork FD
handling remain required. This GPU process guard does not close inherited ledger
file descriptions, repair PID/mount/user namespace identity, grant memory to peers
or replace the ledger's own initialization boundary. No P3 contract changes here.
