# Local project CRDT proof

`db_crdt` in `../projectdb.sh` drives the installed `cozy` CLI in the default home. It creates a real project, imports media, edits and renames an asset, and reads the actual Automerge snapshots persisted by Calcifer. It requires no model, rental or extra daemon and does not restart Calcifer.

The cell checks:

- Native snapshots and original change bytes are committed in schema-2 `project.db`.
- A deliberately unsupported relational/schema-1 copy is refused with its main bytes/schema unchanged and no migration backup or stage. Current behavior fixtures begin with a schema-2 project.
- Damaged relational query projections cannot override canonical state or undo history on the next CLI write.
- The native Rust reader loads the CLI-generated snapshots, verifies the renamed entity retains its identity, and forks three real CRDT replicas. It retains three concurrent prompt alternatives and both independent versions allocated the same display number, converges despite reversed/duplicate delivery, and survives snapshot reload.

The shell cell is part of the existing `projectdb` shard. To run this cell alone once the candidate is installed and the shared-daemon owner approves that candidate, source `e2e/lib.sh`, parse an output directory, then source `project.sh` and `projectdb.sh`, call `db_crdt`, and finish the suite. Keep all execution in the default home; no `COZY_HOME` override.

The Rust driver is only a format/merge inspector for those real project bytes. It does not replace Calcifer storage, authenticate authors, synchronize machines, merge media, or qualify multiplayer rendering performance. Its separate manifest/lock pins the same Automerge 0.12.0 as the product. No unit tests or mocks are added.
