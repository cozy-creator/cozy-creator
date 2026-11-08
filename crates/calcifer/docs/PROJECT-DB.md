# `project.db`: a project's state in one SQLite database (cl-803)

Paul, 11-07: a project was seven files with no transaction between them (`project.yaml`, `project.canvas.yaml`, `.history.json`,
`.keys.json`, `.measured.json`, `project.lock`, `.runs/walk.json`). Now it is one database in its folder. Calcifer is its only writer; the
CLI and the page go through Calcifer's HTTP. Media stays content-addressed in `files/`, so the folder can still be copied or zipped.

## The folder

| Path | Holds |
|---|---|
| `project.db` (+ `-wal`, `-shm` while open) | Everything below. WAL, `synchronous=NORMAL`: a crash of Calcifer loses nothing committed; a power cut can lose the last moment, never corrupt |
| `files/<sha256>.<ext>` | Every file, by hash |
| `<edit>.mp4`, `<edit>.otio` | Each edit's film |
| `.runs/` | Where a run's outputs land on their way into `files/`. Nothing else |

## Schema

| Table | Row | Was |
|---|---|---|
| `meta(key, value)` | `seq`: the revision, one more per committed transaction; `format`: 1; `id` and `name`, the project's identity (cl-819) | file stats, its path |
| `head(key, value)` | A top-level section other than `assets` (`version`, `defaults`), as JSON | project.yaml |
| `boxes(key, name, ord)` | One box: its name key, its name as written, its place in the list | project.yaml |
| `fields(box, ord, field, value)` | One field of a box, as JSON (`tool`, `approved`, `description`, `models`, `lora`, every input) | project.yaml |
| `canvas(key, name, x, y, w, seq, by)` | A box's row, and the transaction and client that last wrote it | project.canvas.yaml |
| `lock_assets(key, name, kind, numbered, tried, failed)` | A box's record: the last number given, the last seed tried, its failure. `key#n` is an orphan set aside | project.lock |
| `versions(key, number, discarded, record)` | One version or take, `record` its JSON (files, seed, run, key, pickup, clips, text) | project.lock |
| `lock_files(sha256, record)` | A file's row where the file can't say it: where it stays, its parts | project.lock |
| `run_files(run, function, output, sha256)` | What a `cozy run` left in the project | project.lock |
| `facts(kind, name, value)` | A package's tags (`reserved`) and what a tool makes (`makes`), as last read | project.lock |
| `walk(id, record)` | What the render owes: its asks, pending runs, renames, numbers, and the folder and Calcifer it is bound to | `.runs/walk.json` |
| `asked(digest, text)` | The project as a render asked it (its export, by sha256), until the render ends | `.runs/asked/` |
| `measured(sha256, record)` | What is measured of a file. Disposable | `.measured.json` |
| `request_keys(key, digest, status, content_type, body, at_ms)` | One write's `Idempotency-Key`, its ask's digest and its answer, a day | `.keys.json`, `state.db` |
| `ops(id, seq, at_ms, by, words, edits, changes, undone)` | One history entry: undo/redo | `.history.json` |

In memory Calcifer holds one copy per folder (`folder::State`): the boxes (`written::Written`), the canvas, the lock and the walk. A
read costs no query. A write is applied to a copy of what it changes and committed as row changes; nothing is rewritten whole. Another
connection's commit (`PRAGMA data_version`) drops the copy.

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
| `DELETE /assets/{name}`, `PUT` | the box, its row, its record set aside or back, the stopped jobs' walk rows, the entry |
| `POST /assets/{name}/versions`, `DELETE`/`PUT …/versions/{n}` | the version, `approved` where it goes, the entry |
| `POST /approve`, `/unpin` | the field, the entry (a cut it settles is a render, after) |
| `POST /rename` | the box key, every mention, its record, its row, the walk's rename, the entry |
| `PATCH /canvas` | the rows, the entry (none when every row is `placed`) |
| `PUT /yaml` (import) | every box, head and field that changed, the entry |
| `POST /history/undo`, `/redo` | each change of the entry put back, and its `undone` |
| `POST /runs` | the ask, its text, the key |
| `POST /reclaim`, `/cancel`, `push`, `pull` | their rows, no entry |

## The op log

