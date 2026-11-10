# Calcifer API

Calcifer is the daemon on every computer that runs cozy. Its HTTP surface is the product: the `cozy` CLI and a
web UI are equal clients of it, and the CLI only parses arguments and prints (cl-716). No client shares a disk with it
(cl-819): a client's files, folders and local code go by content (`PUT /v1/files`), a project is named by its id or name,
and a run's outputs are fetched by their URL. A path a client sends is always one of Calcifer's own computer. Every behaviour a verb
offers is one or more of the calls below. Written by hand and changed in the same PR as the routes
(`crates/calcifer/src/api.rs`, `machines.rs`, `project/mod.rs`); the records are `api::calcifer` (`crates/api`). The CLI
uses them as Rust types; the page uses the same records as TypeScript, `web/app/src/api/calcifer.ts`, which ts-rs writes from
them (`scripts/web-types.sh`, checked in build).

## Clients and discovery

Calcifer listens on `127.0.0.1` only: on `calcifer.port` from `~/.cozy/config.toml` and no other if it's set, else
on 8818 or the next free port, so a page's origin outlives a restart. It holds `~/.cozy/calcifer/calcifer.lock` and publishes a lock record beside it at `calcifer.lock.holder` (owner-only): `{address, token,
pid, version, started_at_ms}`. A client reads the record for the address and the bearer. The CLI starts Calcifer when none
answers, and restarts it when it runs another cozy than the installed one.

Calcifer's own state is one SQLite file, `~/.cozy/calcifer/state.db` (WAL, owner-only), each change one transaction: runs,
machines and asks, and beside them the projects it opened, browser sessions' digests, your tool list, Render here, what model
headers said and an ask's step after its run (cl-818). The JSON files an earlier Calcifer kept these in (`projects.json`,
`sessions.json`, `tools.json`, `here.json`, `checkpoints.json`, `after/`) are imported at its first start and deleted. Blobs stay
files: `files/`, `inputs/`, `machine-logs/`.

Calcifer and the CLI are one binary, but a running Calcifer can be older than a freshly updated CLI. The CLI
reads an answer that shows this as a version skew and never shows it as a bare 404 (`calcifer_client.rs`,
`Client::exchange` and `Client::opened`, behind every `get`, `post`, `delete`, `text` and `watch`):
- from a Calcifer of another version, a route it does not serve (`404 unknown_route`), a request body it cannot
  read (`400 body_invalid`, the CLI's newer shape), or a read's answer without fields the CLI reads: the CLI says
  so, restarts Calcifer on the installed cozy and asks once more. From a Calcifer of the CLI's own version these are
  refused as they are: a restart could not help. If the restarted Calcifer is the same version,
  it refuses with `calcifer_outdated` and the remedy (`cozy update`).
- request fields Calcifer did not read: Calcifer names them in `Cozy-Ignored-Fields`. The ask was carried out
  without them, so it is never sent again; the CLI says so and restarts Calcifer for the next command.
- a write's `2xx` whose answer the CLI can't read: it was carried out, so it is never sent again either
  (`answer_unreadable`); the CLI restarts Calcifer for the next command.

## Security

Every request passes, in order:
1. **Loopback peer** (`not_loopback`).
2. **`Host`** is `127.0.0.1`, `localhost` or `[::1]`, on any port, so an SSH `-L` to another port works
   (`host_not_allowed`): a DNS-rebinding page's `Host` is its own name, and it's turned away before anything else.
3. **`Origin`**, when present, is `http://<Host>` (`origin_not_allowed`). No CORS header is ever sent.
4. **A credential** (`unauthenticated`), on every route but the page and `POST /v1/session`:
   - `Authorization: Bearer <token>`, either the lock record's token (the CLI) or a session's (a browser);
   - `?media=<media>`, a session's second secret, on `GET` and `HEAD` of a file route only (`/v1/runs/{id}/outputs/{name}`,
     `/v1/projects/{id}/files/{file}`, `cuts/{cut}`, `thumbs/{sha256}`), as `<img>` and `<video>` can't send a header.
5. **Admission**: a stopping Calcifer refuses new mutations (`503 calcifer_stopping`); `POST /v1/down` and reads
   pass.

There are no cookies: a cookie reaches every port of a host, and a page's `localStorage` only its own. The lock
record's token is minted at every start; a session outlives restarts.

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

**Every project write can be sent again.** A POST, PATCH, PUT or DELETE under `/v1/projects/` that carries an
`Idempotency-Key` is done once: the same key and the same ask (method, path and body) again is answered what it was
the first time, with `Idempotent-Replayed: true`, across a restart too, and one still going is waited for. A project's
write keeps its key in its `project.db`, committed in the transaction of its effect, and its answer on that row just after;
one whose answer was lost there (Calcifer stopped between) is answered `200 {}`, replayed: it was done once. `POST
/v1/projects` keeps its answer in `state.db`. Each is kept a day. The same key for another ask is `409
idempotency_conflict`. The CLI makes one key per call and the page one per outbox action and ask, so a resent undo steps
once and a resent discard is no `404`.

**Lists.** A list answers an object holding the plural field (`{"runs": [...]}`), newest first, with `limit`
(default 50, at most 1000) and `before` (the last id or number seen) for the next page, and how many more match
(`older`).

**Errors.** Every refusal is one envelope, with the HTTP status its `kind` names:
```json
{"error": {"kind": "not_found", "code": "run_not_found", "message": "…", "remedy": "…", "next": ["cozy run ls"], "cause": "…"}}
```
`kind` is `usage` or `validation` (400), `credential` (401), `forbidden` (403), `not_found` (404), `conflict`
(409), `capacity` (507), `unavailable` (503) or `internal` (500). `code` is stable and snake_case; `message` is
for people; `remedy`, `next` (commands to run), `cause` (a peer's own code, such as the Hub's) and `metadata`
(that peer's details of its refusal, verbatim) are optional.
A run's or machine's outcome carries the same typed reason.

**Events.** A stream is `GET …/events` (`text/event-stream`). Each event's `data:` is one JSON object tagged by
`"type"` (snake_case), with no `event:` field. A browser reads a stream from `fetch`'s body, as `EventSource` can't send
`Authorization`. A
resumable stream gives each durable event an `id:` and resumes after `Last-Event-ID` or `?starting_after=`
(the same cursor). A comment keeps the connection alive every 15 s. A stream ends when what it follows has
ended, or when Calcifer stops; a client reconnects from its last `id:`.

**Files.** A file a resource serves answers `GET` and `HEAD` with `Content-Length`, `Content-Type`,
`Accept-Ranges: bytes` and an `ETag`, a single `Range: bytes=…` with `206` and `Content-Range` (`416` when
unsatisfiable, and `200` with the whole file when `If-Range` names anything but its ETag), and `If-None-Match`
naming its ETag with `304`, so a `<video>` seeks it and a browser caches it (`files.rs`, one responder for every
route). Each answers `Content-Security-Policy: sandbox`, and `Content-Disposition: inline` for `image/`, `video/`
and `audio/`, else `attachment`: what a run wrote never runs as script in Calcifer's origin. What's named by its
own hash (a project's `files/{sha256}.{ext}` and `thumbs/{sha256}`, the page's `/assets/*`) answers `Cache-Control:
private, max-age=31536000, immutable`; everything else, `outputs/{name}` included, `no-store`.

## Resources

### The event stream
`GET /v1/events`: what a client keeps on screen, on one stream. Runs and machines go to every connection. Project changes, canvas rows, history and the render board go to connections holding that project through `PUT /v1/events/{stream}`. There is no backlog: subscribe, confirm the projects held, then read their state. It ends only when Calcifer stops, and a type a client doesn't know is for it to skip.

- `hello {version, stream}`, first. `stream` names this stream for `PUT /v1/events/{stream}`.
- `run.updated {run}`: a run's change, as `GET /v1/runs/{id}` answers it.
- `machine.updated {machine}`: a machine as `GET /v1/machines` lists it, each time it changes. `machine.removed {name}`: it left the list.
- `project.changed {id, rev, assets?, gone?, head?}`: `assets` names those whose row is another now, each to read by `GET /v1/projects/{id}/assets/{name}`, and `gone` those no longer there; `head` says something else `GET /v1/projects/{id}?assets=false` holds changed. No `assets`: read the whole project, as a client does at the start and after `resync`. What it costs a client is what changed, whatever the project holds. `rev` is opaque, and changes with each transaction its `project.db` commits, when a file comes to or goes from `files/`. Nothing outside Calcifer writes a project: an edit by hand is an import of its YAML.
- `canvas {id, seq, cards: {name: {x, y, w?} | null}}`: the rows of a project's canvas that changed, in the transaction of revision `seq`. A row written with its asset goes with it, so a client that reads the name has its place.
- `board.updated {project, board}`: a project's render, whole (`GET /v1/projects/{id}`'s `board`, below), when more than one job changed: it began or ended, said something, failed of no asset, or a machine or a shot joined it. A client keeps the board with the highest `seq`.
- `job.updated {project, job, seq}`: one job of a project's render, whole, each time it changes; `id` names it for as long as the render goes, and the board has them all. A client reads the board, then keeps what's newer than its `seq`.
- `ahead.updated {project, ahead}`: what a machine is fetching ahead of a project's queued jobs, as it stands now: `{function, machine, run, stage?, done, total, ready?, held_back?}`. The board's `ahead` has them all.
- `history {id, undo?, redo?}`: a project's history moved, an edit, an undo or a redo, and the entries undo and redo would take now, as `GET /v1/projects/{id}/history` lists them.
- `resync`: this stream fell behind and goes on: read again what you hold.

Newer services add optional `project.changed.cursor {generation, sequence}` and full project-state `covered` with the same shape.
The opaque generation is fresh per controller process; the sequence is assigned by its project-event publisher, not a database revision.
`covered` is captured before any state read/validation supplying that full response. It never comes from a later cursor and is absent on
head-only, failed or partial/error-derived snapshots. A client can avoid a whole project-state reload only when every whole cause is at
or before its cached full snapshot's covered sequence in the same generation. Named/head deltas still apply; YAML/non-project
invalidations, newer/unknown/legacy/generation-mismatched causes, hello/resync and count fallbacks still refresh. A summary or partial
merge cannot promote full-snapshot coverage. Older peers omit or ignore these additive fields and retain ordinary refresh behavior.

`PUT /v1/events/{stream}` `{projects: [id]}`: `200 {held: [id], refused: {id: {status, error}}}`, the projects the page on this stream shows now,
each held open while the stream lasts or until a later `PUT` names it no more. At most 64 sorted, distinct IDs are attempted; an ID outside
that bound is not acknowledged. One that can't open keeps its own refusal without dropping the healthy IDs. The reply comes after Reader
installation; a page orders its first project-state GET after its own held acknowledgement. Older services answer `204`, confirming completion
without an authoritative per-ID result. `404
stream_unknown` once the stream has ended: the next stream's `hello` names it. The page sends what all its tabs show, from the tab that
holds the stream, each time that changes.

