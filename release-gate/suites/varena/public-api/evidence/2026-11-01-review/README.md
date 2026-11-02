# P4 integration-review follow-up

Updater bridge: `fa64d02912bca7439539b81c6e697b8d50f07697`.
Retirement/loader/packaging source: `6ab1dabac1121072127d6c71e868f777a72c740e`.
This is development evidence; PR #2408 remains draft. No machine, rental, merge,
release, CUDA execution or model qualification was performed by this session.

The bridge is deliberately separate: an existing static cozy cohort can receive
library-aware update staging before the dylib cutover. Old library generations stay
available for rollback. A pre-bridge updater still extracts only one executable and
will refuse the new dynamic candidate; use the bridge or updated installer first.

## Proven host boundary

- The committed-source host harness builds the full TensorFS extension/CLI, C/C++
  clients, safe Rust client and actual mixed clients. Both load orders pass.
- All 30 declared C ABI exports match the actual library.
- Ten real-image loader cases pass: matching/mismatching concurrent loads, native
  acquisition races, reference lifetime/refusal retry, fork-lock reset, sequential
  mismatch rejection and unchanged older C clients. Every case maps one core and
  asserts no `libcuda.so`. The fork case does not qualify CUDA use after fork.
- PEP 517 accepts the tested `--profile=dev -Fextension-module` options, builds a
  complete CPython 3.12 Linux wheel, and installs it in an isolated environment.
  Python import and native `tfs version` run without build-directory loader variables.
  Both private mismatch directions are refused from that installed wheel.
- Real cozy/TensorD bundles run from isolated prefixes without loader variables.
  The actual platform staging/copy/packing helpers pass an old-to-new staged update,
  raw installed-binary transport and rollback, retaining both old library files.
  This does not claim execution of a live machine update or the network/signature flow.
- The generated shell install body installs and reinstalls real native artifacts.
  Shell, PowerShell and Homebrew template patches and workflow syntax are checked;
  PowerShell/Homebrew execution was not available on this Linux host.
- Matrix ownership and the configured glibc ceiling are checked with real images:
  a 2.34 dependency is refused under a 2.28 requirement. This is a negative gate,
  not qualification of a release/manylinux cohort.
- The archive builder's final follow-up selects only declared bundle members, plus
  original dist documentation. A real native client bundle with output inside its
  staging directory excludes the output archive and an older C-client artifact.
  Its script hash and archive receipt are recorded independently of native source 6ab.
- Earlier missing-library, private-symbol and concurrent-double-core failures remain
  in the preserved artifacts. No red receipt was replaced by a reduced assertion.

## Retirement and integration limits

Checked Rust completion/free methods retain their owner on failure. Native finalizers
retain the exact handle/completion metadata, retry capture/busy/pending fences and
quarantine terminal failures or unreleased live Cache pages. Physical Python cache
callbacks use the same native owner/drain path; only an in-flight matching callback
retains release/read access after ownership transfer.

The coordinator confirmed both Peer and callback-worker collections retain observer
Arena references. P4 does not force them away. The owned retryable native retirement
interface using stable identities is still pending; current final-Arena retention stays
conservative until it can be composed. Shared service implementation is `db7014b0`, in
coordinator integration `106581cb`; P4's older ledger baseline must not be installed as
a downgrade. Multi-arena Plans and asynchronous Sources remain unimplemented here.

`retirement_gpu.py` and the Rust retry helper are prepared, not executed. The fixture
records driver samples but does not itself claim an exact warmed driver baseline.
The coordinator must qualify final Arena, callback, stream, failure and physical release
cases on its authorized GPU lane. General Torch offload and advanced draft verbs stay open.

Linux host execution does not qualify manylinux, macOS or Windows. Release packaging
now verifies final dependencies and the configured GNU floor, and limits each matrix
job to its own target archives. Windows source builds require file-symlink capability;
the stale hard-link fallback is refused. Platform CI and real service rollout remain
separate integration gates; none was dispatched here.
