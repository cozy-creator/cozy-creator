# GPU service

`gpu_service.rs` (`Pool`) coordinates published, cached GPU callables through retained
Runtime executors. TensorD supplies model references and schedules GPU call permits. Each
executor owns its GPU allocations, process-local Varena residency and ordinary Torch cache.
TensorD records measurements, offloads whole idle executors and observes exact process exits;
it has no Varena manager, GPU allocation custody or second memory-admission queue. Nothing
ends an executor for a stall.
Acceptance, journal, progress and output custody stay in `Engine`; scheduling stays in `service.rs`.

## Configuration

The machine builds its `GpuConfig` at start (`serve.rs`) from its GPUs and `config.toml`:
- `devices`: the GPU envelope, `"0"` or `"0,1,..."`. A plan of degree K > 1 runs on the first K;
  a one-GPU plan on any of them. An executor is sealed to its lane (`CUDA_VISIBLE_DEVICES`).
- `inventory`: observed GPU identities and capacities used for measurements and lane selection.
- `gpu_memory_limit`: a requested aggregate machine cap. GPU execution currently refuses this
  setting because executor-local limits cannot enforce a total across a call family and its
  non-allocator bytes.
- `environment`: names `launch_identity::credential_name` judges credentials never reach an executor.
- `host`: `{mode}`, `dedicated` on a rental, else `shared`.

## Preparation

`prepare_root` reads only the installed static interface.
- Every model the entrypoint declares at `<entrypoint>.models.<param>` gets a slot (H3 turbo:
  base and LoRA). Several slots load as one many-model construction (`Load.models`).
- Choices with `source`, `profiles` or `adapters` are unsupported.
- Exactly one `ModelGrant` must match each slot. Declared `component_use` must be within its components.
- Degree: the widest group every slot declares (`sequence_parallel.degrees`, one always)
  within the machine envelope and `RunSpec.gpus` when positive: a cap the run's children inherit,
  never a refusal. Model lanes/checkpoint overrides never change it; the Hub resolves models for the
  selected group's GPUs. Busy GPUs cause queuing, not a smaller group.
- Degree 0 (deviceless): `accelerator=False`, or a machine without GPUs. Such a call runs on a
  pooled deviceless executor within CPU room; idle ones are ended by host pressure, least recently
  used first, before any GPU executor. Degree 1: a call that constructs nothing, on one GPU, an
  process with its own GPU allocations.
- Allocations (`gpu_allocation.rs`), as Slurm's: a deviceless root (a request with no parent)
  holds a contiguous lane, `--gpus` wide or as wide as the widest group its environment's models
  run on here. It is registered when the root dispatches and bound when its first GPU step is
  ready; its steps (child calls, any depth) run only inside it, one call per GPU. It ends at
  `Context.release_gpus()` (in-flight calls keep their GPUs until they return) or when the root's
  call ends. A GPU call is its own root for the call's duration, and its family (its calls, any
  depth) shares its GPUs. Children carry `root` and `family`, stamped at acceptance.
- Order: ready runs dispatch by (root acceptance, id). A root takes its place only with a GPU
  step ready: one still downloading weights or in CPU start-up claims nothing, so younger ready
  roots use the GPUs meanwhile. A root's first waiting step claims, for the pass, a lane no
  older root holds; its younger holders yield at their next step boundary: they start no new
  step (calls in flight and their own children finish; nothing is preempted),
  then their lanes end (a `release` naming the root it went to) and they queue again at their
  own place. A root that released or yielded comes back at its place among the waiting roots
  and never displaces a holder: holders keep their lanes between their steps. A waiting step
  keeps free GPUs from younger requests. Each `grant`/`wait`/`release` is a `gpu` entry in the
  root's log (`RunEvent.gpu`); a wait is logged again only when its blocker (the oldest other
  request claiming, holding or calling on its lane) changes.
- A `Plan`'s construction hashes {generation, App, constructed slots, degree}, not the entrypoint
  that names them: its executors' key, so entrypoints building the same models (H3's
  `ref2va_turbo` and long_form's `motion_segment`) share one executor and its weights. Its id adds
  the entrypoint and the `Weights[M]` sources (`Invoke.sources`). A plan binding a model is
  journaled as a `Preparation`, and `submit` records `preparation_id`; one binding none is
  derived at dispatch.
- An empty `installation_id` binds a `published-<hash>` installation from `packages`.

## Dispatch