ACK-capable pages share the stable `cozy-events-presence-v1` Web Lock and BroadcastChannel. An older open page retains its older
coordination cohort during an upgrade, so it cannot strand a new page waiting for an acknowledgement it does not implement. This is a
coordination protocol boundary, not a build/version pin; tab liveness, media/log locks and ordinary browser storage remain shared.

### The page
The web UI, embedded when cozy is built (`crates/calcifer/build.rs`, from `web/app/dist`). It holds no secret, so these
two routes need no credential; the guard's first three checks still apply.
- `GET /`: its `index.html`.
- `GET /assets/{file}`: a file named by its hash, `immutable`; `404 asset_not_found` for any other.

Each answers `Content-Security-Policy: default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' blob: data:;
media-src 'self' blob:; font-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'` and
`Referrer-Policy: no-referrer`. A build without the page answers `503 page_not_built`, with how to build it.

### Status
- `GET /v1/status`: `{version, os, local_usable, signed_in, heap_bytes}`. `heap_bytes` is what Calcifer's code has allocated and not freed: what it holds, without what its allocator keeps of what was freed, which its resident memory counts too. `os` is `linux`, `macos` or `windows`; `local_usable` is
  whether this computer runs models itself; `signed_in` is whether it holds a device key for its Hub, which it answers
  without asking the Hub (`GET /v1/account` does).

