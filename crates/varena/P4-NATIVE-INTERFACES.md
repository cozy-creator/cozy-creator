# P4 native-interface coordination request

Owner split remains: the coordinating agent owns the shared peer-service seam and
GPU execution. P4 owns external ABI ownership, private-library compatibility and
packaging. This is a request for agreement, not implemented Plan/Source extensions.

The supplied integration review is
`/home/fidika/cozy_v2/outputs/xs138-p4-integration-review/review.md`.
P4 still starts from dfb912042. The coordinator supplied shared-client implementation
`db7014b0377259f8ab7831251c5a3754d4fb52b1`, in integration
`106581cb66f161a218672032ebcbc0b59b89dd01`.
Compose the P4-only changes onto the current v3 cohort; never install this old ledger
baseline as a downgrade. P4 has not edited coordinator worktrees or rental state.

- **Shared service:** the agreed implemented signature is
  `Owner::get_or_open(device, ledger::Device, &Path, initial_sharing, initial_priority)`
  returning `Result<Owner>`, with `close(&self) -> Result<()>`. Acquisition validates
  device/domain and inherits existing configuration; creation defaults do not change
  a surviving service. Every handle owns one client lease; final close retains a
  retryable retirement ticket. Unknown/abandoned activity cannot train H.
  Public Gpu will retain that lease when composed onto this cohort; Torch's
  installed allocator requires its own root. P4 will not create another service or
  infer idle/active state. API retirement is independent of peer-service ownership.
- **Observer retirement interaction (new concrete finding):** Peer::State.arenas
  and the cache callback worker both retain observed `Arc<Arena>` owners for their
  entire lifetimes. P4 preserves a final Arena in its retirement queue and uses
  `Arc::try_unwrap` under exclusion so a transient observer cannot become an unsafe
  final destructor. These persistent observer roots prevent that queue from finishing
  while a service or unrelated cache owner survives. Please supply/agree the native
  detach/retirement seam that releases obsolete observer custody safely and keeps
  accounting exact. P4 will not force-drop active owners or invent a byte policy.
- **Multi-arena Plan:** agree a native Plan constructor accepting region owners from
  several arenas on one GPU, including a globally unambiguous region identity and
  consistent inventory generation. The native core must own admission/reclaim intent.
  The ABI can keep Sources and Regions alive but must not implement a second policy.
  Current one-arena refusal remains until this seam is supplied and qualified.
- **Asynchronous Source:** agree a native completed/in-flight fill ticket that retains
  producer streams/staging, orders each consuming stream behind the producer, and
  preserves failed-fill ownership and capture exclusion. The core must distinguish
  published contents from actual completion. P4 will not remove the current checked
  completion join or mark an enqueued fill ready by itself.

Please return exact interface signatures and an integration commit before P4 widens
either contract. General Torch offload and advanced draft verbs remain unfinished.
GPU retirement/mixed-binding fixtures are source-prepared only until the coordinator
runs them on its authorized lane and records actual contents/accounting/driver results.