An entry is today's history entry, kept as a row: `{id, at_ms, by: {via, client?}, words, edits, changes, undone?}`. A change is one of:
- `asset {name, before?, after?}`: a box added, gone, or rewritten, each side its fields (JSON) where `.history.json` had its YAML lines;
- `fields {name, fields: {field: [before, after]}}`;
- `card {name, before?, after?}`;
- `take {name, number, noun, before, after}` (`absent`, `listed`, `discarded`);
- `renamed {from, to}`.

The writes record their changes as they're made; the act composes them (consecutive changes of one thing fold, a rename is all its
own) and writes the entry in the same transaction. A request sharing a `Cozy-Edit` with a standing entry joins it. Undo and redo check
each change is as the entry left it ("`name`'s `field` changed since", naming the later entry) and put it back, all in one
transaction: refused part way, nothing moved. The last 200 entries are kept. First open converts `.history.json` (each `asset` change's
lines read into fields), so undo and redo go on across the cut.

## Keys

`keyed::answered`, cl-812's layer, is around every write. A project write under an `Idempotency-Key` writes its key row in its act's
transaction: the effect and "done" commit together. The answer is kept on that row just after the response is built. The same key and
ask again is answered what it was (`Idempotent-Replayed: true`); one still going is waited for; another ask is `409
idempotency_conflict`. A row with no answer (Calcifer died between commit and keeping it) answers `200 {}` replayed: done once, its
words lost. `POST /v1/projects`, before there's a project, keeps its answer in `state.db` (cl-812). The page's key is its outbox action's
id and the ask's hash; the CLI's one per call. The render's own `.keys.json`, `walk.asked` and in-memory maps go.

## Boxes, the binder and format 4

`written::Written` is the project as rows: its head and its boxes in order, each box its fields in order. The reader (`format::Doc`)
reads a `Written` exactly as it read project.yaml: the same `tool`, `approved`, `description`, `models`, `lora`, every other key a field
the binder gives by type, `<name>` mentions, text boxes, names matched in any case, the name-blind key. Only the parse is gone. A render
asks on a snapshot: the export of the boxes, kept in `asked` by its digest until it ends, as `.runs/asked/` was. Line surgery
(`assetfile`) is gone: a write sets fields, so comments aren't kept, and an `etag` is a box's canonical JSON digest.

## Canvas: a precondition, not `was`

`PATCH /canvas {cards, placed?: {name: seq}}`. A person's row is set. A row in `placed` is the page's own working (a new box's place, a
box pushed aside), with the revision of what it was worked out from: it's set only if no other client wrote that row after that; else it
stands, and is answered in `kept` as it is. This is checked in the transaction, so cl-800 holds with no value compare. A patch of only
`placed` rows is the page arranging its own boxes: no entry. A push-aside drop sends its pushes with the box it makes (`moves` and
`placed` on `POST /assets` and `/imports`): one request, one transaction, one entry. Reads and `canvas` events carry `seq`.

## Export and import

YAML is a view. `cozy project export [-o file]` writes the project as format-4 YAML (`GET /v1/projects/{id}/yaml`), for reading, diffs or
git. `cozy project import <file>.yaml` reads one back (`PUT …/yaml`): each box, head and field that differs is written, in one
transaction and one entry, and what it doesn't read is refused with the line, changing nothing. A media file to `import` stays an
asset import. No `project.lock` is exported: versions, takes and files stay in the database.

## First open

A folder with `project.yaml` and no `project.db` is imported once, when Calcifer first opens it:
1. a format 2 or 3 file is upgraded as before (`upgrade::on_touch`, its `.v2`/`.v3` backups stay);
2. `project.yaml`, `project.lock`, `project.canvas.yaml`, `.measured.json`, `.keys.json`, `.history.json`, `.runs/walk.json` and
   `.runs/asked/` are read into `project.db.new`, committed and renamed to `project.db`;
3. those files are deleted. Killed before the rename, the files stand and the next open imports again; after it, the next open deletes
   what's left.

`cozy project upgrade` goes: the first open does it, inside Calcifer. `cozy project new` makes `project.db` with the starter boxes. A
folder is a project when it holds `project.db` (or a `project.yaml` to import).

## Open for Paul

1. `cozy project import x.yaml` reads a project; any other file is an asset, as before. Recommended over a new verb.
2. Identity is `meta`'s `id` (cl-819): 128 random bits for a new project, the path hash it had for one from before. Calcifer's index
   (`state.db`) maps it to its name and where it last was; a moved folder is the same project, and a copy of one another folder still
   holds takes a new id when it opens, so its walk's paid work doesn't go on there.
3. A hand-edited YAML imports without its comments: the database holds values, not text.
