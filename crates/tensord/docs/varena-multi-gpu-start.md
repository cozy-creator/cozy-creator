# Per-rank cooperation descriptors

Owner: `/root`, branch `codex/xs138-multi-gpu-start`. Fresh master base
`ca4052592`; snapshot `0fbbc114f` imports native coordinator `404e38e14` while
preserving the current Calcifer-routed CLI machine files. Snapshot code is not
additional multi-GPU work. The old CLI updater bridge and machine-mode option
need integration into the current caller separately.

`protocol::executor::Start.cooperation` is now `Vec<Cooperation>`, in the same
order as the session's sealed devices. CPU/import-only starts carry no entries.
TensorD resolves every actual GPU/MIG UUID before Start, rejects duplicate
identities, and supplies one descriptor per rank. A cooperative executor can
now bind a whole GPU lane. Runtime's companion branch selects and validates the
local descriptor before any CUDA work and checks follower acknowledgement before
collective formation. This is a private protocol hard cut; the old object form
is not accepted as a one-entry array.

This does not move byte policy to TensorD. Native Varena still owns memory
decisions. Source checks, an installed worker pair, independent multi-GPU jobs
and H3 Ulysses qualification are separate gates. Current leg17 retains its exact
f57/08a/2f7 single-GPU cohort; do not install this work there. No merge or release.
