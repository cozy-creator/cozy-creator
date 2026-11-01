# Calcifer API

Calcifer is the daemon on every computer that runs cozy. Its HTTP surface is the product: the `cozy` CLI and a
web UI are equal clients of it, and the CLI only parses arguments and prints (cl-716). Every behaviour a verb
offers is one or more of the calls below. Written by hand and changed in the same PR as the routes
(`crates/calcifer/src/api.rs`, `machines.rs`, `project/mod.rs`); the records are `crates/protocol`.

## Clients and discovery

Calcifer listens on `127.0.0.1` only, on `calcifer.port` from `~/.cozy/config.toml` (0: any free port), and
publishes a lock record at `~/.cozy/calcifer/calcifer.lock` (owner-only): `{address, token, pid, version,
started_at_ms}`. A client reads the record for the address and the bearer. The CLI starts Calcifer when none
answers, and restarts it when it runs another cozy than the installed one.

Calcifer and the CLI are one binary, but a running Calcifer can be older than a freshly updated CLI. The CLI
reads an answer that shows this as a version skew and never shows it as a bare 404 (`calcifer_client.rs`,
`Client::exchange` and `Client::opened`, behind every `get`, `post`, `delete`, `text` and `watch`):
- from a Calcifer of another version, a route it does not serve (`404 unknown_route`), an answer without fields
  the CLI reads, or a request body it cannot read (`400 body_invalid`, the CLI's newer shape): the CLI says so,
  restarts Calcifer on the installed cozy and asks once more. From a Calcifer of the CLI's own version these are
  refused as they are: a restart could not help. If the restarted Calcifer is the same version,
  it refuses with `calcifer_outdated` and the remedy (`cozy update`).
- request fields Calcifer did not read: Calcifer names them in `Cozy-Ignored-Fields`. The ask was carried out
  without them, so it is never sent again; the CLI says so and restarts Calcifer for the next command.

## Security

Every request passes, in order:
1. **Loopback peer** (`not_loopback`).
2. **`Host`** is one of Calcifer's own `127.0.0.1:<port>`, `localhost:<port>` or `[::1]:<port>`
   (`host_not_allowed`): a DNS-rebinding page is turned away before anything else.
3. **`Origin`**, when present, is Calcifer's own `http://<host>` (`origin_not_allowed`). No CORS header is ever
   sent.
4. **The bearer** (`unauthenticated`), on every route, from either carrier of the one secret:
   - `Authorization: Bearer <token>`, the lock record's token (the CLI);
   - the `cozy_session` cookie, for a browser page, which cannot put a header on a `<video>` or an
     `EventSource`. A page receives the token in its URL's fragment (`http://127.0.0.1:<port>/#token=…`, which no
     server or log sees) and posts it once as `Authorization: Bearer` to `POST /v1/session`, which answers
     `Set-Cookie: cozy_session=<token>; HttpOnly; SameSite=Strict; Path=/v1`. On the cookie, an unsafe method
     (POST, PUT, PATCH, DELETE) must carry Calcifer's own `Origin`, and a read (GET, HEAD) must not be
     `Sec-Fetch-Site: cross-site` (`session_cross_site`). `DELETE /v1/session` ends it.
5. **Admission**: a stopping Calcifer refuses new mutations (`503 calcifer_stopping`); `POST /v1/down` and reads
   pass.

The token is minted at every start: a restart ends every session.

## Conventions

These hold for every resource. A route that cannot follow one says why in its section.

**Naming.** Paths are `/v1/<resources>[/{id}[/<sub-resource>]]`, plural and kebab-case (`/v1/rental-quotes`).
An action on a resource is a POST to a sub-resource named for it (`/v1/runs/{id}/cancel`,
`/v1/projects/{id}/approve`). JSON fields are snake_case. Ids are strings with a kind prefix (`run-…`,
`project-…`); a run's `number` is for people and is accepted wherever its id is. Times are Unix milliseconds in
fields ending `_ms`, in records and queries alike (`since_ms`); durations end `_s` or `_ms`. Request bodies
ignore unknown fields and name them in `Cozy-Ignored-Fields`.

**Hub.** A call that reads or writes the Hub names it with `hub` in the body, or `?hub=` on GET and DELETE;
absent, Calcifer's configured Hub.

**Creating and long work.** A POST that creates a resource or starts work carries an `Idempotency-Key` header:
one key is one ask, however often it is sent (`409 idempotency_conflict` when a key is reused for another ask).
It answers `202` with the resource as it stands, or `200` when the key had already made it. Work that goes on
after the answer is followed on the resource's events stream; a POST never answers with a stream. Content that
names itself (files by `sha256:`) is written with an idempotent PUT instead.

