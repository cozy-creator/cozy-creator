# xs160 native Rust merge evaluation

A standalone experiment for offline Cozy project metadata. The binary uses real, exact-pinned `automerge = 0.12.0` and `loro = 1.17.1`, with a committed `Cargo.lock` and its own `[workspace]`. It does not modify or integrate Creator, rent services, or implement a CRDT.

Run from this directory:

```bash
CARGO_HOME="$PWD/cargo-home" CARGO_TARGET_DIR="$PWD/target" \
  cargo +1.99.1 run --release --locked -j 2 > run.log 2> run.stderr.log
cargo +1.99.1 fmt --all -- --check
```

`cargo-home/` and `target/` are local isolated caches/build outputs and should not be copied into the PR. The source, manifest, lock, Markdown, JSON, logs, and tiny conflict snapshots are portable review artifacts. The executable writes its generated artifacts in the current directory.

The project model is a map with `project_id`, `title`, `description`, scalar `prompt`/`approval`, and an assets map. Each asset has `asset_id`, `name`, and a versions map. Each version contains `version_id`, a synthetic blob reference, and `prompt`. Asset/version IDs and contents are identical across libraries; normalized JSON equality is asserted. Automerge uses its document root; Loro uses its named `project` root map. Synthetic blob references are strings; no media bytes are embedded.

The common seed uses actor/peer 100. Four offline replicas use 101–104; three write concurrently and the fourth receives their work. Automerge actor IDs encode the same numbers as eight big-endian bytes. Replicas have unique IDs; these fixed IDs are only for this deterministic experiment. Fresh receivers use 301–304. The five delivery packets are seed, first title write, its causally dependent second write, independent version addition, and independent description edit. Four seven-packet orders include duplicates and deliveries before dependencies; exact orders and Loro import statuses are in `results.json`.

The main scenarios check disjoint fields, different new versions on one asset, three concurrent prompt/approval writes, same-slot deletion versus a write in both peer-ID orders, parent deletion versus child-version addition, snapshot/reload with additional offline edits, duplicate/reordered delivery, and persistence of missing-dependency updates. Additional checks expose version-ID collisions and confirm that the libraries themselves do not enforce record immutability.

The optional measurement is three sequential repetitions of 1,000 assets with two versions each, one seed commit, and one offline title edit. Timings exclude compilation, disk I/O, and JSON assertions/normalization; no warmup, affinity control, or process isolation was used. It is a local synthetic measurement, not a production benchmark. Automerge compressed snapshots and Loro full state-plus-history snapshots are different formats; raw Automerge change sizes and encoded Loro delta sizes also have different framing.

Read `RESULTS.md` for the verdict and `results.json` for observed values. `loro-conflict-history.json` contains actual exported operations, dependencies, IDs, and Lamport values; `loro-after-resolution-history.json` shows that old history still exists after a causal resolution. Both libraries' pre-resolution snapshots are saved. `registry-verification.json` records the crates.io check; `environment.txt`, build/run logs, and the compatibility-check log preserve local execution evidence.

Primary API sources used:

- [Automerge 0.12.0 ReadDoc](https://docs.rs/automerge/0.12.0/automerge/trait.ReadDoc.html), including `get_all` and historical reads.
- [Automerge 0.12.0 AutoCommit](https://docs.rs/automerge/0.12.0/automerge/struct.AutoCommit.html), including changes, save/load, actors, and application of changes.
- [Loro map semantics](https://www.loro.dev/docs/tutorial/map): LWW slots and distinct identities for ordinary child-container creations.
- [Loro 1.17.1 LoroDoc](https://docs.rs/loro/1.17.1/loro/struct.LoroDoc.html): history export, checkout, version vectors, binary import/export, and `export_pending_updates`.
- [Loro export modes](https://www.loro.dev/docs/tutorial/encoding).
- Downloaded pinned crate source, particularly `loro-1.17.1/src/lib.rs`, `loro-internal-1.17.1/src/encoding/json_schema.rs`, and `automerge-0.12.0/src/autocommit.rs`, under this directory's Cargo registry cache. API behavior claims below are also exercised by the binary.
