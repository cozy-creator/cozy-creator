# Public native Source provider clients

These are actual C/Rust providers of a full GPU memset payload. They use the public
constructor, fixed owned nonblocking producer streams, native ordered consumer Uses and
checked job/Source retirement. GPU modes are authored but unrun; root owns every GPU lane.

The C executable accepts `host`, or `MODE GPU-UUID DEVICE` where MODE is ordering,
partial, prepare_error or source_pending. Ordering holds actual producer work behind a
CUDA memory-value gate and verifies the consumer waits, then checks every output byte.
Partial submits only half the destination and returns a closed Source error, then refills
at the same VA. Prepare_error creates a real stream retained by persistent Source custody
until checked cleanup; no destination was exposed. Source_pending holds a real native
owner mutex so retirement returns BUSY without waiting; retirement_stats must remain queued
until the owner unlocks. Callback counts are observations, not completion certificates.

The Rust `varena-client` example `source GPU-UUID DEVICE` implements the unsafe Source/Job
traits, retains actual stream and library owners, proves ordered full bytes and waits for
both native job and persistent Source retirement. No Python/GIL callback is involved.

```sh
cc -D_POSIX_C_SOURCE=200809L -std=c11 -Wall -Wextra -Werror \
  -I crates/varena/include release-gate/suites/varena/public-source/source.c \
  -L CORE_DIR -Wl,-rpath,CORE_DIR -lvarena -ldl -pthread -o source-c
./source-c host
./source-c ordering GPU-UUID 0
VARENA_LIB_DIR=CORE_DIR cargo build --locked -p varena-client --example source
# Use the exact same private shared-library cohort; no local host build is a portable SDK.
```

The host constructor-refusal check uses the real core, preserves provider ownership, and
verifies libcuda was not loaded. Existing mixed-client native.c is updated only for the
new distinct provider constructor. Old by-value vr_v1_region_new is absent from the export
set. No public ticket functions are added; native Source/Fill observers feed retirement_stats.

Provider prepare enqueues no GPU work and must retain all failed-preparation resources.
Submit closes every host submitter before returning. Closed source errors abort; panic or
VR_FAULT quarantines unproven ownership. Each published Region is whole, while publication
across several Regions is not transactional. Checked retire is the only GPU cleanup path;
inert drop callbacks are host-only and cannot wait for the GIL.

The current foreign-provider checkpoint preserves the existing checked host-byte provider.
Its separately approved stream-scoped bounded staging successor will follow. Full-model
quality/performance, mixed Python GPU ownership and real driver-failure paths remain open.
The enclosing operator must bound whole GPU child processes: a driver or ownership bug can
block a CUDA call before the fixture's host polling deadline can run.