**Lists.** A list answers an object holding the plural field (`{"runs": [...]}`), newest first, with `limit`
(default 50, at most 1000) and `before` (the last id or number seen) for the next page, and how many more match
(`older`).

**Errors.** Every refusal is one envelope, with the HTTP status its `kind` names:
```json
{"error": {"kind": "not_found", "code": "run_not_found", "message": "…", "remedy": "…", "next": ["cozy run ls"], "cause": "…"}}
```
`kind` is `usage` or `validation` (400), `credential` (401), `forbidden` (403), `not_found` (404), `conflict`
(409), `capacity` (507), `unavailable` (503) or `internal` (500). `code` is stable and snake_case; `message` is
for people; `remedy`, `next` (commands to run) and `cause` (a peer's own code, such as the Hub's) are optional.
A run's or machine's outcome carries the same typed reason.

**Events.** A stream is `GET …/events` (`text/event-stream`). Each event's `data:` is one JSON object tagged by
`"type"` (snake_case); there is no `event:` field, so an `EventSource`'s `onmessage` sees every event. A
resumable stream gives each durable event an `id:` and resumes after `Last-Event-ID` or `?starting_after=`
(the same cursor). A comment keeps the connection alive every 15 s. A stream ends when what it follows has
ended, or when Calcifer stops; a client reconnects from its last `id:`.

**Files.** A file a resource serves answers `GET` and `HEAD` with `Content-Length`, `Content-Type`,
`Accept-Ranges: bytes` and an `ETag`, a single `Range: bytes=…` with `206` and `Content-Range` (`416` when
unsatisfiable), and `If-None-Match` naming its ETag with `304`, so a `<video>` seeks it and a browser caches it
(`files.rs`, one responder for every route).

## Resources

### Session
- `POST /v1/session` (the bearer in `Authorization`): `204` and the `cozy_session` cookie.
- `DELETE /v1/session`: `204`, the cookie cleared.

### Runs
<!-- cl-716 B: runs, files, interfaces -->

### Machines and rentals
<!-- cl-716 C -->

### Packages, models, account and orgs
<!-- cl-716 D -->

### Local code
Local code is a target like any other: `./dir/function` or `./script.py/function`, an absolute path once a
client has resolved it (Runs). Calcifer reads the package or PEP 723 script, keeps its manifest under
`~/.cozy/calcifer/local/`, and makes it ready on the machine a call goes to: an install run first when that
machine lacks it, then the call with its files. Its interface comes from Runtime's static describer on the
Python and cozy-runtime its project locks; nothing runs to describe it.

### Projects
A project is a folder holding `project.yaml` (people's), `project.lock` (Calcifer's), `assets/`, and its cuts
(`edit.mp4`, with `resolution:` also `preview.mp4`), each with its timeline beside it (`edit.otio`). Its id is
fixed by its folder. Every file the project uses
is in `assets/<sha256>.<ext>`, flat: what cozy makes as real files, and each file project.yaml names (relative,
absolute or `~/…`) taken in on first use, by the first method that works: a copy-on-write clone (reflink:
Btrfs, XFS, ZFS 2.2+, APFS), else a symlink to the original, else (Windows without the right) a hard link on
the same volume, else a copy, said once. project.lock records each file's path, size, sha256 and method under
the path written, and with every video asset (made or named) its shape, probed once: `{width, height, fps,
frames}`. A changed original is taken in again and what read it goes out of date; for a link that is
said (`reference changed`), and a gone original is `reference missing: relink`, what reads it not rendering.
A clone or copy is the project's own: the original moving changes nothing. Names live only in project.yaml and
project.lock. Calcifer deletes from `assets/` only what project.lock no longer names.

A shot's and an upscale's alternatives are takes, every other node's versions: records carry one generic
field (`versions`, `version`) and a `noun`. Each is numbered (1 first, never reused) and is a set of assets
(`{path, sha256, original?}`, `original` the path project.yaml writes for a file it names).

- `POST /v1/projects` `{path, create?}`: the project holding `path` (the nearest folder up from it with a
  project.yaml), or with `create` a new one made at `path` (`409 project_exists`). Answers `{id, path}` and
  records it in the index as used now. `404 project_not_found` when no folder up from `path` is a project.
- `GET /v1/projects?limit=&before=`: the index, last used first: `{projects: [{id, name, path, used_at_ms,
  missing?, busy?}], older}`. `missing`: the folder no longer holds project.yaml.
- `DELETE /v1/projects/{id}`: `204`, forgotten from the index; the folder is untouched. `409 project_busy`
  while it renders.