- One call slot per GPU: a call takes every slot of its lane. GPU dispatch is fenced only by a
  startup fence or an active GPU record this machine run does not supervise. CPU dispatch continues.
- Placement (`Pool::place`), inside the root's lane (else on unallocated, unclaimed GPUs): a
  group takes a free contiguous lane of K GPUs. It prefers a permitted lane already hosting
  its plan, then any other permitted lane. Measured peaks are hints for offloading idle
  executors before loading, never a reason to refuse a nested call. A waiting standalone call
  claims the GPUs it waits for. A prefetch loads only
  inside its root's bound lane, after queued requests of that root and older ones, and ends at
  its release. A new executor's GPU is logged as `{"event": "place"}` in the memory log;
  `loads.jsonl` and `invokes.jsonl` name each line's `gpus`.
- Memory observations, offloads, pressure holds, returns and placements are lines of the store's
  `logs/memory.log` (rotated once past 4 MiB), machine log `memory`
  (`cozy machine logs <name> memory`; `local` names this computer's). Each call's own memory is
  the run's `memory` measurements (`cozy run show`): torch's peaks and the executor's `stats`
  (bytes paged in per model per step, evictions with rung and reason, activation peaks per
  shape, leftover), which the merge gate reads as `measurements.memory.stats`.
- Ending idle executors for room runs under that GPU's call slot; host pressure ends idle
  executors only on GPUs no call holds.
- Startup: every journaled birth still alive from a previous machine run (GPU births including
  completed requests', and any nonterminal run's) is killed, since nothing can adopt it. GPU
  dispatch stays fenced until each exit is observed. Leftover executor scopes of this machine
  (cgroups, tokens) are killed and removed. A leader still tearing down (no pidfd yet) is watched by sampling.
- Groups (degree K > 1): one executor sealed to the K GPUs with the NCCL seal (`NCCL_NVLS_ENABLE=0`,
  `NCCL_P2P_LEVEL=NVL`); `Start`/`Load` carry `sequence_parallel_degree: K`. Rank 0 spawns, dials back
  and forms its followers inside `Start` (Runtime `RankGroup`); the machine never talks to a follower.
  Each rank owns its local memory. Followers' pids from `Start` are recorded for their GPUs'
  measurements, host accounting and exact process lifetimes; no GPU ledger handles are shared.
  Teardown waits for every group member's exit. Startup fences GPU dispatch until a previous
  machine's leaders and their followers are gone. A refusal or poisoned group call fails the run
  with the executor's own code (a group's first fault names its GPU). A group forks from the generation's import-only parent like a
  single GPU does: the child takes the lane and NCCL seal with its environment; followers are spawned
  by rank 0.
