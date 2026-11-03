# V4 shared-mount ledger proof

Product feature `906cebea719ae5c48900185c14b15f60095c3c45` changes only the
shared header/open identity and its contract documentation. V4 stores actual
fstat device/inode in header words12/14. The full local Scope, including mount
namespace, stays in every connection and process-local Owner identity. Layout,
rows, banks, OFD locks, birth checks and all byte policy remain unchanged.

`build.py` freezes the exact ledger.rs and ledger/os.rs from named commits into
small standalone metadata-only crates. It does not copy the whole old tree,
substitute a kernel backend, or build another GPU allocator. `peer.rs` uses the
actual module, real process identity, real tmpfs and real OFD locks. Its device
key is explicitly a metadata fixture input, not a claim to discover or exercise
physical GPU hardware. Namespace/PID/start-tick/UID/backing identities are actual
kernel observations, never patched constants.

The build operator uses a task-private target and the cached pinned image
`sha256:4131ddd7acb9c6c966af02fe7e71edc51e078c2d41f475966697676430bfa2ae`,
one CPU, offline, no GPU devices and runc. Both peers built once in16.50s. Their
ELF dependencies are system libraries only; actual clients assert no CUDA/NVML
library is mapped. The existing operator's manylinux target is not used.

```bash
python3 build.py --repository <owned-worktree> \
  --control 1a066af55a982c3fc6407af1a332a65695289437 \
  --candidate 906cebea719ae5c48900185c14b15f60095c3c45 --output <new-build-source>
# Build each generated Cargo project with its own target; preserve source.json,
# Cargo.lock, compiler/container command, binary hashes and all build failures.
python3 prove.py --peers <directory-with-peer-v3-and-peer-v4> --output <new-receipt> \
  --device-key GPU-13800000-0000-0000-0000-000000000003
```

The supplied key matches the earlier metadata client's test key. No GPU identity,
CUDA memory, allocator donation or release is inferred from it. All published
physical fields are zero; publication proof changes an actual monotonic timestamp.

## Actual result, 2026-11-03

`outputs/xs138-ledger-scope-v4-20261103/proof-001/report.json` records the completed
available-case proof:

- Genuine v3 rejects an independent different-mount opener of the same live table;
  the table bytes/device/inode/size remain unchanged.
- V4 independent containers share the exact tmpfs object through different mount
  paths and observe each other's real registrations and publications.
- Real private PID namespace and different effective UID attempts refuse.
- A copied nonempty v4 file on another inode refuses without byte modification.
- Identical path spellings over separate private backing objects remain isolated.
- Killing an exact owned participant releases its kernel OFD row lock; another
  participant reaps it. No GPU release or capacity credit is claimed.
- Existing connections refuse actual file replacement and symlink substitution;
  the original files remain preserved.
- The real same-connection mount-unshare attempt returns EPERM. That migration
  rejection case is explicitly **unqualified**, not simulated or marked passed.

Every owned container and exact PID/birth is independently absent. All new test
files are retained under the receipt's private tmpfs root. No unrelated Docker
resource was changed, no image was pulled, and no privilege was added.

`source-equivalence.json` additionally verifies the complete Scope/current
implementation, per-connection checks, transaction/row policy tail, OS boundary,
Owner identity and Python snapshot binding against1a. Frozen v3 fixtures and
receipts are byte-preserved. The unchanged safety/advisory guard passes after aligning the merged platform-signal lock version with master0.16.8. The initial locked-metadata refusal is preserved; the product906 feature itself needs no dependency change.

This qualifies the available Linux metadata/domain cases only. A true different
user namespace and successful same-process namespace migration need an environment
that permits those actual operations; no such support is promised. Shared-core
consumer/worker integration and actual new-cohort GPU donation/foreign-pressure
remain separate operator gates. V3/v4 never join or migrate; no release is made.