### Session
A browser signs in with a single-use code. `cozy open` prints an authenticated local URL carrying
that code. The browser exchanges it for a session; the code expires after 60 seconds. The
fragment carries `code=<one-use-code>` while the path identifies the project as `/p/<public-code>`. Older `&to=<route>` links are accepted for compatibility. `state.db` holds sessions' digests; 30 days unused ends one.
- `POST /v1/session-codes` (the lock record's bearer only, else `403 session_code_needs_bearer`):
  `{code, url, expires_ms}`, 32 random bytes in hex, good for 60 s. `url` is `http://<Host>/#code=<code>`.
- `POST /v1/session` `{code}`, needing no credential: `{token, media}`, new for each code. `400 session_code_invalid`
  when it's unknown, used or expired.
- `DELETE /v1/session`: `204`, the session whose token this carries has ended.

### Runs
A run is asked for unbound (`RunRequest`); Calcifer resolves its target, binds it against the function's
interface, cuts and stages its files and builds the machine's spec (`crates/calcifer/src/runs`). No client builds
a spec.

- `POST /v1/runs` (`Idempotency-Key`): a `RunRequest`. A run needs a `project`, its id or name, or the folder on this computer holding one: without it `400 not_in_a_project`, an unknown one `404 project_unknown`, and its outputs land in that project's `files/`, listed as its runs'; a file it's given joins `files/` at submit, so it can be asked again once what held it is gone. Answers `202` (`200` when the key had made it) with
  `Submitted {run, replay, notes, preparing}`:
  - `target`: one of Calcifer's own jobs, `cozy/…` ("Jobs"), which needs no project; else `[org/]pkg/fn[@release]` (no `@`: the newest release its machine holds that isn't yanked, else the
    Hub's newest, which it installs; a yanked one moves to the newest with a warning; `x/y` is root package x's function y, else account x's package y).
    With `code`, local code by content (Local code), `target` is the function of it, empty for none named. A target written as a path is `400 target_by_path`.
  - `on` (`local`, a machine, `any`, `new[:sku]`; absent, Calcifer's to say, and no client picks a default: where a render's job would go, the set the browser sends for automatic placement, on whichever quotes the soonest finish ("Placement")), `hub`, `gpus` (a count the machine honours; absent, the machine chooses), `lane`, `attention_kernel`, `project`, `warm`
    (a level, instead of running), `agreed` (a `new` run's quote), `no_memo` (dev-only `--no-memo`: no machine memo answers or keeps it).
  - The payload: `input` (an object) or `input_text` (the same as JSON or YAML text); then `terms` override it (`key=value` typed as declared, `key:=<json>`, `model.<slot>=<ref>`, one
    positional value, input files), `loras` (`[slot=]ref[,strength]`), `files` (`{name?, file}`: an input's
    field path, a label, or none for the declared files) and `fidelities`. A file is a held
    `sha256:<hex>` (`404 file_not_held` when neither Calcifer nor the project holds it), six or more hex digits that start the hash of a file of the
    project, or a path its client sent: `paths` maps each path the request names, as written (`@` before it is allowed), to `{file: "sha256:<hex>"}`, or for a
    folder at a tree input `{tree: "sha256:<hex>"}`, its tree document (`api::tree`), each sent first by `PUT /v1/files`. A tree input also takes a held
    tree document by its `sha256:`. Calcifer reads no client's disk: a path not in `paths` is `404 input_file_absent`. The CLI sends every file a term, `--file` or
    `--input` names that it has, and a folder written as a path; `--input`'s file it reads and sends as `input`, its names relative to it. In a project, a value that mentions a box (`<coast.2>`, `<take3:48-336>`)
    goes through the project's binder, as Render's does: a prompt's references, a slot's file, a range cut here; and `pickup.take=<shot_1.3>`,
    `pickup.out_frame=240`, `pickup.beats_from=7` are Pick up from here.
  - An omitted integer `seed` derives from the key, so a retried key rebuilds the same request. The same key
    again answers the recorded run without binding anything; another request under it is `409`.
  - A value over a limit is no refusal: Runtime clamps a number and cuts text or a list, each in the run's
    `adjustments`, and a list of files is cut here, before they go, with a warning.
  - `notes` are what binding said, in order (`info`: a clip cut; `warning`: fields dropped, a list of files cut,
    a machine with no GPU). `preparing`: the run is local code's install; follow it, then send the same request under the same key.
  - A package of one public function runs it when the request gives it something to run with. Refusals name
    the field: `function_unnamed` (with the commands that name one), `callable_unknown`,
    `callable_recipe`, `request_payload_invalid`, `asset_media_type`, `input_segment_invalid`, …. `local_unavailable`
    (`on: local` where this OS runs no models) comes once the target names a function, or at once for local code.
    `machine_unreachable` records nothing: ask again under the same key (the CLI does, five times with backoff, saying
    each); `machine_offline` and `machine_ended` are the Hub's word, and asking again won't help.
- `POST /v1/run-quotes`: a `RunRequest` whose `on` is `new[:sku]`: what it would rent and its price
  (`RunQuote`), or `null` for one that rents nothing, as Calcifer's own work does. Nothing is rented or recorded.
- `POST /v1/run-estimates?count=` (`Idempotency-Key`): a `RunRequest`, bound as a run is: `RunEstimate {machines, soonest?, notes, preparing?}`,
  each machine it may go to (its `on`, else the render set) as it quotes it now, soonest finish first, for each of `count` copies
  (1 to 32, default 1) sent at once: `{machine, job, fits, refusal?, width, start_ms?, finish_ms?, prepare_ms, queue_position, basis}`,
  ms from the answer, `job` the copy; one that can't run there (`fits: false`) says why in `refusal` and has no times. `soonest` is where the run would go, absent when no machine it may go to can run it. A machine on another minor than this cozy quotes nothing
  (a note says so). Nothing is sent, reserved or updated. Local code not ready answers its install as `preparing`: follow it, then
  ask again under the same key.
- `GET /v1/tools?q=&on=&hub=&project=`: `{tools: [{id, package, function, release, name, label, package_title, makes, description?, local?, fields, ui?, pickup?, model?: {slot, default?, family?}}], note?}`: what can be dropped on a canvas: first the Edit, `cozy/edit`, which Calcifer renders itself, free, with no machine, its one field a `timeline`; then your list, by a card read of the Hub or kept here, so listing installs nothing and it stands with no Hub; then local code and what `on` has installed; and only with `q`, the Hub's, and what you took off your list. Each is every public function of it declared `@app.tool` (its interface's `tool`), never a `quantize`. With `project`, first the local code its assets name, `local` and labelled so, and no other local code is listed. Then what the machine `on` (default `local`) has installed, each package at its newest there, matching `q`'s words in its package's name or its label; then the Hub's search for `q`, or its first page, at each package's newest. `note`: the Hub didn't answer, so only what's installed. `id` is what a box names as its `tool`. `name` is what a new box of it is called, its tool's `named` or its package's word, numbered: `shot_1`, and a taken name counts up. `label` is what it does within its package, its tool's, else its function in words. `package_title` is its package in words, which heads its tools in the page's list: `qwen-image-2` is `Qwen Image 2`, a word with no vowel upper case (`sdxl` SDXL). `makes` is `image`, `video`, `audio` or `text`: its result's first picture, video or sound, else a text. `ui` is the path in its package of a component that draws its box, where it ships one (`PACKAGE-UI.md`). `pickup` `{every, offset, first}`: it takes a pickup, which Pick up from here fills, landing on `offset + k * every` from `first` (`Snap`), or any frame `{1, 0, 0}`. `fields` are its struct's fields in order, each `{field, type, label, required?, default?, auto?, placeholder?, advanced?}` plus by `type`: `words` (a `str`); `prompt` (text with mentions; `takes?: ["image"]` when it takes only those); `toggle` (`auto` for `| None`); `number` (`min, max, step?, unit?, integer?, total?`, `auto` for `| None`; an unbounded number is no field); `choice` (`options`, the values in order, `words?` and `descriptions?` maps by value from `Label`); `shape` (the box's own shape, `ratios?` or `min?, max?`); `slot` (`accept: image|video|audio`, `described?` for a file with its description, `multiple?`, `spans?`, the list whose timeline a video's length is); `group` (`members`, nested as deep as the struct, `list` for rows with `min_items?, max_items?`); `seed`; and the Edit's `timeline`. `advanced` is `Advanced()`: under More, folded. A field's `label` is `Label`'s, else its name in words. A pickup is no field, and neither is `tool`, `approved`, `description`, `models`, `lora`, `text` or a type this cozy can't draw. A box's `fields` hold what's written: a slot's value is a mention, `<coast.2>`, a list when `multiple`.
- `POST /v1/tools/list` `{add?: "<package>", remove?: ["<tool id>"]}`: `{packages, without?}`, your list, in `state.db`, the same in every project: Qwen Image 2 and Minimax H3 until it's changed. Local code is a project's, named by its boxes: a path is `400 tool_list_local`. `add` puts a package's media functions on it, all of them again if some were taken off. Neither installs or deletes anything, and a box of a tool taken off goes on as it was.
- `GET /v1/tools/browse?q=&limit=&hub=` (1–200, 50): `{packages: [{package, summary?, tools}], older}`: the Hub's packages that make media, for adding to your list. `older` is how many more a bigger `limit` reads.
- `GET /v1/tools/{id}/ui?project=`: `{module}`: where to `import()` that tool's component from, its bytes in hand. `{id}` is one percent-encoded segment. A release's come from the Hub, for the newest any machine you hold has installed, its wheel held to the Hub's digest; local code's from the files `project` names, as they are now. `404 tool_ui_none`: it ships none. `404 tool_ui_not_installed`: no machine you hold has it, or `project` names no such local code. `404 tool_ui_missing`: its package doesn't hold what it names. `503 tool_ui_hash_mismatch`: the wheel isn't what the Hub says. `400 tool_ui_too_big`.
- `GET /v1/tools/{id}/ui/{hash}/{*file}`: a module of a component as `{module}` named it, `application/javascript`, under the page's CSP, `immutable`, needing no session: `import()` can't send one, and the hash is what `GET /v1/tools/{id}/ui` said. Calcifer holds what it read in memory, so after a restart a page asks again. `404 tool_ui_file_not_found`.
- `GET /v1/targets?target=&hub=&on=&tree=&at=`: what a run of `target` on that machine calls, resolved as `POST /v1/runs` resolves it; with `tree` and `at`, local code's `code`, `target` its function:
  `{package, function?, release, release_from: asked|installed|latest|local, summary, interface, functions, contract?, request?, next?}`:
  `interface` is the package's interface document. Local code's is what its machine's Runtime said installing this very source, else read from its source by the Runtime release its uv.lock holds, the newest within its unconditional `cozy-runtime` version bounds, or PyPI's newest when neither is specified. No package code runs: `400 describe_failed` when that Runtime can't read it, with `next` the run that reads it with the machine's own; `functions` `[{function, description}]` is what a
  person runs of it, with no internal function or recipe. A named function is checked as a run checks it, and `contract` is its contract as people
  read it, `request` its schema. `next` names the functions to describe.
- `PUT /v1/files/sha256:<hex>`: the one way a client's file reaches Calcifer, for a run, an import, a model or a machine update to name by its digest. Its
  bytes, which must hash to its name: `201`, or `200` when held, with
  `{digest, length, media_type}`. With `Content-Range: bytes a-b/len`, one part: `204` with `Cozy-Held: a-b,c-d` until it is whole and
  verified, then as above. Parts land in any order, side by side, and what a body carried before it broke is held. A whole `PUT` carries
  its `Content-Length` (`400 file_length_required`); `400 file_digest_mismatch` drops the bytes; `409 file_length_conflict` when it is
  being written at another length. `HEAD`: `200` held whole, else `404`, with `Cozy-Held` either way; a client sends only what it lacks. A held file or part unused for a day
  is dropped, and a `HEAD` that finds it is a use.
- `GET /v1/runs` (`status`, `package`, `machine`, `project`, `before`, `limit`): `{runs, older, machine?, unreadable?}`. `project` is an id or name (`404 project_unknown`), or a folder of this computer's:
  that project's runs. Each run names its `project`, the project's name, and `project_id`.
- `GET /v1/runs/{id}`: the run. While it waits, `queue_position` (1 is next); while it runs, `progress` with
  `done_fraction`, `bytes_per_s`, `step_steady_ms`, and its time left: `eta_s`, `eta_confidence`, `eta_basis` ("Time left"). Each output, once it landed, has `url`, the API path that serves it, and `sha256`, and `width`, `height`, `duration_ms` as measured when it landed, where they apply and ffprobe is here. `?detail=full` adds `measurements`, `calls`, `report` (each
  executor's stages and unaccounted time, models, kernels, and each GPU's memory, with rates against its
  ceiling) and `request {target, input, seed?}`: what it was asked, read-only, each file by its `sha256:` and the models and LoRAs it chose as `models`. To ask it again, send `target` and `input` with a new key.
  A run settled over 30 days ago has dropped `measurements` and each call's, so its `report` is gone. Nothing else
  of a run is ever dropped: its row, `result`, reason, outputs, logs and `calls` stay for good.
- `GET /v1/runs/{id}/events` (`Last-Event-ID` or `?starting_after=`): `run.*`, `output_item.*`, `run.log`,
  `run.call`, and the live `run.progress` and `run.waiting`. It ends after the terminal event once every output
  has landed here or failed to.
- `POST /v1/runs/{id}/cancel`: `202`, or `200` when it had ended.
- `GET /v1/runs/{id}/play?output=&expires=` (`expires`: `90s`, `30m`, `24h`, `7d`, default `1h`): `Play {address, fingerprint, token, url, expires_s}`: how a browser plays that output straight from its machine, and `url`, the hosted player's link. `400 play_expires_invalid`, `400 play_output_invalid`.
- `GET /v1/runs/{id}/outputs/{name}` (`?index=` an item of a list): an output's bytes, where its `url` says. A folder's is its tree document (`application/vnd.cozy.tree-manifest`), and `?member=<path>` one file of it: a client fetches what it asks for and holds each to its digest (`cozy run -o`, `cozy run show -o`). An output names no path of Calcifer's. An item with `stays`, an output its function's result declares `Latents`, stays on its machine: no `url`, nothing waits for it, and this answers `409 output_on_its_machine`.

### Time left

Calcifer says how long a run has left, all of it, so the page, `cozy run`, `run show`, `run ls` and a machine's activity show one
number. `eta_s` is the seconds left, `eta_confidence` 0 to 1, and `eta_basis` what it stands on: `machine` (runs like it on this
machine), `gpu` (on other machines of its GPU, by the name each run's record keeps as `gpu`), `steps` (its own steps alone), or
`first_run` with no `eta_s` (nothing like it ran here, and it has fewer than two steps of its own).

- **Prior.** The newest five completed runs of its target (any release) on its machine, else on its GPU, each split into its phases:
  its start and load (to its first stage), each stage its executor names (`encoding prompt`, `generating image`, `decoding image`, …), a
  stepping stage's first step and steady pace (the median of its steps after the first), and the tail to its end. Each phase is the
  median of theirs. With `megapixels` (or `width` and `height`) in its input, runs within 15% of that size, else all of them with their
  steps and what follows scaled by megapixels. The Hub keeps no timings per SKU, so there's no third tier.
- **Live.** Before its execution starts, the prior's whole time, plus a download's bytes left at its rate. Then the prior for each phase
  not reached, what's left of the one it's in, and for a stepping stage its steps left at a pace blended from the prior's and its own:
  `(w·prior + n·own) / (w + n)`, where `own` is TensorD's `step_steady_ms` (the median of its last five steps after the first), `n` the
  steps it has after its first, and `w` 3 for this machine's history, 2 for its GPU's, halved when scaled. Its first step is never its
  pace: with no prior and under two steps, it says `first_run`. A phase that overruns counts as done until its next starts.
- **Smooth.** It's said again each second as it counts down. A new estimate within 10% (or 2 s) of what was shown is shown as it is; a
  bigger one is eased to over about a second, never jumped to.
- **Quoted.** A run sent where its machine quoted it ("Placement") counts down to at least that quote's finish before it starts. With no
  prior, the quote's run time is one: `machine` when the machine measured it, `gpu` when it stood on Calcifer's `prior_ms`.

### Placement

The machine owns its GPU width (cl-816): a lone job runs wide, several queued jobs one GPU each; Calcifer never picks a width and sends
`gpus` only when a person asked for a count. It sends each job where it finishes soonest:

- Each machine a job may go to is asked once per batch (a render's turn, or one `cozy run`), through TensorD's `POST /v1/estimate`
  (capability `estimate/1`): each job by `function` (`org/pkg/fn@release`, local code `sha256:<manifest>/fn`), `models` (each checkpoint's
  lane commit and bytes, from its Hub card), `size` (`JobSize::of` its payload), `gpu` (it or a call it makes builds a model on a GPU) and `prior_ms`
  (eta's time for it on one GPU of that kind, from runs at one width). Its quote is packed with that machine's queue as if the whole batch
  went there; its relative ms are added to the time the answer came.
- A machine that can't run it at all (`fits: false`: no GPU for a function that builds a model on one, its weights too big for the disk) has no finish
  and is never chosen. Of the rest the soonest finish wins; within 2% (or 1 s), one whose quote stands on its own measured runs before one
  that doesn't, then a rental before `local`. `local` takes nothing it would do poorly (as `fit` judges) while another can, unless its
  quote stands on its own measured runs.
- Only a machine on this cozy's minor quotes ("Machines on this minor"); one on another is updated first and quotes on the next turn.
- With several machines, a job a machine would start more than a second after its preparation waits here, quoted again as the render
  moves or every 10 s, so it commits to no queue early. A quote reserves nothing: two clients picking one machine are settled by its queue.
- `any` and `new` are never quoted: the Hub keeps no times per SKU. They go as before (`any` the least busy rental, `new` a rental).

### A run that stopped moving

A running run with no progress for 10 minutes carries `stalled {since_ms, machine, usd_micros_per_hour?}` until it
moves, and its record gets one `no_progress` warning naming the machine and its price. Nothing ends it: only its owner does.

### Machines on this minor

Work goes only to a machine whose TensorD is on this cozy's major and minor (Paul, 11-07: pre-launch, no support for older
machines). Before a run is sent, and before a machine is quoted for work going now, one on another minor is updated to this cozy,
the update `POST /v1/machines/{name}/update` makes with `tensord` (once per boot of it, for all that send there): `local` to the cozy
installed here, another to this release, or a dev build's own binary on a machine of this computer's platform. What is
sent there waits, saying `updating <name> to cozy <release> first`, and `cozy run` follows the update (`preparing`), then asks again.
A rental boots its image's TensorD, so one on an older minor is updated before its first run. One added from another computer, or
with runs going, is left as it is: the run fails `409 machine_other_minor`, its `next` `cozy machine update <name> --tensord
<release or this binary>`; so does a failed update. Every run goes in one admission, `POST /v1/runs/batch` (one run is a batch of one); an update
alone goes by `POST /v1/runs`, which every TensorD takes.

### Machines and rentals
<!-- cl-716 C -->
A machine is this computer's (`local`, started when used), a rental, or another computer's added by address.
`{name}` is a live machine's name or former name, or any machine's rental id. Records: `api::calcifer`.

- `GET /v1/machines[?refresh=true]`: `MachineList {machines, hubs, unanswered, signed_out, rentals}`. It lists
  every live machine, every live rental of this account among them: one another client ordered is reached with this
  computer's device key, reads `ordered: false` and takes no run that doesn't name it.
  `refresh` lists every Hub again first. `rentals` sums the live rentals: count, `usd_micros_per_hour`, and
  `accrued_usd_micros` on its weakest `accrued_basis` (`accrued_floor` when some Hub said nothing).
  A machine's `status.activity` is what it does for this computer's runs besides their own work, as its Status says,
  and moves on `machine.updated`: each `Operation {id, kind: download|install, subject, stage?, bytes_done, bytes_total,
  bytes_here, bytes_per_s?, eta_s?, started_at_ms, runs}` once, however many runs wait on it. `subject` is the model's
  repository (or provider source), or the package; a download's bytes are what it fetches, and
  `bytes_here` what of it was here already; `runs` are the run numbers waiting on it.
- `GET /v1/machines/{name}[?fresh=true]`: `Machine`. A rental rented with an SSH key has `ssh {args, line}`: the command that logs in, with a known-hosts file of its own, as an argument list and as a line to paste. `fresh` reads its Status now, or refuses
  `machine_unavailable`.
- `POST /v1/machines` (`NewMachine`):
  - `{"kind": "rent", sku, hub?, gpus?, disk_gb?, ssh_key?, agreed}`, with an `Idempotency-Key`. It answers
    `202 Rented {machine, quote, replay}`, or `200` on a replay. Follow the boot on the events until the
    machine is `ready`, or `ended` without one. `agreed` is the quote's `agreed`, sent back as it came; the order is
    capped at it (the Hub's `max_usd_micros_per_hour`, the quote's price when absent), and a dearer one rents nothing
    (the Hub's `409 rental.price_rose`).
    - A Hub that does not answer is asked again under the same key, across restarts, so one order buys one
      pod at most. Until it answers, the order is a `booting` machine in the list, under the name it was asked
      with and with no `rental_id`. Another rent of the same thing joins it (`503 rental_order_pending`).
  - `{"kind": "add", name, address, fingerprint}` answers `Machine`.
- `DELETE /v1/machines/{name}[?force=true]`: `Ended {machine, changed, abandoned}`, `202` while `ending`. `local` is
  refused `machine_local`.
  - The end is recorded at once and survives a restart. Calcifer carries it out once the outputs of the
    machine's runs have landed here, and `machine.ending` says what it waits for.
  - `force` ends it now; `abandoned` names the outputs given up. Asked again with `force`, it stops the wait.
  - A rental is `ending` until its Hub affirms the pod gone, then `ended`. An added machine is removed.
  - An ending machine takes no new runs (`409 machine_ending`), and `local` is refused.
  - An order its Hub has not answered stops at once (`ending`, no `rental_id`): it buys nothing more, and leaves
    the list once its Hub answers and anything it placed is ending. So does one whose run ended, or that its Hub
    has not answered for an hour.
- `POST /v1/machines/{name}/keepalive`: `Keepalive`. It restarts a rental's 15-minute idle clock once the
  machine acknowledges.
- `POST /v1/machines/{name}/update` (`MachineUpdate`), with an `Idempotency-Key`: the machine's Runtime,
  TensorFS and TensorD (its `cozy`) in one update run and one restart. Any other run for that machine waits here for its end, saying `getting <name> ready`, and goes to what it became.
  - Each component is `{"version": "0.33.1"}` or `{"file": {sha256, name, version?}}`, a dev build sent first (`PUT /v1/files`): a wheel
    `name`d `<distribution>-<version>-….whl`, or a `cozy` binary and the release it says it is (`cozy --version`), when it says one.
  - A wheel version that PyPI lacks is fetched from its GitHub prerelease, matched to the machine's platform.
  - With nothing named, each component moves to the Hub's target: always when the target is pinned, else only
    forward.
  - On `local`, a `cozy` that `cozy update` installed goes along with the update.
  - It answers `Updating {machine, run?, target, warnings}`: `202` with the run, which is followed like any
    other; `200` with no run when the machine already runs the target.
  - The run fails `update_unapplied` unless the machine then runs what it asked for.
- `GET /v1/machines/{name}/packages[?fresh=true&hub=]`: `MachinePackages {machine, packages: [{machine, package, release, runtime, level, hub, source, warning}]}`. `source` is
  `local`, `hub`, or another Hub's URL, for a client of `hub` (default this computer's).
- `GET /v1/machines/{name}/models[?fresh=true]`: `MachineModels {machine, models: [{machine, model, release, lane, precision, checkpoint, bytes, shared_bytes}], silent?, repositories, bytes, stored_bytes}`.
  `silent`: it has reported no Status yet. `repositories` and `bytes` are its store's, shared content counted once;
  `stored_bytes` is all its store takes on the disk, models or not (0 from a machine that doesn't measure it).
- `GET /v1/machines/{name}/logs/{tensord|tensorfs|memory}?lines=N&since_ms=T`, and on `local` `logs/calcifer`, Calcifer's own (`400 log_not_kept` elsewhere; the CLI reads its file, so it works with Calcifer down):
  - The newest `N` whole lines (`0`: all), as `text/plain`. `since_ms` drops the lines before the first one
    stamped at or after `T`.
  - With `Accept: text/event-stream`, each batch follows as `{"type": "lines", after, text}`, with `id:` set to
    `after`. Resume with `?after=` or `Last-Event-ID`.
  - A machine that cannot answer is answered from the copy Calcifer kept.
- `PUT /v1/machines/local/authorized-keys/{key}` and `DELETE …/{key}` answer `Authorized {key, file, changed,
  machine, image}`: the controllers of other computers that this computer's machine admits.
  - `key` is an Ed25519 public key, 32 bytes of unpadded base64url.
  - The answer comes at once. `local` reads its keys again on the call this makes, then listens on every
    interface once it is idle, and `machine.reach` says where other computers add it.
  - A removed key's next call is refused.
- `GET /v1/rental-skus[?hub=]`: `RentalSkus {rental_skus}`. These are the Hub's products for rent now,
  cheapest first by their fewest GPUs. A product the Hub cannot sell now, for any reason, is not listed.
- `POST /v1/rental-quotes` (`NewMachine` rent): `Quote`, the Hub's one price for exactly that order: `usd_micros_per_hour`, the
  machine with its disk and traffic, what a person is shown, and `agreed {usd_micros_per_hour}`, the cap an order sends back. So does `RunQuote.quote`. Nothing is
  rented.
  - First it admits the product at that GPU count, never widening it to another: `400 sku_unknown`, or
    `507 sku_out_of_stock`, `sku_cooldown` or `sku_retry_in_progress`.
  - `POST /v1/run-quotes` admits a product the run names the same way.
- `GET /v1/rentals[?since_ms=&hub=]`: `RentalHistory {since_ms, rentals, days, total}`.
  - It covers every rental of this account at the Hub, ended ones and those rented from any computer, newest
    first, with what each cost (`spend_basis`: `charged`, `unmetered` or `estimate`).
  - Sums are per UTC day and in all. It is not paged: `since_ms` is the window.
  - Provider cost is present only when the Hub tells it, which it does for an operator.

### Packages, models, account and orgs
Calcifer speaks to the Hub as this computer's device key (`~/.cozy/keys`); every call names its Hub
(`hub`, `?hub=`). The Hub's refusals pass through with its code as `cause` (as the code itself where
a client branches on it: `account.name_unavailable`). Records are `api::calcifer::hub`.

**Account.** A key is enrolled by a code the Hub emails: the client reads the code from its user and
sends it back. A code lives 10 minutes, and a wrong answer leaves it waiting.
- `GET /v1/account`: `Account`, who this computer signs in as, its credit (`billing`,
  `balance_usd_micros`, none where unmetered) and where to buy more; `key_refused` when the Hub no
  longer accepts the key.
- `POST /v1/account/codes` `{hub, purpose, email}`: `Code {id, email, retry_after_s?}`. `purpose` is
  `sign_in`, `export` (a key for another computer, such as CI) or `sign_out_everywhere` (mails the
  signed-in key's address; `not_logged_in` without one). Asked again before the Hub will send another,
  `id` is the code already out and `retry_after_s` when it will, the wait the Hub stated; with none out
  it's `503 rate_limited` with the same `retry_after_s`.
- `POST /v1/account/codes/{id}` `{code}`: `Proven`, tagged by `purpose`: `sign_in` keeps the key and
  answers the `Account` (`account` none until named); `export` answers the key file's text, kept
  nowhere here; `sign_out_everywhere` revokes every other key, then this one (`SignedOut`). Each email
  has its own code, so what arrived is tried on `id`'s and then the 2 newest others to that mailbox for
  that purpose. `400 code_wrong` when none takes it, all still waiting; `404 code_not_found` once they've
  expired or one was answered. A Hub refusal carrying `metadata` is a step of AuthKit's own (restoring a
  deleted account): it passes through with that `metadata`, and the code waits on it under `id`.
- `POST /v1/account/codes/{id}/step` `{path, body}`: takes that step. `body` goes as is to `path` under the
  Hub's AuthKit mount (`account/recovery/confirm`), with no credential; then the waiting code finishes, as
  `POST /v1/account/codes/{id}` answers. `400 step_path_invalid` unless `path` is plain lowercase segments;
  `404 code_not_found` when no code of `id`'s waits on a step.
- `PUT /v1/account/name` `{hub, name}`: names a new account; ask again on `account.name_unavailable`
  or `account.name_invalid`.
- `PUT /v1/account/key` `{hub, key}`: an exported key's text becomes this computer's key.
- `DELETE /v1/account?hub=`: `SignedOut`. The key is revoked at the Hub and erased here, whatever the
  Hub answered (`notes` say when it did not confirm).

**Orgs.**
- `POST /v1/orgs/{org}/claim` `{hub}`: `OrgClaim`, the code and where to post it, until a handle the
  org lists shows it.
- `PATCH /v1/orgs/{org}` `{hub, name, links, handles}` (platform admins): `Org`. Each field given
  replaces the org's own; an org that does not exist yet is parked.

**Models.** A root model's org is `_`.
- `GET /v1/models?q=&family=&tag=&no_tag=&limit=` (1–100, 20): `ModelSearch {models, total, older}`, each
  model's newest release, lanes and `tags`; signed in, also the caller's private ones. `q` matches names
  and tags, and exactly `org/name` is that model. `tag=` and `no_tag=` (comma-separated) keep models with
  every one, and with none (`no_tag=nsfw` leaves out models that make sexual or nude images). Each says
  what its header says, read once from the Hub and kept, fetching no weights (the Hub doesn't say it yet, tensorhub th-367): `kind` is `lora` (only a LoRA's
  factors), `weights` (a model's: its header verifies as a family) or `component` (weights no family's
  model is: a text encoder, a VAE, an upsampler), absent if it couldn't be read. Its `family` is its owner's label, else
  what its header verifies as; `family_from` says which (`owner`, or `weights` when its header verifies
  as it). `family=` lists every model the Hub labels so, page by page (`limit` of them), and the others
  (up to 1000 read) whose header alone says it.
- `GET /v1/models/{org}/{name}?release=`: `ModelDetail`, its releases (the one named) and, to its
  org's members, every commit; `family`, `family_from`, `weights_family` (what its header verifies as),
  `kind` and `tags` as above.
- `PATCH /v1/models/{org}/{name}` `{hub, family, tags, add_tags, remove_tags, trigger, strength}`:
  `ModelEdited {model, warnings}`, the model as `GET` reads it. Each field given is set, an absent one
  kept: `family` empty or `none` clears the label (its weights' family shows then); `tags` replaces them
  (empty clears them), then `add_tags` are added and `remove_tags` removed; an empty `trigger` and a null
  `strength` clear. The Hub fills a model's family from its header and its tags from its source's page;
  what its owner sets here wins.
- `GET /v1/families`: `Families {families}`, what a header can verify as: the owner's picks.
- `POST /v1/models/{org}/{name}/releases` `{hub, version, commits}`: `ModelReleased`, one commit per
  lane under a version higher than every earlier one. The Hub never reuses a version, so it takes no
  `Idempotency-Key`.
- `DELETE /v1/models/{org}/{name}`, `…/releases/{version}`, `…/commits/{commit}` (`?hub=`):
  `ModelDeleted`.

**Jobs.** What takes long is a run (cl-841), asked as any is: `POST /v1/runs` of a reserved target of the package `cozy`, its
fields as `input` and `on`, `hub`, `code` and `agreed` the request's own, `202` with the run (`Submitted`); quoted by `POST
/v1/run-quotes`; listed, shown, followed and canceled as a run. `terms`, `files` and the like are `400 job_input`. With no `on` it goes
to this computer's machine, else any rental there is. Records: `api::calcifer::hub` (`job`). What Calcifer does once a machine
completes one happens before the run settles and is in its result (`result.after`, `AfterRun`), so a client that stops following never
skips it. Some are Calcifer's own work, on no machine (`kind: here`, `machine` none): each says what it does in the run's log, its
result is its record, one that fails keeps the refusal whole as `result.error`, a cancel drops it where it stands (after the step in
hand), and one a Calcifer stopped under fails `calcifer_stopped`: asking again is safe.
- `cozy/model_download` `ModelDownload`: a warm run that
  makes a model present in a machine's store. A `.safetensors` file of a client's is held (`PUT /v1/files`), named by its `sha256:<hex>`
  and its client's `name`, and kept under an alias, `local/<name>`, its name's stem when none is given.
- `cozy/model_upload` `ModelUpload`: a warm run that makes
  the source and uploads it as a private commit of `destination` in `lane`; its mirror is recorded before it (`mirror`), and `notes`
  say so. `source` is a provider source, a Hub model, a
  `local/` alias, a held file's `sha256:<hex>` (`PUT /v1/files`, `404 file_not_held`; `name` its client's name for it): a `.safetensors` file, or a
  CozyTensors folder's tree document, its files held too, or `run:N[#output]`, which runs
  on the machine that holds the output. A commit, `org/name@sha256:…`, on no `base` is at the Hub
  already: it's recorded as a commit of `destination` when the caller may read its whole closure,
  checked against `lane` before anything is recorded. That is Calcifer's own run, its result a `RecordedCommit`, and its quote
  `null`, since nothing rents. Its trigger and strength (the flags, else the source's own)
  are set at the Hub once it completes (`result.after.display`).
- `cozy/model_quantize` `ModelQuantize` (quoted for published
  packages only): Calcifer picks the commit (a release's original weights, else the commit named),
  reads its header's components, matches the package's recipe for the lane and the calibration
  models it builds, and calls the package's `quantize`. The package is `package`, or local code's `code`. Local code not yet ready
  answers its install (`preparing`): follow it, then send the same request under the same key.
- `cozy/package_install` `PackageInstall`: a warm run of a
  package's environment, no weights; the Hub's newest release when none is named. Once it is in, the
  older releases of the package the machine holds from the same Hub are removed, one remove run each
  (`result.after.retired`). Local code goes by content as `code`, and retires nothing; what its machine read of it is kept
  as a run's install keeps it, so a read of it next (`GET /v1/tools`, `fit`) has it at once.
- `cozy/package_remove` `{packages}`, `on` one machine: Calcifer's own run, which sends one
  remove run per package and Hub the machine holds it from, every release, and sums them: `PackagesRemoved {machine, removed,
  reclaimed_bytes, retained?}`. A name the machine does not hold is refused before anything is recorded; a remove run that
  does not complete fails it `package_remove_incomplete`, saying what went.
- `cozy/package_lock` `{root, upgrade}`, of `code`: Calcifer's own run. It locks local code a client sent (Local code) with uv
  (fetched when missing), in its copy, never running its code, for the Hub and account it publishes to (`root`: the root
  namespace), and writes nothing into the client's folder; its copy leaves Torch to the machine (`exclude-dependencies`: torch,
  torchvision, torchaudio, triton; a cap or exact pin on one is a notice). Its result is `Locked {lock, account_index, changed}`,
  `lock` the new `uv.lock`'s text, which the client writes into the package's folder (`changed`: it differs from the one sent).
- `cozy/package_publish` `{root, yanked}`, of `code`: Calcifer's own run. It locks first when it has no `uv.lock`, answering that
  one as `lock` for the client to write; stages, relocks for this Hub's account
  index, builds, carries its git dependencies' wheels, describes, uploads what the Hub lacks and
  finalizes (`yanked`: committed yanked, never the newest release). Its result is `Published`
  (`status: already published` for a version the Hub holds).

**Packages.**
- `GET /v1/packages?q=&limit=` (1–200, 20): `PackageSearch {packages, total, older}`.
- `GET /v1/packages/{org}/{name}?release=`: `PackageDetail`, releases newest first; one named comes
  with what its publish warned of, to its owner.
- `POST /v1/packages/{org}/{name}/releases/{release}/yank` `{hub}`: `PackageYanked`; machines keep it,
  only an exact pin installs it. `DELETE` on the same path un-yanks it: `PackageUnyanked`.
- `DELETE /v1/packages/{org}/{name}?hub=`: `PackageDeleted`, every release, its name free again.

### Local code
Local code is a package folder or a PEP 723 script nobody published. It comes by content (cl-819), as `code` `{tree, at}`
on a run, `GET /v1/targets`, and the jobs that install, quantize, lock and publish it: `tree` is a tree document (`api::tree`, each
file's `executable` kept) of the package and the path dependencies its sources name, under their common folder, each file
and then the document sent first by `PUT /v1/files`; `at` is the package's folder or script in it (empty: the tree itself).
What goes is `api::code`'s rule: no VCS, caches, virtualenvs, credentials, keys, weights or links; the CLI says what it
left out. Calcifer copies the tree once from what it holds, into `~/.cozy/calcifer/code/<digest>/` (a script's package
written beside it), keeps the newest 64 a day, and works from that copy: it reads no client's folder. A project's own
`with: ./dir/fn` is a folder of Calcifer's computer, read into a copy the same way. Calcifer keeps its manifest under
`~/.cozy/calcifer/local/` and makes it ready on the machine a call goes to: an install run first when that machine lacks it,
then the call with its files. Every machine, `local` too, is sent the files by content (only what it lacks); none reads a
host path. Its interface comes from Runtime's static describer on the project's Python: the exact cozy-runtime its lock holds,
else the newest within its unconditional version bounds, else PyPI's newest. Only plain PEP 440 bounds on the normalized
`cozy-runtime` name select that SDK; extras stay package dependencies. Conditional requirements, direct URLs and unspecified
versions retain the newest-SDK fallback. Project URLs, paths, git sources and indexes are never forwarded as the SDK requirement.
Invalid plain bounds return `400 runtime_requirement_invalid` before the static reader command runs. The static reader executes no package code and
creates no project lock.

The source bundle includes project files, including Git-ignored runtime resources. Known credential
names (`.env`, `.env.*`, `.envrc`, `.direnv/`, `.aws/`, `.ssh/`, and named key/credential files),
known key/weight suffixes, bytecode and symlinks are excluded and listed as left out. This filter does not
detect secrets stored under other names; keep those outside the package source folder.

### Projects
A project is a folder holding `project.db`, `files/` and its edits at its root (`<name>.mp4` and `<name>.otio`). `project.db` (SQLite, WAL;
`PROJECT-DB.md`) holds all the project says and has made: its boxes and their fields, their rows on the canvas, versions and takes, what is
measured of each file, what its render owes, its history and its request keys. Calcifer is its only writer, and each action below is one
transaction: what it writes, its history entry and its key commit together, or none does. Its id and name are kept in `project.db`
(cl-819): the folder is only where it lives, on Calcifer's computer. A new project's id is 128 random bits; one from before keeps the id its
path gave it. A moved folder is the same project, its walk going on, and the index follows it; a copy of a project another folder still
holds takes a new id, and its walk's paid work doesn't go on. Calcifer holds a project open, its parts, history and `project.db`
connection, while anything reads it: a call on it, a page showing it (`PUT /v1/events/{stream}`), its render (until
nothing is owed, waiting for a machine too). It closes when the last of these ends, and the next call opens it again (cl-823). `POST
/v1/projects` makes one. Names are unique in the index (`_2`), and every route's `{id}` takes a project's
name or id, or its folder on this computer, percent-encoded (`api::calcifer::project::NAMED_BY`): never the folder the client is in. A folder holding a project the index
doesn't list is listed by the first call that names it. Every file the
project uses is a real file in `files/<sha256>.<ext>`, flat, and a file the database has a row for says only what a file can't: where it
stays and its parts. YAML is its export (`PROJECT-YAML.md`): `GET …/yaml` writes it and `PUT …/yaml` reads it back. A folder of the files
from before (`project.yaml`, `project.lock`, `project.canvas.yaml`, `.measured.json`, `.history.json`, `.keys.json`, `.runs/walk.json`) is
imported on its first open, a format 2 or 3 file upgraded to 4 first, and they're deleted.

An asset's state is its `approved` hash (a hash of a version's main file brings its whole file set), else its
newest version whose `inputs` match now, else none; with no `tool`, its newest version, which nothing puts out of date. A box is its words, an optional tool, and its versions: with no tool and no versions it's a text asset, whichever of `text`, `summary` or `instructions` holds its words. `inputs` is blind to names: eight hex digits of its `tool` (no release, no `_/`), `models`, `lora` and
its fields as written, after defaults, each box they mention replaced by its number in order of first mention (`<Amy.2>` is `<#0.2>`), then twelve for each of those boxes in that order: a box's `description` and the files of each version read, by sha256, or a text box's words, numbered on. So a rename puts nothing out of date, and a description edited does, `<Abr.2>`'s too. What a package is given has each text box's words in place. A tool Calcifer runs holds only what it reads of its inputs, and the files. A read gone from the project is no change. The seed, a pickup, the release and the machine aren't in it.

A mention reads an asset's state, and moves with it: `<abr>`. `<abr.2>` reads that version, fixed, its number never reused, and `<self.2>` a version of the asset whose text it is. Neither is a dependency: what's named is a file that's there, so it waits for nothing, closes no cycle, and a render of the asset that reads it makes none of what it names. A name holds no `.`: `<name.N>` is a version, `<name.word>` an extra output of its version (`<page.cleaned>`), and `<name:48-120>` frames 48 up to 120 of a video, `<name:48->` to its end, cut here before it's sent. Names match in any case, with `_` and `-` alike. `<self>` alone would change its own input on each render, so it fails like a mention of nothing, `input_missing`, with the fix; so does a version deleted or never made, whose name is in `missing`.

An operation is a tool (`[org/]package/function[@release]`, or local code, `./dir/function`), and its `kind` is the file type it produces, `image`, `video` or `audio`, by its output, its file or its version, never by the file saying so. `x/y` is root package x's function y where the Hub has one, else account x's package y, as `GET /v1/targets` resolves it. Its function is called directly, with no CPU run first, by one binder, the same for the browser's Render and `cozy run`, by the fields' types: a `str` takes words, a mentioned box with a file going as its name, with a warning; a prompt goes as `{text, references}`, its text as written with each mention spelled as its box spells itself, and `references` a list in order of first mention, `[{key, type, file, description}]`, `key` that spelling (`Amy`, `Amy.2`, `walk:48-120`), only of the kinds it takes: another goes as its name, with a warning; a slot takes only the file written in it, with its `description` and its version's extras of the same name (`latents`) when it's a `described` slot and the same package made it, and `own` true for one of the box's own versions; a slot written with a file of another kind is left empty for that render, with a warning naming it; a pickup comes only from `runs`' `pickup`, never the file: `{take, out_frame, …}`, its frame on the tool's `Snap`; a text box is put in as its words, anywhere. A tool naming a package with one public function that makes its kind calls that one. A number past its bounds is clamped, a choice it doesn't offer is its default, a shape the nearest it takes and a list cut to its count, each with a warning; a mention of nothing, a required field left empty, or one of a type this cozy can't fill (`next`: `cozy update`) fails that box before anything is sent. A failed call stops the list and makes no version; a function taking `seed` gets the one Calcifer drew, kept for the next try when the
`models` and `lora` go on its call: a tool's `model` is what they mean with no slot named, the one model its function runs, its `slot` the key in `models` and `family` what the header of its `default` says. A tool whose function runs several, or none, has no `model`. The version records its run, and each file by its output's name. The package `cozy` is reserved: `cozy/edit`, a video, and `cozy/frame`, a picture, run here, free, with no machine, once what they
read settles and on `approve`, reading each asset's state, else its newest version, which they say. An edit keeps its versions, each with the clips that made it, and the same edit isn't rendered again; a frame keeps one and its approved one.

- `POST /v1/projects` `{name, path?, starter?}`: a
  new project called `name`, at `~/<name>` in this person's home folder, through `as_name` with `.` as `_` (`untitled` when nothing is left),
  taking `<name>_2` when anything is there, a project or not, which stays as it is and `said` says so; or at `path`, a folder of this
  computer's by its absolute path (`409 project_exists`, its name the folder's unless given; `400 project_path` for a relative one); `starter`: it holds the starter film. The same
  `Idempotency-Key` again answers the same project. Answers `{id, path, name, next, said?}` and records it in the index as used now. An existing
  project needs no call here: a route names it, and `404 project_unknown`, `409 project_ambiguous` (a name two projects share, which they never
  do once each is opened here) or `404 project_not_found` (a folder in none) refuse one that names none. A folder of
  files from before `project.db` is imported by the first call that names it, a format 2 one upgraded first, and `GET /v1/projects/{id}` says it in
  `warnings`. What can't convert refuses with why, its files as they were.
- `POST /v1/projects/{id}/imports` `{source, name?, at?: {x, y, w?}, description?, exact?, moves?, placed?}`, with an `Idempotency-Key`: `{name, sha256, field, path, already?, kept?}`. One call, one transaction, makes an element: the file in `files/`, a box `{approved, description?}` under its name, with `at` its row on the canvas, and `moves`, the rows its drop pushed aside, each with its revision in `placed` as `PATCH /canvas` takes it (`kept` the ones another wrote since, as they are). `source` is `sha256:<hex>` (a held file, moved rather than copied, or one the project has; else `404 file_not_held`), the start of the hash of a file the project has, or an `http(s)` URL (64 MiB at most, public addresses only at every hop); a client's own file comes held, never by its path. `name` absent: the stem of a URL, lower-cased, else `file`; the browser sends the file's name. It goes through `as_name`, and a name the project has, by the rule names match, gets `_2`, so `name` in the answer is the one it got. With `exact`, a person typed it: `409 asset_exists`, naming what has it. The same file twice is a second element with no new bytes, `already` saying so. `field` is `image`, `video` or `audio`, from the bytes; anything else is `400 import_failed`.
- `GET /v1/projects?limit=&before=`: the index, last used first: `{projects: [{id, name, path, used_at_ms, busy?}], older}`. A project whose folder
  no longer holds `project.db` (deleted, moved, on a drive that's away) leaves the index and frees its name; opened again by its path
  it is back by its id, under its name, or `<name>_2` if another took it.
- `GET /v1/projects/{id}` (`?assets=false`: all but its assets; `unsent`: what kept its last render from reaching a machine, each error once, until another is asked): worked out once per revision, so another read costs only its answer. `{id, path, board?, assets, queue?: [{name, running, run?}], missing, stale: [{name, why}], orphans: [{name, kind, versions, bytes, renamed_to?}], busy,
  absent, cuts: [{cut, path, fps, cut_at_ms?, frames?, retimed?, skips?, without?}], warnings: [{name?, message}], next?}`. `next` is the commands to run given what it holds and lacks. Each asset is `{name, standing, making?, kind, noun, tool?, versions, prefixes?: [{number, prefix, pickup?, latents?, execution_ms?, run?, main?, from?, without?}],
  version?, approved?, pinned?, inputs?, files, path?, sha256?, field?, width?, height?, duration_ms?, alpha?, absent?, machine?, made_by?, status, reads?: [{name, version?, at, key?}], fields?, etag?, failure?, changed?, missing?}`: `standing` is `current`, `out_of_date`, `missing` or `making`, for a client that shows it, `making` the runs making it now, `version` the version that is its state, `approved` is the version a pin is, `pinned` that its `approved` is its state,
  `files` its state by output name, each `{path, sha256, absent?, machine?}`, `absent` that its bytes aren't on this computer and `machine` the machine that keeps them when one does; `path` its main file, `sha256` and `field` that file's hash and the field naming its type, `width`, `height` and `duration_ms` what is measured of it so far, `alpha` that part of it is fully transparent, `absent` and `machine` as for a file, and `status` is for people, in the asset's noun. A prefix's `pickup` is the take it picked up from, `{take, out_frame, beats_from?}`, and `latents` the machine keeping its latents, which a pickup of it needs. Its `main` is that version's main file, `{output, path, sha256, field, width?, height?, duration_ms?, fps?, alpha?, absent?, machine?}`, `output` being what `GET /v1/runs/{id}/play` takes of its `run`; `execution_ms` is how long its runs took. `from` and `without` are a cut's: what that version was cut from, and what it was cut without, each `<piece>: <why>`. `reads` is what its text mentions that names something, with `version` the one a `<name.N>` fixes, `at` the places that say it (`summary`, `beats.0.description`, `tracks.0.clips.1`), and `key` twelve hex digits of what that read sends now (its version's files and its box's description, or a text box's words), which moves only when what it reads does. `fields` is its inputs, flat, each by a control's `field`, as they apply now, its text as `instructions`, `tool` what makes it, `etag` what `PATCH`'s `If-Match` names: the digest of its name and fields. `failure` is what stopped its last try, the one error envelope: it stands until a version lands, it's tried again or its inputs change, and canceling is none. `changed` is a pin's: what changed since the version it holds, and it stands as `out_of_date` until `runs` with `outdated` (or naming it) re-runs it. An input gone from the project isn't a change: it is in `missing`, and `outdated` leaves that pin alone. `input_missing` is a `<name>` in its text, or a file link of an entity it reads, that names nothing and is no tag of its package: nothing was sent, so nothing was charged. `missing` is those names, once its package's tags are known; a pin lists them too, for its text, though it stands until it is run again. A warning's `name` is the asset or entity it's about, and it has none when it's about the file. A box renamed in an imported YAML keeps its versions, approval and canvas row (decision 6): a name with no versions takes the row of a name gone from it when exactly that row holds a version of the same content, by its name-blind key or the file it approves, and no other new name claims it. An orphan is an asset whose versions the project keeps and that it no longer names; its `bytes` count what `reclaim` would free of it, each file once; `renamed_to` the one new asset of
  its kind with none. A project's `absent` counts files the lock names that are not here and no machine keeps. `warnings` holds what reading its boxes said, what each
  state's version says, and each asset whose tool names a published package, as last heard of, with no one function that makes it. A read waits on no Hub and never writes: a package not heard of yet is asked of in the background, and a later read or a render says it.
- `GET /v1/projects/{id}/assets/{name}?call=`: `{asset, versions: [{number, files, made_by?, seed?, runs, approved?, warnings?, pickup?, execution_ms?, text?}], discarded?: [number], call?, call_warnings?}`. With `call=true`, `call` is what its binder would send now, the JSON `cozy run --input` takes, with what binding said (the browser reads it through this endpoint); what can't make one, a package the Hub doesn't have among them, is no `call` and why in `call_warnings`, never a refusal of the read. `text`: the words that version was made from. `discarded`: what's set aside until `reclaim`, which undo of its discard brings back.
- `POST /v1/projects/{id}/assets` `{tool?, at?: {x, y, w?}, name?, fields?, exact?, kind?, moves?, placed?}`, with an `Idempotency-Key`: `201 {name, etag, kept?}`. A tool dropped on a canvas: an asset, with `at` its row and `moves` what its drop pushed aside (as an import's), in one transaction. `tool` is an `id` from `GET /v1/tools`, and `name` and `kind` what its row says, so a page's drop waits on no read. With no `kind` that isn't held already, or no `name`, the tool is read, refusing what can't be one (`400 tool_names_no_function`, `400 tool_makes_no_media`), and a Hub that's away refuses nothing: its render says. With none it's a text asset, `kind` `text`, its words in `fields.text` (`400 asset_says_nothing`, and `400 asset_name_reserved` for a tag of a package the project uses). `exact` says a person typed `name`: `409 asset_exists` if the project has it, naming what does, not `_2`. `name` goes through `as_name`, a `.` staying a character like any other but for an ending of `.` and digits, which says a version and is `_` instead, `400 project_name` with `exact`; `self` is taken; absent, it's the tool's function. A taken name, or one whose versions the project keeps, gets `_2`, chosen in the transaction that writes it.
- An Edit's `fields.timeline` is `{tracks: [{clips: [{clip: "<name>", in?, out?} | {clip: "<name>", frames?}]}]}`: one track for now, a clip's place in the list its place on the track, with no gaps. `in` and `out` count a video's frames from 0 at the Edit's rate, `out` exclusive, as OTIO and an EDL do, and no `out` is its end; `frames` is how long a still is held, 3 s when absent. Its row and each of its `prefixes` also say `timeline: {fps, tracks: [{clips: [{clip, in?, out?, frames?, name, field?, path?, sha256?, holds?, width?, height?, missing?}]}]}`: the clips as they read now, or as they read when that version was rendered. `holds` is how many frames a video has at that rate, and `missing` why a render goes on without the clip (`no longer in the project`, `no version yet`, `not on this computer`). `rendering`, 0 to 1, is how far a render here is, with `standing: making`; each change says `project.changed`.
- `PATCH /v1/projects/{id}/assets/{name}` `{fields?: {<field>: value | null}, tool?}`, with `If-Match` if it should not write over another's edit: `{name, etag, was, warnings?}`. Its row moves by `PATCH …/canvas`, the page's drag and API callers alike. `field` is exactly a control's `field`, or any key of the asset's `fields`, `description` among them; a null clears it. `tool` changes what makes it, and `""` takes it off, its fields with it (Paul, 11-06; Undo writes them back): its versions, their numbers, `approved`, `description`, `models`, `lora` and its words stay. Only a box with no versions and no `approved` moves its words: `text` to `instructions` as a tool goes on, `instructions` or `summary` to `text` as it comes off. A tool going on a picture no version holds lists it as the next version, as if it had made it, and starts from what the box shows, approved else newest: `<self.N>` in its first required picture slot, else at the start of its first prompt. Its approval, or none, stays as it was: a tool approves nothing. Naming `instructions` or `text` in `fields` says what they are instead. Only the fields it names change. Setting what it holds changes nothing, and a render going keeps the text it was asked with. `If-Match` is the asset's `etag`, so an edit of another asset never conflicts: `409 asset_changed` when it moved, and the answer's `was` tells a window whether a write of its own is all that moved it.
- `DELETE /v1/projects/{id}/assets/{name}`: `{name, at?}`, it and its row gone, in one transaction, and what it made stands. Its versions wait as an orphan's, its name free, for undo, for `rename`, or for `reclaim`. Undo (`POST …/history/undo`) puts it back as it was, where it was, with its versions, but not a render it stopped: what a render still owes of it stops, queued with its row saying so, running canceled, and landing nothing. `404 asset_unknown`.
- `POST /v1/projects/{id}/assets/{name}/versions` `{source}`: the same answer as below, with `already` when a version holds that file, then that version. A file joins the asset as its next version, so versions of one thing live on one box. `source` is an import's, and a file it holds with no version is a version first. What's approved stays. `404 asset_unknown`, `404 file_not_held`, `400 import_failed`.
- `DELETE /v1/projects/{id}/assets/{name}/versions/{number}`: `{name, number, noun, approved?, stands?, stale?, read_by?}`, that version off the asset at once, local, its number never given again and its files on record until `reclaim`. `approved`: it was the approved one, and `approved:` went with it. `stands`: what the asset stands on now, else its newest; absent, it has none left and stays with no take. `stale`: what is out of date now and wasn't. `read_by`: what fixes this version in its text: it doesn't go out of date, and a render of it fails until the version is back, which undo of this does. A take that picked up from it (`pickup`) stands. `404 version_unknown` if it has no such version or it's discarded already; `409 version_running` while that number is being made, with `next` the `cancel`.
- `GET /v1/projects/{id}/yaml`: the project as YAML, format 4 (`PROJECT-YAML.md`), each section, then each box and its fields in order, its `ETag` the digest of it (`If-None-Match` is `304`). It holds what the project says; versions, takes, files and rows stay in `project.db`.
- `PUT /v1/projects/{id}/yaml`, with `If-Match` optional: the body is the project, each section, box and field that differs from it written in one transaction and one history entry, a box renamed in it keeping its versions (decision 6). With `If-Match`, only while the export is what that ETag read, else `409 project_changed`. `204`, the new `ETag`. A body that doesn't read as a project of format 4 is `400 project_yaml` (or `project_format`), with its line, and nothing changes. Comments aren't kept: the database holds values. It renders nothing, though a render going keeps the text it was asked with.
- `GET /v1/projects/{id}/history?limit=` (50, at most 200): `{entries, undo?, redo?}`, newest first, each `{id, at_ms, by: {via, client?}, words, names?, edits?, undone?: {at_ms, by}}`. Every edit of the project is one entry, a row of `project.db`'s `ops` written in the edit's own transaction: each route below that changes a box, a canvas row, a take or a name (imports, assets, versions, approve, unpin, rename, canvas, yaml). `by` is what the request's `Cozy-Client` says, `page <tab>` or `cli`, else `api`. A request's `Cozy-Edit: <id>[, <id>…]` names what the client calls it: requests that share one are one entry, and undo and redo take it as `edit`. A canvas `PATCH` whose every row is `placed` is the page arranging its own boxes, and none. `words` say it plainly (`Deleted kyogre_3`, `Merged green into red`, `Switched image_2 to generate_turbo`, `Set instructions on image_2`, `Discarded take 3 of fight`, `Moved image_2`). The last 200 are kept, across restarts. Renders, cancels, reclaim, push and pull are no entry: nothing is rendered, rented or ended by undoing.
- `POST /v1/projects/{id}/history/undo` `{entry?, edit?, to?}`: `{entries, already?, stopped?, undo?, redo?}`. That entry, by `id` or its `edit`, put back as it was; with `to`, each that stands back to it, newest first; with nothing, the newest that stands, whoever made it. Each entry is one transaction: each thing it changed goes back only while it's as the edit left it, else `409 history_conflict` naming the later edit, with `next` undoing that first, and nothing of it moves. A row is put back whatever moved it since. `already`: it was undone. `stopped`: what refused one of a `to`, after those before it. `404 history_unknown` for an entry it no longer keeps, `409 history_empty` with nothing to undo.
- `POST /v1/projects/{id}/history/redo` `{entry?, edit?, to?}`: the same answer, the other way: what undo took back, done again, only while it's as undo left it. With nothing, the entry undone last, unless an edit came after that undo; with `to`, each undone since, up to it.
- `POST /v1/projects/{id}/runs` `{names, n?, on, hub?, approve?, missing?, outdated?, pickup?: {take, out_frame, beats_from?}}` (`Idempotency-Key`, committed with the ask, so a retry adds nothing even across a restart): answers `{id, ask?, path, joined?, added, skipped?, queued?: [{name, ahead?}], said?, next, board}`. `pickup`, for one name only: Pick up from here, its new take going on from `take` (`<shot_1.3>`, its own or another box's) at `out_frame`, recorded on the take and never written to its box. Each name is `n` new versions, and `all` one of every asset with an operation, whatever `n` says, with a warning, in reading order, unless `missing` asks only for what has no state. `skipped`: approved assets it was asked of by `all`, left as they are; naming one makes a version to look at, and its pin stays. `joined`, a render was going already; `added` 0 with it is no new work; `queued`, what it added, and the names ahead of each; `said`, what it found of what it names, each `{level, message, names?}` (a flag it ignores, a box it leaves); `board`, the render as it now stands, each change from there told on `GET /v1/events`. Its walk ends when the board's `ended.asks` names `ask`. It captures the project as it is, its export kept in `project.db` by digest until it is done, and plans on that text alone. Asks go at once. each name, or `all`, and what it reads: run A render is walked here and drives `on`, whatever its tools: a machine runs the calls it's sent, with the files each is given. Of several machines in `on`, each job goes to whichever quotes the soonest finish ("Placement"). The browser's automatic placement sends every rental of yours up or on its way, another computer's that's up, and `local` last (`MachineList::render_set`).
  its operation if it has no state, and nothing else. `new`, or `n` over 1, also draws `n` new seeds for those named; a pinned one's state doesn't move.
  `--take`, `--in`: one asset's new take keeps the start of that take, or the one in use (`400 take_unknown`), for a function that fills `continue_from` from `take`: it gets that take's `video`, `latents` and `at_s`, and one its package didn't make is `409 take_not_continued`. `202 {id, path}`, or `200` when
  its key asked it already. The work goes on in the project's walk and is followed on its board.
- `POST /v1/projects/{id}/fit` `{names, outdated?}`: `{tools: [{tool, label, names, line, bytes, here?}], rent?: {sku, gpus, card, vram_gb, usd_micros_per_hour, line}}`. What a render of `names` (or `all`, as `runs` reads it) on this computer alone would do poorly, a tool a row, said before it's sent. A tool is done poorly with no GPU when its function loads a model on one, as it would then run on the CPU, degraded, and on a GPU its weights fill more than three quarters of, the Hub's rule for a card that holds them. Its `bytes` are what the Hub places a run of its release by (`POST /v1/rental-placements`), local code's those of its models' releases, either with a checkpoint on a box in place of its model's; 0 when the Hub doesn't say in 5 s. `line` says how, and how long its last run here took; `rent` is what the Hub places them on: the biggest card of its picks for those not yet said for, else for all. `here`: Render here was said of it on this computer. A client asks this only of a render whose machines are `local` alone: with another, the walk gives `local` nothing it would do poorly. Nothing is listed while `local` hasn't said what it has, for what loads no model or runs on no GPU, or for what Calcifer makes itself. What the Hub says is kept 10 minutes.
- `POST /v1/projects/{id}/fit/here` `{names, outdated?}`: the same, each tool listed kept as rendered here in `state.db`, by what this computer's GPUs are (their names and usable memory, or none) and its weights: `here` then holds until another computer, or weights half as big again, such as a bigger checkpoint on its box.
- `POST /v1/projects/{id}/approve` `{name, version}`: `{name, noun, number, files, path, stale, warnings?}`. `version` is a short run of digits, its version, or anything else a hash prefix, which pins any file of the project; `number` is 0 when it is no version's. Writes the version's main file's hash as that asset's `approved`.
  Makes any tool whose reads settled, with no machine.
- `POST /v1/projects/{id}/unpin` `{name}`: `{name, version?, stale, warnings?}`. Takes its `approved` away. A constant, nothing but its file, keeps it (`400 project_edit`).
- `POST /v1/projects/{id}/rename` `{from, to}`: `{from, to, versions}`, in one transaction. With `from` a box and `to` free, its name and every `<from>` and `<from.N>` change. What the project keeps of `from` is kept as `to`, a box it names now with no versions (`409 rename_taken`), and its canvas row moves. `400 project_name` when `to` can't be a name: it holds what a name can't, ends in `.` and digits, or is `self`. It happens at once, whatever is going, and changes nothing else: a render going says `to` from then on, what lands is `to`'s, and what reads it reads on. Nothing on disk moves. A name is no input: what read it stands as it did, each key written anew.
- `POST /v1/projects/{id}/reclaim` `{yes?}`: `{orphans, files, bytes, unrecorded?, deleted?, next?, lost?, unheld?}`. `unrecorded`: how many of the files nothing has a record of, which `yes` leaves. `unheld`: what runs left in `.runs/` that `files/` doesn't hold by its sha256, each `{path, sha256?, bytes}` (a folder has no `sha256`), which `yes` leaves: an output `files/` couldn't take stands there, where its run's record names it. `yes` clears discarded versions, and keeps what another version names, a clip of an Edit's version cut, or a `<name.N>` in any text reads, with its record, a deleted asset's too. `lost` says why `yes` would be `409 project_history_lost`: the project lists no version or run while `files/` holds what nothing names. Lists orphans and the files nothing else names; with `yes`, deletes them, an orphaned cut's film at the root too, and canvas rows nothing names (`409 project_busy` while it renders). Never automatic.
- `POST /v1/projects/{id}/cancel` `{names}` (or `["all"]`): `{dropped?, canceled?}`. Drops their queued work and cancels their runs; nothing dropped is sent again.
- A render's `board` (in `GET /v1/projects/{id}`, and whole in `board.updated`, each job in `job.updated`): all a view needs, with no run to follow. The board a render ended on is kept in `project.db`, and is the project's `board` until another begins, across a close, a restart and a move; a copy of the folder, or another Calcifer, shows none of it. A walk's end is told before the next walk's board begins. `{seq, on, began_at_ms, elapsed_ms?, shots, jobs: [{id, asset, state, noun, number?, shot?, why?, gpus?, starts_at_ms?, ends_at_ms?, run?, machine?, started_at_ms?, took_ms?, stage?, detail?, fraction?, stalled?, preview?: {label, path}, path?, seconds?, error?}], ahead?, said?: [{level, message, names?}], errors?, ended?: {error?, rendered?: {takes, versions, elapsed_ms, execution_ms}, asks?}}`. `gpus`, `starts_at_ms` and `ends_at_ms` are its machine's quote ("Placement") for a job whose turn is next or that was sent, and once its run runs and says its time left, `ends_at_ms` is that (`eta_s`), the time the page shows. `seq` only rises: keep the highest heard. `jobs` go what has ended, then what is going, then what waits. `state`: `waiting`, `running`, `done`, `failed`, `canceled`, `not_sent`. `why` is a sentence: `after a, b`, `comes with c`, `waiting for a GPU, held by d`, or what came of it. `shot` is which of the render's `shots` it makes, 1 first, what's queued counted: a `kind: video` its package makes, not a cut or a still, the same number for the whole render, and `shots` grows only as another render joins. `stage` and `detail` say where it is: `segment 2/2, denoise`, `14/30`. `preview` is what the run has shown so far while it goes on, never its result. `said` is each line the render said, once, in order, `level` `info` or `warning`; a warning's `names` are the assets whose runs gave it. `errors` are what failed of no one asset (a job's failure is its `error`). `ended`: its walk is over, nothing it was asked is left to send or follow, and every take it made is whole here; `error` is the first failure, or when more than that was asked, `render_incomplete`: `rendered 6 of 15; 9 failed: <the first>`, with the command for the rest in `next`; `asks` are the renders it ended (`runs`' `ask`). A second run failing the same way on a machine stops what that render sends there, and cancels what it sent that has not begun.
- `GET /v1/projects/{id}/files/{sha256}.{ext}`: a file (Range, ETag its sha256). `GET /v1/projects/{id}/cuts/{cut}`: a cut or its `.otio`, as last made.
- `GET /v1/projects/{id}/canvas`: `{seq, cards: {name: {x, y, w?}}}`, the rows and the revision they're of. A name with no row isn't listed, and the page places it. `PATCH` `{cards: {name: {x, y, w?} | null}, placed?: {name: seq}}`: `{seq, kept?}`, these rows set and a null one gone, last move wins, to a hundredth, in one transaction, and `canvas` on `GET /v1/events`. A row in `placed` is the client's own working, a new box's place or a box pushed off another, with the revision of what it was worked out from (a read's `seq`, or the newest `canvas` it had heard): it is set only if no other client wrote that row after it, checked in the transaction (cl-800); one that was stands, and is answered in `kept` with its row now. A person's move goes unplaced. Nothing in it is an input. Each row records the revision and client (`Cozy-Client`) that wrote it.
- `GET|HEAD /v1/projects/{id}/thumbs/{sha256}?w=&strip=&preview=`: a thumbnail, with `Cache-Control: immutable`, `sha256` with or without its `sha256:`. A picture keeps its alpha (PNG; JPEG with none); a video gives a poster, or with `strip=<n>` (1 to 32) one JPEG of `n` frames side by side; a sound gives a waveform (PNG). `w` (default 480, 16 to 2048) is its width, or each frame's, and it is never wider than the file. A video's poster is the first of its frames at 20, 35, 50, 65 and 80% whose luma isn't flat (a spread under 12 of 255, so not black), else the busiest of them. `preview=true` gives a video's preview, whatever `w` and `strip`: 3 s from its poster's frame on, or to its end, at most 480 px on its longer side, silent, as H.264 MP4 with Range, for a page to loop on a hover. It is made once per file, as the video lands or is imported, else on its first ask. `404 file_missing` when the project has no such file here, `400 thumb_failed` when ffmpeg can't read it, which is kept for a day. Made by Calcifer's own ffmpeg, a few at a time, and kept under `~/.cozy/cache/thumbs` until unused for 30 days.

### Tools and Calcifer
Calcifer fetches the tools it runs on first need, pinned builds held to their sha256, into `~/.cozy/tools`:
ffmpeg (cuts, frames, clips) and uv (local code). No route asks for them.

- `POST /v1/down`: `{stopping: true, owed}`. Admission closes, mutations in flight finish, every stream ends,
  then the server. Machines keep what they accepted; the next Calcifer resumes `owed`. Never refused.

## CLI verbs and their calls

The CLI resolves the current directory to an existing project and sends its absolute path as the routes' `{id}`. `cozy new [directory]` creates a project explicitly through `POST /v1/projects`. `cozy open`, `cozy run` and `cozy status` require an existing project. API clients can also use a project name or id. The browser provides asset import, editing, rendering, export and history through the project endpoints described above.

| Verb | Calls |
|---|---|
| `cozy new [directory]` | `POST /v1/projects` with the selected absolute path |
| `cozy open [--port PORT]` | current directory, `GET /v1/projects/{id}?assets=false` for its id; then `GET /`, `POST /v1/session-codes` |
| `cozy status` | project health, warnings and suggested actions from `GET /v1/projects/{id}` |
| any verb, Calcifer of another version | `POST /v1/down`, then the installed cozy's Calcifer started |
| `cozy run <target> [inputs] [flags]` | `HEAD`/`PUT /v1/files/sha256:<hex>` for each file and folder it names, and local code's tree, `POST /v1/run-quotes` (`--on new`), `POST /v1/runs` (again after a `preparing` install), then `GET /v1/runs/{id}/events`; `-o DIR`, each output `GET /v1/runs/{id}/outputs/{name}` |
| `cozy run <target> --describe`, `cozy run <package>` | local code's tree `PUT`, `GET /v1/targets` (`tree`, `at`) |
| `cozy run <target> [inputs] --estimate [N]` | local code's tree `PUT`, `POST /v1/run-estimates?count=N` (again after a `preparing` install) |
| `cozy run ls` | `GET /v1/runs`; live: `GET /v1/events`, then `GET /v1/runs` |
| `cozy run show <run> [-o DIR]` | `GET /v1/runs/{id}?detail=full`, `GET /v1/runs/{id}/events`; `-o DIR`, each output `GET /v1/runs/{id}/outputs/{name}` (a folder's tree, then `?member=`) |
| `cozy run cancel <run>` | `GET /v1/runs/{id}`, `POST /v1/runs/{id}/cancel` |
| `cozy run play <run>` | `GET /v1/runs/{id}/play` |
| `cozy login <email>` | `GET /v1/account`; `POST /v1/account/codes` (`sign_in`), `POST /v1/account/codes/{id}`; `PUT /v1/account/name` while unnamed |
| `cozy login --export FILE` | `POST /v1/account/codes` (`export`), `POST /v1/account/codes/{id}`; the CLI writes the file |
| `cozy login --key FILE` | `PUT /v1/account/key`; `PUT /v1/account/name` while unnamed |
| `cozy logout [--everywhere]` | `DELETE /v1/account`; with `--everywhere`, `POST /v1/account/codes` (`sign_out_everywhere`), `POST /v1/account/codes/{id}` |
| `cozy whoami` | `GET /v1/account` |
| `cozy org claim` | `POST /v1/orgs/{org}/claim` |
| `cozy org edit` | `PATCH /v1/orgs/{org}` |
| `cozy model ls [query]` | `GET /v1/models` |
| `cozy model ls --on` | `GET /v1/machines/{name}/models`; `--on all`: `GET /v1/machines` |
| `cozy model info` | `GET /v1/models/{org}/{name}` |
| `cozy model edit` | `PATCH /v1/models/{org}/{name}` |
| `cozy model publish` | `POST /v1/models/{org}/{name}/releases` |
| `cozy model delete` | `DELETE /v1/models/{org}/{name}[/releases/{v}\|/commits/{c}]` |
| `cozy model download` | a file `HEAD`/`PUT /v1/files/sha256:<hex>`, `POST /v1/runs` of `cozy/model_download` (`POST /v1/run-quotes` first for `new`); `GET /v1/runs/{id}/events` |
| `cozy model upload` | a file or folder `HEAD`/`PUT /v1/files/sha256:<hex>`, `POST /v1/runs` of `cozy/model_upload` (`POST /v1/run-quotes` first for `new`); `GET /v1/runs/{id}/events` |
| `cozy model quantize` | `--package ./dir`, its tree `PUT`; `POST /v1/runs` of `cozy/model_quantize` (`POST /v1/run-quotes` first for `new`), again while `preparing`; `GET /v1/runs/{id}/events` |
| `cozy package ls [query]` | `GET /v1/packages` |
| `cozy package ls --on` | `GET /v1/machines/{name}`, `GET /v1/machines/{name}/packages?fresh=true`; `--on all`: `GET /v1/machines` |
| `cozy package info` | `GET /v1/packages/{org}/{name}` |
| `cozy package install` | local code's tree `PUT`, `POST /v1/runs` of `cozy/package_install` (`POST /v1/run-quotes` first for `new`); `GET /v1/runs/{id}/events` |
| `cozy package remove` | `POST /v1/runs` of `cozy/package_remove`; `GET /v1/runs/{id}/events` |
| `cozy package yank [--undo]` | `POST` (`DELETE`) `/v1/packages/{org}/{name}/releases/{release}/yank` |
| `cozy package delete` | `DELETE /v1/packages/{org}/{name}` |
| `cozy package lock` | the package's tree `PUT`, `POST /v1/runs` of `cozy/package_lock`, `GET /v1/runs/{id}/events`; the CLI writes the `uv.lock` its result holds |
| `cozy package publish` | the package's tree `PUT`, `POST /v1/runs` of `cozy/package_publish`, `GET /v1/runs/{id}/events`; a `lock` its result holds, the CLI writes |
<!-- each PR adds its verbs' rows -->
| `machine ls` | `GET /v1/machines?refresh=true`; on a terminal, `GET /v1/events`, then `GET /v1/machines` |
| `machine ls --history [--since D]` | `GET /v1/rentals?since_ms=` |
| `machine show <name>` | `GET /v1/machines/{name}`; live on a terminal, as `ls` |
| `machine logs <name> [log] [--since D] [-f]` | `GET /v1/machines/{name}/logs/{log}?lines=&since_ms=` (SSE with `-f`). `logs local calcifer` reads Calcifer's log file, so it works while Calcifer is down |
| `machine rent` | `GET /v1/rental-skus` |
| `machine rent <sku>` | `POST /v1/rental-quotes`, `POST /v1/machines` (rent), then `GET /v1/events` until ready or ended |
| `machine end <name> [--force]` | `DELETE /v1/machines/{name}`, then `GET /v1/events` until ended |
| `machine keepalive <name>` | `POST /v1/machines/{name}/keepalive` |
| `machine update <name> [--runtime\|--tensorfs\|--tensord V\|PATH]` | a file `HEAD`/`PUT /v1/files/sha256:<hex>`, `POST /v1/machines/{name}/update`, then the run's events. A project directory is built here first |
| `machine add <name> <address>` | `POST /v1/machines` (add) |
| `machine authorize <key> [--remove]` | `PUT` or `DELETE /v1/machines/local/authorized-keys/{key}` |
| `machine ssh <name>` | `GET /v1/machines/{name}` for its address, then `ssh` here |
| `update` (moving `local` to the new cozy) | `POST /v1/machines/local/update` with `{"tensord": {"version": V}}` |
