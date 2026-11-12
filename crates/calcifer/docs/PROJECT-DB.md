# `project.db`: canonical Automerge state in a SQLite transaction

Paul, 11-07: a project was seven files with no transaction between them (`project.yaml`, `project.canvas.yaml`, `.history.json`,
`.keys.json`, `.measured.json`, `project.lock`, `.runs/walk.json`). Now it is one database in its folder. Calcifer is its only writer; the
CLI and the page go through Calcifer's HTTP. Schema 2 makes native Automerge values and original changes authoritative for logical
project structure, canvas, versions and undo history. SQLite commits those bytes with local execution bookkeeping and disposable
relational projections. Media stays content-addressed in `files/`; video, audio and images are never CRDT values. Encryption is optional.

`cozy new` encrypts new projects by default and writes this database directly through the readable
mount. The paths below are inside that view. `cozy new --unencrypted` explicitly creates ordinary
storage; existing projects are unchanged. See [local encryption](PROJECT-ENCRYPTION.md) for credentials and recovery.

## The folder

| Path | Holds |
|---|---|
| `project.db` (+ `-wal`, `-shm` while open) | Canonical native state, projections and local bookkeeping below. WAL, `synchronous=FULL`; committed SQLite transactions are durable subject to the filesystem/device honoring flushes |
| `files/<sha256>.<ext>` | Every file, by hash |
| `<edit>.mp4`, `<edit>.otio` | Each edit's film |
| `.runs/` | Where a run's outputs land on their way into `files/`. Nothing else |

An edit's root film and timeline follow its selected version. Replacing a root film does not create
an `edited-*` side copy. The retained version's media and timeline stay in `files/` and version history.

## Schema

| Table | Row |
|---|---|
| `crdt_document(id, format, snapshot, heads)` | The complete native Automerge snapshot and causal heads; logical schema 1 |
| `crdt_changes(hash, seq, actor, bytes)` | Original immutable native changes, keyed by native change hash |
| `meta(key, value)` | `seq`: the revision, one more per committed transaction; `format`: 1; `id` and `name`, the project's identity (cl-819); `board`: the board its last render ended on, and whose (a copy's or another Calcifer's isn't shown); `rows_at`: the document's heads and stream length the rows were written at; `conflicts`: the document's concurrent alternatives |
| `head(key, value)` | A top-level section other than `assets` (`defaults`), as JSON |
| `boxes(key, name, ord, id)` | One box: its name key, its name as written, its place in the list, its permanent id |
| `fields(box, ord, field, value)` | One field of a box, as JSON (`tool`, `approved`, `description`, `models`, `lora`, every input) |
| `canvas(key, name, x, y, w, seq, by)` | A box's row, and the transaction and client that last wrote it |
| `lock_assets(key, name, kind, numbered, tried, failed)` | A box's record: the last number given, the last seed tried, its failure. `key#n` is an orphan set aside |
| `versions(key, number, discarded, record)` | One version or take, `record` its JSON (files, seed, run, key, pickup, clips, text) |
| `lock_files(sha256, record)` | A file's row where the file can't say it: where it stays, its parts, or the whole hash of one no version or run records, which a box names only the start of (`Lock::keeps_row`) |
| `run_files(run, function, output, sha256)` | What a `cozy run` left in the project |
| `facts(kind, name, value)` | A package's tags (`reserved`) and what a tool makes (`makes`), as last read |
| `walk(id, record)` | What the render owes: its asks, pending runs, renames, numbers, and the folder and Calcifer it is bound to |
| `asked(digest, text)` | The project's boxes as a render asked them (its snapshot, JSON, by sha256), until the render ends |
| `measured(sha256, record)` | What is measured of a file. Disposable |
| `request_keys(key, digest, status, content_type, body, at_ms)` | One write's `Idempotency-Key`, its ask's digest and its answer, a day |
| `ops(id, seq, at_ms, by, words, edits, changes, undone)` | One history entry: undo/redo |

Schema 4 (cl-885): a read takes the `head`, `boxes`, `fields` and `canvas` rows, the heads and `meta` `conflicts`, which each commit
writes with the document; the document is decoded only to write, for history, or when `rows_at` doesn't match `crdt_document` (a
document set in place by another writer, cut short, or holding two boxes of one name, which rows can't say). Then the document is read,
and the next write rewrites the rows whole. `GET /v1/projects/{id}/integrity` lists where the rows differ from the document; e2e checks
it after every write. History reads the document, never `ops`.
`walk`, `asked`, `measured` and request keys remain local SQL bookkeeping. A render's temporary `asked` text is dropped at settlement;
its retained versions keep original text, input digest, files, seed, tool, pickup/clips, warnings and actual global run identity.

`project/crdt.rs` maps boxes, assets, versions, canvas and history by permanent IDs rather than names or local display numbers.
Explicit presence tombstones retain concurrently written children. The document uses native maps/lists/scalars, not one opaque JSON
string. Original actors/changes survive save/load; each local load starts a fresh writer actor so copied snapshots cannot resume the
same actor counter. Concurrent scalar alternatives remain in `get_all` and surface as warnings; application conflicts still need a
person's choice. No network replication service is introduced in this phase.

In memory Calcifer holds one copy per folder (`folder::Shared`): the boxes (`written::Written`), the canvas, the lock and the walk. A
read costs no query. A write is applied to a copy of what it changes and committed as native changes plus a recoverable snapshot and
projection row changes. Another connection's commit (`PRAGMA data_version`) drops the copy. Two connections: the one that writes lives in the write turn, and one that
only reads answers what the copy doesn't hold (history, keys, asked texts, `meta`). A read never waits on a writer, even one waiting
out another program's write lock (cl-837). The copy, its history and the connections last as long as anything holds the project open
(`CALCIFER-API.md`, Projects): the last `Folder` to go closes them.