- `GET /v1/projects/{id}`: `{id, path, fps, nodes, missing, stale: [{name, why}], orphans: [{name, kind, versions,
  renamed_to?}], busy, cuts: [{cut, path, cut_at_ms?, frames?, retimed?, skips}], warnings}`. `fps` is the
  edit's frame rate; a cut's `frames` its length at it and `retimed` the takes in it at another rate
  (`<node> (<fps> fps)`), both from the shapes project.lock records. Each node is `{name, kind, noun,
  versions, approved?, assets, path?, made_by?, status}`: `versions` counts them, `approved` is the approved
  one's number (absent for a file attached by hand), `assets` what the node reads as now by output name,
  `path` its primary file, and `status` is for people, in the node's noun. An orphan is a node project.lock
  keeps versions of that project.yaml no longer names; `renamed_to` the one new node of its kind with none.
- `GET /v1/projects/{id}/nodes/{name}`: `{node, versions: [{number, assets, made_by?, runs, approved?}]}`.
- `POST /v1/projects/{id}/runs` `{names, n?, on, hub, preview?, approve?}` (`Idempotency-Key`): `n` new takes
  or versions of each name, or `names: ["all"]`, everything missing or out of date, each approved, then the
  cuts. `approve` approves the new one when it lands (`400 approve_one` with `n` > 1). `202 {id, path}`; the
  work goes on in the project's walk and is followed on its events.
- `POST /v1/projects/{id}/rename` `{from, to}`: `{from, to, versions}`. What project.lock keeps of `from`
  (versions, approval, what other versions read of it, what is owed) is kept as `to`, a node project.yaml names
  now, of the same kind and with no versions (`409 rename_taken`, `400 rename_kind`, `409 project_busy` while it
  renders). Nothing on disk moves.
- `POST /v1/projects/{id}/approve` `{name, version}` (`version` its number): `{name, noun, number, assets, path,
  stale, warnings}`. `path` is its primary asset; `stale` names what this put out of date; the cuts are made
  again when a shot's take changed.
- `GET /v1/projects/{id}/events`: `started {name, run, number}` (follow the run on `GET /v1/runs/{id}/events`),
  `landed {name, noun, number, assets, path}`, `said {message}`, `warning {message}`, `cut {cut, frames, fps, retimed?, took_s}` and
  `ended {error?}` when the walk owes nothing more. Not resumable: a client reads `GET /v1/projects/{id}`
  after reconnecting.
- `GET|HEAD /v1/projects/{id}/assets/{sha256}.{ext}`: the asset (Files). Its ETag is its sha256.
- `GET|HEAD /v1/projects/{id}/cuts/{cut}` (`edit.mp4`, `preview.mp4`, and each one's `.otio`): the cut or its
  timeline as last made (Files), so a `<video>` seeks it; `404 file_missing` before the first cut. Every in
  and out point is an exact frame at the project's `fps`. The timeline is OpenTimelineIO JSON
  (`application/json`): the project's rate, a video and an audio track (a piece without sound is a gap), one
  clip per edit entry with its frame-exact `source_range`, referencing that take's asset
  (`assets/<sha256>.<ext>`); a retimed clip's metadata is `{cozy: {retimed_from_fps}}`. The encoded edge
  pieces are kept in cozy's cache folder (`$XDG_CACHE_HOME/cozy/cuts`, `~/Library/Caches/cozy/cuts`,
  `%LOCALAPPDATA%\cozy\cache\cuts`) by take, frames and encoding, unused ones pruned after 14 days and the oldest
  past 10 GiB.

### Tools and Calcifer
Calcifer fetches the tools it runs on first need, pinned builds held to their sha256, into `~/.cozy/tools`:
ffmpeg (cuts, frames, clips) and uv (local code). No route asks for them.

- `POST /v1/down`: `{stopping: true, owed}`. Admission closes, mutations in flight finish, every stream ends,
  then the server. Machines keep what they accepted; the next Calcifer resumes `owed`. Never refused.

## CLI verbs and their calls

| Verb | Calls |
|---|---|
| `cozy project new <path>` | `POST /v1/projects {path, create: true}` |
| `cozy project ls` | `GET /v1/projects` |
| `cozy project render …` | `POST /v1/projects {path}`, `GET /v1/projects/{id}/events`, `POST /v1/projects/{id}/runs`; each `started` run followed on `GET /v1/runs/{id}/events` |
| `cozy project approve <node> <n>` | `POST /v1/projects {path}`, `POST /v1/projects/{id}/approve` |
| `cozy project status` | `POST /v1/projects {path}`, `GET /v1/projects/{id}` |
| `cozy project rename <old> <new>` | `POST /v1/projects {path}`, `POST /v1/projects/{id}/rename` |
| any verb, Calcifer of another version | `POST /v1/down`, then the installed cozy's Calcifer started |
<!-- each PR adds its verbs' rows -->
