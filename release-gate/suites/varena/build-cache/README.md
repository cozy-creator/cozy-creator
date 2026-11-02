# CLI/extension native feature parity

The CLI stage selects both tensorfs and tensorfs-py. Cargo defaults would therefore enable
native tensorfs/default, while maturin selects only tensorfs-py and reaches its tensorfs
store dependency without native defaults. Different cfg features require a second native
compile even with identical source/target/profile.

The helper disables selected-package defaults, then explicitly enables only Python's
default, extension-module, store and interpreter ABI features. Unqualified user features
belong to tensorfs-py. --all-features expands that package's real Cargo metadata feature
keys; it is never forwarded globally to both selected packages. Metadata is Cargo JSON,
so Python3.10 needs no tomllib/tomli dependency and future declared features are discovered.
The full-wheel no-default-features refusal and option/profile/target forwarding remain.

The helper also accepts standard --manifest-path, allowing an exact build-driver overlay
to target a frozen native tree without changing its Rust/version stamp. Both duplicate
CI prebuilds use the same helper and explicit interpreter selection. No native math,
compiler flags, AOT input or GPU behavior changes.

`prove_features.py --out FILE` runs actual Cargo feature trees without building. It proves
that old native default+store skew exists and that stage/extension dependency feature sets
match for default, unqualified, qualified and all-Python-feature cases. Run it under3.10
and a current interpreter. These metadata checks are not a cached-build speed proof.

Root owns the still-busy portable target. The actual cached no-recompile check must use the
recorded helper commit/tool hashes as a packaging-driver overlay against frozen08a Rust,
with pinned maturin1.16, after that target is released. Preserve the earlier build/log and
report native product, packaging driver and tool identities separately.

A second observed mismatch came from the mixed-client library's unused normal dependency
on platform/fetch, needed only by its installer proof. That proof is now an example with
a dev dependency. Actual whole Cargo graphs show all234 extension/CLI dependency nodes
match the mixed/native-GPU helper cohort; only the three helper/client package roots are
additional. Before the change, six shared feature sets changed and forty extra dependency
nodes appeared. The installer example must be built in its own target, outside the frozen
wheel/SDK cohort. All helper builds still need the canonical namespaced feature selection.