## One action, one transaction

`Folder::act` runs a request's whole effect under the folder's write turn: every write it makes (boxes, canvas, lock, walk), its
history entry and its `Idempotency-Key` row commit together, or nothing does. A write outside any act (a render landing, a measure) is
its own transaction. Killed at any point, an action is whole or absent. Two writers never interleave: the turn serializes them, and each
applies its change to what is there when its turn comes.

| Route | One transaction holds |
|---|---|
| `POST /imports` | the box, its row, the push-aside `moves`, the lock's file row, the key, the entry |
| `POST /assets` | the box, its row, `moves`, the key, the entry |
| `PATCH /assets/{name}` | its fields (and with a tool on a picture, the version it lists), the entry |
| `DELETE /assets/{name}` | the box, its row, its record set aside, the stopped jobs' walk rows, the entry |
| `POST /assets/{name}/versions`, `DELETE …/versions/{n}` | the version, `approved` where it goes, the entry |
| `POST /approve`, `/unpin` | the field, the entry (a cut it settles is a render, after) |
| `POST /rename` | the box key, every mention, its record, its row, the walk's rename, the entry |
| `PATCH /canvas` | the rows, the entry (none when every row is `placed`) |
| `POST /history/undo`, `/redo` | each change of the entry put back, and its `undone` |
| `POST /runs` | the ask, its text, the key |
| `POST /reclaim`, `/cancel` | their rows, no entry |

## The op log

An entry is a native history-map record with a permanent `uid`; `id` is a local display label. Its projection is
`{id, at_ms, by: {via, client?}, words, edits, changes, undone?}`. A change is one of:
- `asset {name, before?, after?}`: a box added, gone, or rewritten, each side its fields (JSON);
- `fields {name, fields: {field: [before, after]}}`;
- `card {name, before?, after?}`;
- `take {name, number, noun, before, after}` (`absent`, `listed`, `discarded`);
- `renamed {from, to}`.

The writes record their changes as they're made; the act composes them (consecutive changes of one thing fold, a rename is all its
own) and writes the entry in the same transaction. A request sharing a `Cozy-Edit` with a standing entry joins it. Undo and redo check
each change is as the entry left it ("`name`'s `field` changed since", naming the later entry) and put it back, all in one
transaction: refused part way, nothing moved. The last 200 entries are kept. History is loaded only from
the current native CRDT document; physical `.history.json` files are never imported.

## Keys

`keyed::answered`, cl-812's layer, is around every write. A project write under an `Idempotency-Key` writes its key row in its act's
transaction: the effect and "done" commit together. The answer is kept on that row just after the response is built. The same key and
ask again is answered what it was (`Idempotent-Replayed: true`); one still going is waited for; another ask is `409
idempotency_conflict`. A row with no answer (Calcifer died between commit and keeping it) answers `200 {}` replayed: done once, its
words lost. `POST /v1/projects`, before there's a project, keeps its answer in `state.db` (cl-812). The page's key is its outbox action's
id and the ask's hash; the CLI's one per call. The render's own `.keys.json`, `walk.asked` and in-memory maps go.

## Boxes and the binder

`written::Written` is the project as rows: its head and its boxes in order, each box its fields in order. The reader (`format::Doc`)
reads a `Written` (`PROJECT.md`): `tool`, `approved`, `description`, `models`, `lora`, every other key a field the binder gives by type,
`<name>` mentions, text boxes, names matched in any case, the name-blind key. A render asks on a snapshot: the `Written` as JSON, kept in
`asked` by its digest until it ends. A write sets fields, and an `etag` is a box's canonical JSON digest.

## Canvas: a precondition, not `was`

`PATCH /canvas {cards, placed?: {name: seq}}`. A person's row is set. A row in `placed` is the page's own working (a new box's place, a
box pushed aside), with the revision of what it was worked out from: it's set only if no other client wrote that row after that; else it
stands, and is answered in `kept` as it is. This is checked in the transaction, so cl-800 holds with no value compare. A patch of only
`placed` rows is the page arranging its own boxes: no entry. A push-aside drop sends its pushes with the box it makes (`moves` and
`placed` on `POST /assets` and `/imports`): one request, one transaction, one entry. Reads and `canvas` events carry `seq`.

## Current-format opening

Schema 4 databases with the current native Automerge storage format are opened; 2 and 3 are migrated in one transaction.
`db::open` opens an existing file only. Read-only SQLite inspection validates schema and native
storage format before any writable connection setup, pragmas or DDL. Schema 1, unversioned/empty
existing databases and unsupported versions are refused; no migration, recovery backup, stage,
relational load or SQL identity fallback runs. Normal SQLite read-only WAL coordination may
create necessary sidecar metadata; committed current WAL contents remain readable. Refusal
preserves the primary data/schema/history and any existing backup/stage files.

New project creation exclusively reserves a new database path, initializes the current schema and its
canonical native document, then sets project identity before publishing it. An existing path,
including a zero-byte or interrupted `.new` file, is never removed/truncated by creation.
A missing database is not recreated by opening a folder. Current append/CAS, transaction,
corruption checks and relational query projections retain their ordinary roles.

A folder with no `project.db` is not a project, whatever else it holds: Calcifer never reads, imports,
upgrades or backs up an old `project.yaml`, `project.lock` or `project.canvas.yaml`, and
`cozy new --unencrypted` there makes a fresh project beside them.

## Open for Paul

1. Identity is canonical `identity.id` (also projected in `meta`, cl-819): 128 random bits for a new project, the path hash it had for one from before. Calcifer's index
   (`state.db`) maps it to its name and where it last was; a moved folder is the same project, and a copy of one another folder still
   holds takes a new id when it opens, so its walk's paid work doesn't go on there.
