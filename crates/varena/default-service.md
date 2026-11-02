# Standalone cooperation defaults

Owner: `/root/p3_owner_restart`, `codex/xs138-p4-default-service`.
Base: fetched master0888d1026 plus reviewed native187e27d68.

Ordinary public `open(0)` inherits any ready native service. If absent, native
code creates the existing `/dev/shm/cozy-varena-<effective UID>` domain and opens
Shared/priority0, both creation-only. TensorD uses the same pure-CPU directory
helper and keeps its explicit negotiated mode/priority. No environment setting,
second default path or separate memory controller is introduced.

Native helpers are `cooperative::default_directory()`, `device_descriptor(device)`,
`Owner::join_existing(device) -> Result<Option<Owner>>` and
`Owner::get_or_open_default(device)`. Ready joining validates the stored domain
outside locks and rechecks the exact Control generation; it performs no NVML/CUDA
discovery. None means absent or successfully retired, never opening/stopping or
a permanent failure tombstone. A default-discovery race with a custom-domain
winner returns Busy, preserving the winner; the next call inherits it.

Discovery matches CUDA's binary UUID to NVML's binary UUIDV lookup, then verifies
the canonical UUID string's bytes and GPU/MIG kind. NVIDIA's published
[NVML12.9.40 header](https://raw.githubusercontent.com/NVIDIA/go-nvml/v0.12.9-0/pkg/nvml/nvml.h)
already contains this API. Local595.99.02 ELF symbol inspection passed without
initialization; exact deployed595.91.07 readback and GPU/MIG execution remain
unrun. Missing discovery support refuses without prefix or ordinal guesses.

Root owns the public C ABI per-open Arena/client integration and lease retention
through actual native Arena-ticket completion. Generic clients remain Unknown
until real request boundaries; defaults do not fabricate idle/H measurements.
Native/host checks and GPU validation are distinct gates. No GPU action or release
is performed in this lane.

## Checks

Native and TensorD checked offline with Rust1.99.1, one Cargo job/nice19:
32.79 seconds initially, 5.21 seconds for the final formatted product source.
The actual host registry/directory/thread/fork proof passed in 12.19 seconds,
including16 concurrent ready-client joins, replaced-directory refusal, a held
registry inherited through real fork, a successfully retired old Control with
reachable closed handles admitting a new domain, and the original permanent
fault tombstone surviving all handles. No CUDA/NVML library was loaded.

Receipts: `outputs/xs138-p4-default-service/host-003/report.json` and
`identity-availability/report.json`. The initial host pass is retained; host-002
is a preserved harness-only compile refusal (the isolated fork function needed
its unsafe-code allowance). Native product never received that allowance.

The fresh base carries workspace0.16.2; Varena/TensorFS remain0.17.16. These are
source versions, not a release/SDK installation. Root must package the coherent
core/worker cohort and perform actual default GPU/MIG discovery and both client
load orders before qualification.