- Before loading or reusing a GPU session, other idle constructions on its lane offload through
  `Map(Ram)`, outside warm sets first and then LRU, until driver free bytes cover the call's
  need: per GPU, the construction's measured working-set peak less what its own executor already
  holds there (the driver's bytes for each rank's process). A cold or offloaded executor holds
  nothing that counts; a reused resident one holds its weights and cache, which the driver's free
  reading already counts as used, so charging its whole peak again would evict small idle
  neighbours (`ask`, `remove_background`) before every segment. Where the driver cannot attribute
  the process (a container whose pids are not the driver's), a reuse is charged its whole peak.
  Without a known peak all eligible candidates offload. If offloading weights still does not cover
  a known, exact need (not a first call, not the whole-peak estimate), idle processes end in the same order
  (`ended_for_room`), newly offloaded ones among them and warm sets last, to reclaim the CUDA
  context and package globals each holds (2-3 GB, most of an 8-12 GB card); a restart is paid
  only if that construction runs again, and only when its held bytes are the difference between
  this call fitting or not. Running sessions are absent from this idle pool. The executor then
  makes room locally or returns its allocation failure.
  `ChildLauncher` journals the birth before Start, Load and Activate; each request sends
  PrepareRequest and Invoke. Final Invoke figures are recorded before dispatch measurements
  end, and persisted peaks retain their high-water values.
- A measured peak is a hint a call can outgrow. While a call holds a GPU and the driver's free
  bytes fall under the pressure headroom (the larger of 512 MiB and 10%), the reclaim loop
  offloads idle neighbours on that GPU (`Map(Ram)`, LRU and warm sets last, `running_call`), so
  the active call pages its own weights only after every idle neighbour has given its back.
  Sibling Cozy executors are never foreign pressure, so ending idle processes and cancelling stay
  the external-pressure rungs below.
- Warming: a warm member's executor is loaded, then `Map{construction, level}` has it load its
  models at `gpu` or `ram`; varena fills them, and its priority is in use > warm > idle.
- External pressure uses driver-attributed process usage on the existing reclaim loop: offload
  idle executors first, then end idle processes, then cancel affected active roots if headroom
  remains low. Cancellation kills their executors and scopes; weights are reloaded on later
  requests. Missing attribution is unknown. The pressure hold and exact exit requirements are
  described in [Device executor](DEVICE-EXECUTOR.md#external-gpu-pressure).
- An executor's failed terminal (its own refusal before entry, or a failed invoke) first keeps a
  triage bundle (`triage.rs`: terminal, traceback, executor pid and stderr tail; canonical JSON
  under `execution/triage/`, fsynced) and binds it in the journal; the outcome body names it as
  `triage_bundle`, and `ReadMachineExecutionTriage` returns its bytes.
- A call whose executor cannot settle (a follower that died mid-call, the leader's poisoned
  latch, or no quiescence) always keeps a bundle, before its kill, with its `fault` (`GPU 1's
  process 5163 ended`, else the latch) beside the stderr tail. Every rank writes the leader's
  stderr (followers inherit its descriptor), so the dead rank's own last words, its abort or CUDA
  error, are in that tail: the run's reason detail and log carry the fault and its last 50 stderr
  lines, a failed child's caller gets the traceback and them, and the machine's tensord log names
  the fault with the same lines.
- Start happens only after `authorize_managed`.
- Invoke must be quiescent. A reusable executor's old cancellation handle is disabled and its
  process detached from that invocation before pool return. Owned CPU output encoding and
  durable custody then continue while the GPU serves another request. The old run can be
  canceled during this work without killing the newer request. A custody failure is `failed`.
- On error the session is terminated (killed, exit observed) before the run is settled:
  CANCELED if a cancel was journaled, otherwise FAILED with the reason, including a pre-start exit
  with its stderr tail; nothing is requeued to fail again. A retained session can lose its channel (EPIPE,
  ECONNRESET, EOF) at PrepareRequest, which enters no handler: a killed executor closes its
  socket before it is a zombie, so the pre-reuse check can miss it. That attempt stays with its
  dispatch (journal `redeliver`: back to awaiting its first executor) and starts once more on a
  fresh executor, whose own failure is FAILED. An unprovable exit leaves the run charged and
  nonterminal.

Executor requests:
- `read_grant {manifest, layout}`: the sealed host layout an executor deposited for that manifest
  and layout (one fd, `held`); else nothing, and the Runtime reads the checkpoint through
  TensorFS. TensorD never fills.
- `deposit {manifest, layout}` + the memfd: a layout varena sealed in the executor, kept as an
  opaque handle (`deposits.rs`) for the next executor of the same weights (D9's refill path);
  under host pressure the least recently granted goes first.
- `checkpoint_item` records a weights output's TensorFS reference, which the Runtime committed.
- `publish` (`Outputs.publish`) goes to `products.rs`: the spool file is retained as a native
  one-file tree bound to the run's actor, then journaled as a `product` (list outputs APPEND,
  single outputs SET; re-publishing identical SET bytes is a no-op). Answers `Published`.
- Progress counts only `advance > 0`.

## Launch

Executors run as the machine's own user: a pod has one tenant, and a local machine runs its owner's
code. Peer credentials must match the machine's UID and GID. On Linux the socket path is
`/proc/<pid>/fd/<dirfd>/executor`.

`ChildLauncher` spawns from one pool-owned thread, because Linux ties `PDEATHSIG` to the creating
thread.

## Known gaps

- Groups always take the first K envelope GPUs. Warm members and import-only parents live on the
  first GPU; executors never share GPU weights (the host cache is shared). No adapters.
- One retained codec helper per executor when the selected SDK's encoding is required; owned
  output work keeps its generation, log and spool alive after GPU executor return.
- Each executor gets its own scope: a cgroup-v2 where the host delegates one (laptop systemd
  scopes, rootful hosts), else a token (containers with a read-only or v1 hierarchy, RunPod).
- Seal: `OMP_NUM_THREADS` 4 (torch's allocator is set in Runtime code); `<root>/home`, `<root>/kernels` (per UID) and
  `<root>/jit/<run>/<generation>`. Earlier runs' JIT scopes, and executor roots (logs) older than a day, are removed at pool start.
