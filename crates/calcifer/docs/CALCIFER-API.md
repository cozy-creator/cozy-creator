# Calcifer API

Calcifer is the daemon on every computer that runs cozy. Its HTTP surface is the product: the `cozy` CLI and a
web UI are equal clients of it, and the CLI only parses arguments and prints (cl-716). Every behaviour a verb
offers is one or more of the calls below. Written by hand and changed in the same PR as the routes
(`crates/calcifer/src/api.rs`, `machines.rs`, `project/mod.rs`); the records are `crates/protocol`.

## Clients and discovery

Calcifer listens on `127.0.0.1` only: on `calcifer.port` from `~/.cozy/config.toml` and no other if it's set, else
on 7317 or the next free port, so a page's origin outlives a restart. It holds `~/.cozy/calcifer/calcifer.lock` and publishes a lock record beside it at `calcifer.lock.holder` (owner-only): `{address, token,
pid, version, started_at_ms}`. A client reads the record for the address and the bearer. The CLI starts Calcifer when none
answers, and restarts it when it runs another cozy than the installed one.

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
the first time, with `Idempotent-Replayed: true`, across a restart too (kept a day in `state.db`), and one still going
is waited for. The same key for another ask is `409 idempotency_conflict`. The CLI makes one key per call and the
page one per outbox action and ask, so a resent undo steps once and a resent discard is no `404`.

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
`GET /v1/events`: what a client keeps on screen, on one stream, each thing whole. There is no backlog: subscribe, then read what you hold. It ends only when Calcifer stops, and a type a client doesn't know is for it to skip.

- `hello {version}`, first.
- `run.updated {run}`: a run's change, as `GET /v1/runs/{id}` answers it.
- `machine.updated {machine}`: a machine as `GET /v1/machines` lists it, each time it changes. `machine.removed {name}`: it left the list.
- `project.changed {id, rev, assets?, gone?, head?}`: `assets` names those whose row is another now, each to read by `GET /v1/projects/{id}/assets/{name}`, and `gone` those no longer there; `head` says something else `GET /v1/projects/{id}?assets=false` holds changed. No `assets`: read the whole project, as a client does at the start and after `resync`. What it costs a client is what changed, whatever the project holds. `rev` is opaque, and changes whenever `project.yaml` or `project.lock` does, as a landing's does, when a file comes to or goes from `files/`, as a pull's do, and when a push or pull ends. While a stream is open, the projects open here are looked at four times a second for edits from outside, `project.canvas.yaml` too, whose rows that changed go as `canvas`.
- `canvas {id, etag, cards: {name: {x, y, w?} | null}}`: the rows of a project's canvas that changed. A row written with its asset goes first, so a client that reads the name has its place.
- `job.updated {project, job, seq}`: one job of a project's render, whole, each time it changes; `id` names it for as long as the render goes, and `GET /v1/projects/{id}`'s `board` has them all. A client reads the board, then keeps what's newer than its `seq`.
- `ahead.updated {project, ahead}`: what a machine is fetching ahead of a project's queued jobs, as it stands now: `{function, machine, run, stage?, done, total, ready?, held_back?}`. The board's `ahead` has them all.
- `history {id, undo?, redo?}`: a project's history moved, an edit, an undo or a redo, and the entries undo and redo would take now, as `GET /v1/projects/{id}/history` lists them.
- `resync`: this stream fell behind and goes on: read again what you hold.

### The page
The web UI, embedded when cozy is built (`crates/calcifer/build.rs`, from `web/app/dist`). It holds no secret, so these
two routes need no credential; the guard's first three checks still apply.
- `GET /`: its `index.html`.
- `GET /assets/{file}`: a file named by its hash, `immutable`; `404 asset_not_found` for any other.

Each answers `Content-Security-Policy: default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' blob: data:;
media-src 'self' blob:; font-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'` and
`Referrer-Policy: no-referrer`. A build without the page answers `503 page_not_built`, with how to build it.

### Status
- `GET /v1/status`: `{version, os, local_usable, signed_in, rental?: {machine_id, idle_deadline_ms?}}`. `rental` is there on a rental: `machine_id` is its Hub's name for this pod, which has no rental id of its own, and `idle_deadline_ms` when it releases itself unless a job comes, absent while one is queued or running. `os` is `linux`, `macos` or `windows`; `local_usable` is
  whether this computer runs models itself; `signed_in` is whether it holds a device key for its Hub, which it answers
  without asking the Hub (`GET /v1/account` does).

### Session
A browser signs in with a single-use code. `cozy open` writes it into a 0600 redirect file
(`~/.cozy/calcifer/open.html`) and opens that, so it's in no process's arguments. With no browser to open, over SSH or with no display, or with
`--no-open`, it prints the link instead. The
fragment may add `&to=<route>`, as `cozy open`'s does in a project (`#code=<code>&to=/projects/<id>`). `~/.cozy/calcifer/sessions.json` (0600) holds sessions' digests; 30 days unused ends one.
- `POST /v1/session-codes` (the lock record's bearer only, else `403 session_code_needs_bearer`):
  `{code, url, expires_ms}`, 32 random bytes in hex, good for 60 s. `url` is `http://<Host>/#code=<code>`.
- `POST /v1/session` `{code}`, needing no credential: `{token, media}`, new for each code. `400 session_code_invalid`
  when it's unknown, used or expired.
- `DELETE /v1/session`: `204`, the session whose token this carries has ended.

### Runs
A run is asked for unbound (`RunRequest`); Calcifer resolves its target, binds it against the function's
interface, cuts and stages its files and builds the machine's spec (`crates/calcifer/src/runs`). No client builds
a spec.

- `POST /v1/runs` (`Idempotency-Key`): a `RunRequest`. A run needs a `project`, its id or a folder in or holding one: without it `400 not_in_a_project`, an unknown id `404 project_unknown`, and its outputs land in that project's `files/` and `project.lock`'s `runs`; a file it's given joins `files/` at submit, so it can be asked again once what held it is gone. Answers `202` (`200` when the key had made it) with
  `Submitted {run, replay, notes, preparing}`:
  - `target`: `[org/]pkg/fn[@release]` (no `@`: the newest release its machine holds that isn't yanked, else the
    Hub's newest, which it installs; a yanked one moves to the newest with a warning; `x/y` is root package x's function y, else account x's package y), or local code `./dir[/fn]`,
    `./script.py[/fn]`.
  - `on` (`local`, a machine, `any`, `new[:sku]`; absent: where a render's job would go, the set `cozy project render` sends with no `--on`, on whichever ends it soonest by what ran there, else by the same card, `local` losing a tie and taking nothing it would do poorly unless what ran says it ends it first), `hub`, `gpus`, `lane`, `attention_kernel`, `project`, `warm`
    (a level, instead of running), `agreed` (a `new` run's quote).
  - The payload: `input` (an object), `input_file` (a JSON or YAML file here; files it names are relative to
    it) or `input_text` (the same read from text; files it names are relative to `cwd`); then `terms` override it (`key=value` typed as declared, `key:=<json>`, `model.<slot>=<ref>`, one
    positional value, input files), `loras` (`[slot=]ref[,strength]`), `files` (`{name?, file}`: an input's
    field path, a label, or none for the declared files) and `fidelities`. A file is a path here (`@` before it is allowed), a directory at a tree input, a held
    `sha256:<hex>` (`404 file_not_held` when neither Calcifer nor the project holds it), or six or more hex digits that start the hash of a file of the
    project. Relative paths start at `cwd`; without one they are refused. In a project, a value that mentions a box (`<coast.2>`, `<take3:48-336>`)
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
  (`RunQuote`). Nothing is rented or recorded.
- `GET /v1/tools?q=&on=&hub=&project=`: `{tools: [{id, package, function, release, name, label, package_title, makes, description?, local?, fields, ui?, pickup?, model?: {slot, default?, family?}}], note?}`: what can be dropped on a canvas: first the Edit, `cozy/edit`, which Calcifer renders itself, free, with no machine, its one field a `timeline`; then your list, by a card read of the Hub or kept here, so listing installs nothing and it stands with no Hub; then local code and what `on` has installed; and only with `q`, the Hub's, and what you took off your list. Each is every public function of it declared `@app.tool` (its interface's `tool`), never a `quantize`. With `project`, first the local code its assets name, `local` and labelled so, and no other local code is listed. Then what the machine `on` (default `local`) has installed, each package at its newest there, matching `q`'s words in its package's name or its label; then the Hub's search for `q`, or its first page, at each package's newest. `note`: the Hub didn't answer, so only what's installed. `id` is what a box names as its `tool`. `name` is what a new box of it is called, its tool's `named` or its package's word, numbered: `shot_1`, and a taken name counts up. `label` is what it does within its package, its tool's, else its function in words. `package_title` is its package in words, which heads its tools in the page's list: `qwen-image-2` is `Qwen Image 2`, a word with no vowel upper case (`sdxl` SDXL). `makes` is `image`, `video`, `audio` or `text`: its result's first picture, video or sound, else a text. `ui` is the path in its package of a component that draws its box, where it ships one (`PACKAGE-UI.md`). `pickup` `{every, offset, first}`: it takes a pickup, which Pick up from here fills, landing on `offset + k * every` from `first` (`Snap`), or any frame `{1, 0, 0}`. `fields` are its struct's fields in order, each `{field, type, label, required?, default?, auto?, placeholder?, advanced?}` plus by `type`: `words` (a `str`); `prompt` (text with mentions; `takes?: ["image"]` when it takes only those); `toggle` (`auto` for `| None`); `number` (`min, max, step?, unit?, integer?, total?`, `auto` for `| None`; an unbounded number is no field); `choice` (`options`, the values in order, `words?` and `descriptions?` maps by value from `Label`); `shape` (the box's own shape, `ratios?` or `min?, max?`); `slot` (`accept: image|video|audio`, `described?` for a file with its description, `multiple?`, `spans?`, the list whose timeline a video's length is); `group` (`members`, nested as deep as the struct, `list` for rows with `min_items?, max_items?`); `seed`; and the Edit's `timeline`. `advanced` is `Advanced()`: under More, folded. A field's `label` is `Label`'s, else its name in words. A pickup is no field, and neither is `tool`, `approved`, `description`, `models`, `lora`, `text` or a type this cozy can't draw. A box's `fields` hold what's written: a slot's value is a mention, `<coast.2>`, a list when `multiple`.
- `POST /v1/tools/list` `{add?: "<package>", remove?: ["<tool id>"]}`: `{packages, without?}`, your list, in `~/.cozy/calcifer/tools.json`, the same in every project: Qwen Image 2 and Minimax H3 until it's changed. A package named by its absolute path is local code: its tools are listed as local in every project. `add` puts a package's media functions on it, all of them again if some were taken off. Neither installs or deletes anything, and a box of a tool taken off goes on as it was.
- `GET /v1/tools/browse?q=&limit=&hub=` (1–200, 50): `{packages: [{package, summary?, tools}], older}`: the Hub's packages that make media, for adding to your list. `older` is how many more a bigger `limit` reads.
- `GET /v1/tools/{id}/ui?project=`: `{module}`: where to `import()` that tool's component from, its bytes in hand. `{id}` is one percent-encoded segment. A release's come from the Hub, for the newest any machine you hold has installed, its wheel held to the Hub's digest; local code's from the files `project` names, as they are now. `404 tool_ui_none`: it ships none. `404 tool_ui_not_installed`: no machine you hold has it, or `project` names no such local code. `404 tool_ui_missing`: its package doesn't hold what it names. `503 tool_ui_hash_mismatch`: the wheel isn't what the Hub says. `400 tool_ui_too_big`.
- `GET /v1/tools/{id}/ui/{hash}/{*file}`: a module of a component as `{module}` named it, `application/javascript`, under the page's CSP, `immutable`, needing no session: `import()` can't send one, and the hash is what `GET /v1/tools/{id}/ui` said. Calcifer holds what it read in memory, so after a restart a page asks again. `404 tool_ui_file_not_found`.
- `GET /v1/targets?target=&cwd=&hub=&on=`: what a run of `target` on that machine calls, resolved as `POST /v1/runs` resolves it:
  `{package, function?, release, release_from: asked|installed|latest|local, summary, interface, functions, contract?, request?, next?}`:
  `interface` is the package's interface document. Local code's is what its machine's Runtime said installing this very source, else read from its source by the Runtime release its uv.lock holds or PyPI's newest, installing nothing: `400 describe_failed` when that Runtime can't read it, with `next` the run that reads it with the machine's own; `functions` `[{function, description}]` is what a
  person runs of it, with no internal function or recipe. A named function is checked as a run checks it, and `contract` is its contract as people
  read it, `request` its schema. `next` names the functions to describe.
- `PUT /v1/files/sha256:<hex>`: a file's bytes, which must hash to its name: `201`, or `200` when held, with
  `{digest, length, media_type}`. With `Content-Range: bytes a-b/len`, one part: `204` with `Cozy-Held: a-b,c-d` until it is whole and
  verified, then as above. Parts land in any order, side by side, and what a body carried before it broke is held. A whole `PUT` carries
  its `Content-Length` (`400 file_length_required`); `400 file_digest_mismatch` drops the bytes; `409 file_length_conflict` when it is
  being written at another length. `HEAD`: `200` held whole, else `404`, with `Cozy-Held` either way. A held file or part unused for a day is dropped.
- `GET /v1/runs` (`status`, `package`, `machine`, `project`, `before`, `limit`): `{runs, older, machine?, unreadable?}`. `project` is an id (`404 project_unknown`), or a folder:
  the runs of the project holding it; in none, all of them. Each run names its `project` and `project_id`.
- `GET /v1/runs/{id}`: the run. While it waits, `queue_position` (1 is next); while it runs, `progress` with
  `done_fraction`, `bytes_per_s`, `step_steady_ms`, and its time left: `eta_s`, `eta_confidence`, `eta_basis` ("Time left"). Each output, once it landed, has `url`, the API path that serves it, and `width`, `height`, `duration_ms` as measured when it landed, where they apply and ffprobe is here. `?detail=full` adds `measurements`, `calls`, `report` (each
  executor's stages and unaccounted time, models, kernels, and each GPU's memory, with rates against its
  ceiling) and `request {target, input, seed?}`: what it was asked, read-only, each file by its `sha256:` and the models and LoRAs it chose as `models`. To ask it again, send `target` and `input` with a new key.
  A run settled over 30 days ago has dropped `measurements` and each call's, so its `report` is gone. Nothing else
  of a run is ever dropped: its row, `result`, reason, outputs, logs and `calls` stay for good.
- `GET /v1/runs/{id}/events` (`Last-Event-ID` or `?starting_after=`): `run.*`, `output_item.*`, `run.log`,
  `run.call`, and the live `run.progress` and `run.waiting`. It ends after the terminal event once every output
  has landed here or failed to.
- `POST /v1/runs/{id}/cancel`: `202`, or `200` when it had ended.
- `GET /v1/runs/{id}/play?output=&expires=` (`expires`: `90s`, `30m`, `24h`, `7d`, default `1h`): `Play {address, fingerprint, token, url, expires_s}`: how a browser plays that output straight from its machine, and `url`, the hosted player's link. `400 play_expires_invalid`, `400 play_output_invalid`.
- `GET /v1/runs/{id}/outputs/{name}`: an output's bytes. An item with `stays`, an output its function's result declares `Latents`, stays on its machine: no `path`, nothing waits for it, and this answers `409 output_on_its_machine`.

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
- `GET /v1/estimate?target=&on=&megapixels=&steps=`: `Estimate {eta_s?, eta_confidence?, eta_basis, runs}`: the prior's whole time for
  a run of `target` (as a run's record names it) on `on` (default `local`), before it's sent; `runs` it stands on. `first_run` with no
  `eta_s` when none ran like it. `404 machine_not_found`.

### A run that stopped moving

A running run with no progress for 10 minutes carries `stalled {since_ms, machine, usd_micros_per_hour?}` until it
moves, and its record gets one `no_progress` warning naming the machine and its price. Nothing ends it: only its owner does.

### Machines and rentals
<!-- cl-716 C -->
A machine is this computer's (`local`, started when used), a rental, or another computer's added by address.
`{name}` is a live machine's name or former name, or any machine's rental id. Records: `protocol::calcifer`.

- `GET /v1/machines[?refresh=true]`: `MachineList {machines, hubs, unanswered, signed_out, rentals}`. It lists
  every live machine, every live rental of this account among them: one another client ordered is reached with this
  computer's device key, reads `ordered: false` and takes no run that doesn't name it.
  `refresh` lists every Hub again first. `rentals` sums the live rentals: count, `usd_micros_per_hour`, and
  `accrued_usd_micros` on its weakest `accrued_basis` (`accrued_floor` when some Hub said nothing).
  A machine's `status.activity` is what it does for this computer's runs besides their own work, as its Status says,
  and moves on `machine.updated`: each `Operation {id, kind: download|install, subject, stage?, bytes_done, bytes_total,
  bytes_here, bytes_per_s?, eta_s?, started_at_ms, runs}` once, however many runs wait on it. `subject` is the model's
  repository (or provider source), or the package; a download's bytes are what it fetches, and
  `bytes_here` what of it was here already; `runs` are the run numbers waiting on it. An older machine says none.
- `GET /v1/machines/{name}[?fresh=true]`: `Machine`. A rental rented with an SSH key has `ssh {args, line}`: the command that logs in, with a known-hosts file of its own, as an argument list and as a line to paste. `fresh` reads its Status now, or refuses
  `machine_unavailable`.
- `POST /v1/machines` (`NewMachine`):
  - `{"kind": "rent", sku, hub?, gpus?, disk_gb?, ssh_key?, agreed}`, with an `Idempotency-Key`. It answers
    `202 Rented {machine, quote, replay}`, or `200` on a replay. Follow the boot on the events until the
    machine is `ready`, or `ended` without one. `agreed` is the quote's `agreed`, sent back as it came: a dearer order rents
    nothing (`rental_price_rose`).
    - A Hub that does not answer is asked again under the same key, across restarts, so one order buys one
      pod at most. Until it answers, the order is a `booting` machine in the list, under the name it was asked
      with and with no `rental_id`. Another rent of the same thing joins it (`503 rental_order_pending`).
  - `{"kind": "add", name, address, fingerprint}` answers `Machine`.
- `ANY /v1/machines/{name}/calcifer/{*path}`: that machine's own Calcifer through its TensorD (`Link::forward`,
  machine scope), with both bodies streaming, so another Calcifer's API is this one's under a base path. `local` is
  refused `machine_is_here`, and a path with an empty, `.` or `..` segment `calcifer_path_refused`. The machine
  refuses `/v1/down` and writes to `/v1/account*` the same way, and answers `503 calcifer_down` while its Calcifer
  isn't answering. A key it doesn't admit answers `401 capability_invalid` there, passed on as it is, like any answer
  but this route's own `machine_*` refusals. `?media=` opens this route's reads of files too, and goes no further. **Its
  answers are hostile data on this computer's origin:** a request carries only `Accept`, `Content-Type`,
  `Content-Length`, `Content-Range`, `Range`, `If-Match`, `If-None-Match`, `If-Range`, `Last-Event-ID`,
  `Idempotency-Key` and `Cozy-*`, and never `?media=`; an answer brings back only `Content-Type` (its one
  `type/subtype`, else `application/octet-stream`), `Content-Length`, `Content-Range`, `Accept-Ranges`, `ETag` and
  `Cozy-*`, always with `X-Content-Type-Options: nosniff` and `Content-Security-Policy: sandbox`, and with
  `Content-Disposition: attachment` unless it is JSON, SSE or a picture (not SVG), video or sound. A redirect's
  `Location` is dropped and never followed.
- `DELETE /v1/machines/{name}[?force=true]`: `Ended {machine, changed, abandoned}`, `202` while `ending`. On a rental,
  `DELETE /v1/machines/local` asks its own machine to release itself: `202` with `ending`, or `409 machine_refused`
  (`machine_working`) while a job is queued or running, whatever `force` says, since a forced End is the renting
  computer's. It can't read the Hub, so nothing here can say the pod is gone. Elsewhere `local` is refused `machine_local`.
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
  - Each component is `{"version": "0.33.1"}` or `{"file": "/abs/path"}`. A file is a dev build on this
    computer: a wheel named `<distribution>-<version>-….whl`, or a `cozy` binary.
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
- `GET /v1/machines/local/authorized-keys`: `AuthorizedKeys {file, keys}`. These are other computers'
  controllers that this computer's machine admits.
- `PUT /v1/machines/local/authorized-keys/{key}` and `DELETE …/{key}` answer `Authorized {key, file, changed,
  machine, image}`.
  - `key` is an Ed25519 public key, 32 bytes of unpadded base64url.
  - The answer comes at once. `local` reads its keys again on the call this makes, then listens on every
    interface once it is idle, and `machine.reach` says where other computers add it.
  - A removed key's next call is refused.
- `GET /v1/rental-skus[?hub=]`: `RentalSkus {rental_skus}`. These are the Hub's products for rent now,
  cheapest first by their fewest GPUs. A product the Hub cannot sell now, for any reason, is not listed.
- `POST /v1/rental-quotes` (`NewMachine` rent): `Quote`, the Hub's price for exactly that order: `gpu_usd_micros_per_hour`, what a
  person is shown, and `agreed {usd_micros_per_hour, usd_micros_per_gb}`, the terms an order sends back. So does `RunQuote.quote`. Nothing is
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
a client branches on it: `account.name_unavailable`). Records are `protocol::hub`.

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
  what its header says, read once from the Hub and fetching no weights: `kind` is `lora` (only a LoRA's
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

**Asks that become runs.** Each is a POST with an `Idempotency-Key`, `202` with the run
(`calcifer::Submitted`), followed on `GET /v1/runs/{id}/events` like any run. Each has a quote route
taking exactly its body, which answers a `new` run's `RunQuote` and rents nothing. What Calcifer does
once the machine completes the run happens before the run settles and is in its result
(`result.after`, `AfterRun`), so a client that stops following never skips it.
- `POST /v1/model-downloads` `ModelDownload` (quote: `/v1/model-download-quotes`): a warm run that
  makes a model present in a machine's store. A local `.safetensors` file is named by its absolute
  path and needs an alias, `local/<name>`.
- `POST /v1/model-uploads` `ModelUpload` (quote: `/v1/model-upload-quotes`): a warm run that makes
  the source and uploads it as a private commit of `destination` in `lane`; `ModelUploading`, the run
  and the mirror recorded before it (`mirror`). `source` is a provider source, a Hub model, a
  `local/` alias, an absolute path to a file or a CozyTensors folder, a held file's `sha256:<hex>` (`PUT /v1/files`, `404 file_not_held`), or `run:N[#output]`, which runs
  on the machine that holds the output. A commit, `org/name@sha256:…`, on no `base` is at the Hub
  already: it's recorded as a commit of `destination` when the caller may read its whole closure,
  checked against `lane` before the mirror is recorded. That answers `200` with `recorded`
  (`RecordedCommit`) and no run, and its quote answers `null`, since nothing rents. Its trigger and strength (the flags, else the source's own)
  are set at the Hub once it completes (`result.after.display`).
- `POST /v1/model-quantizations` `ModelQuantize` (quote: `/v1/model-quantization-quotes`, published
  packages only): Calcifer picks the commit (a release's original weights, else the commit named),
  reads its header's components, matches the package's recipe for the lane and the calibration
  models it builds, and calls the package's `quantize`. Local code not yet ready answers its install
  (`preparing`): follow it, then send the same request under the same key.
- `POST /v1/package-installs` `PackageInstall` (quote: `/v1/package-install-quotes`): a warm run of a
  package's environment, no weights; the Hub's newest release when none is named. Once it is in, the
  older releases of the package the machine holds from the same Hub are removed, one remove run each
  (`result.after.retired`). A package directory or script is named by its absolute path and
  retires nothing.
- `POST /v1/package-removals` `{hub, packages, machine}`: `PackageRemovals {machine, runs}`, one
  remove run per package and Hub the machine holds it from, every release; a name the machine does
  not hold is refused before any run. Each run's result is `{removed, reclaimed_bytes, retained}`.

**Packages.**
- `GET /v1/packages?q=&limit=` (1–200, 20): `PackageSearch {packages, total, older}`.
- `GET /v1/packages/{org}/{name}?release=`: `PackageDetail`, releases newest first; one named comes
  with what its publish warned of, to its owner.
- `POST /v1/packages/{org}/{name}/releases/{release}/yank` `{hub}`: `PackageYanked`; machines keep it,
  only an exact pin installs it. `DELETE` on the same path un-yanks it: `PackageUnyanked`.
- `DELETE /v1/packages/{org}/{name}?hub=`: `PackageDeleted`, every release, its name free again.

**Authoring.** Calcifer locks and publishes a project directory on this computer with uv (fetched
when missing), never running its code. Its copy leaves Torch to the machine (`exclude-dependencies`:
torch, torchvision, torchaudio, triton; a cap or exact pin on one is a notice). Each is long work:
`202` with `Authoring {id, state, notices, locked | published | error}`, kept while this Calcifer
runs; asking again after a restart is safe.
- `POST /v1/package-locks` `{hub, dir, root, upgrade}`: locks for the Hub and account it publishes to
  (`root`: the root namespace) and writes back only `uv.lock`; ends `locked`.
- `POST /v1/package-publications` `{hub, dir, root, yanked}`: stages, relocks for this Hub's account
  index, builds, carries its git dependencies' wheels, describes, uploads what the Hub lacks and
  finalizes (`yanked`: committed yanked, never the newest release); ends `published`
  (`status: already published` for a version the Hub holds).
- `GET /v1/package-locks/{id}`, `GET /v1/package-publications/{id}`: the work as it stands.
- `GET …/{id}/events`: `notice {message}` events (uv's warnings, an upload sent again), then one of
  `locked`, `published` or `failed {error}`; each with its number as its `id:`.

### Local code
Local code is a target like any other: `./dir/function` or `./script.py/function`, an absolute path once a
client has resolved it (Runs). Calcifer reads the package or PEP 723 script, keeps its manifest under
`~/.cozy/calcifer/local/`, and makes it ready on the machine a call goes to: an install run first when that
machine lacks it, then the call with its files. Its interface comes from Runtime's static describer on the
Python and cozy-runtime its project locks; nothing runs to describe it.

### Projects
A project is a folder holding `project.yaml` (what a person or the page says, which Calcifer edits line by line, keeping the rest as it stands; `PROJECT-YAML.md` is the format), `project.lock`
(Calcifer's), `files/`, and its edits at the root (`<name>.mp4` and `<name>.otio`). Its id is fixed by its folder. Every file the project
uses is a real file in `files/<sha256>.<ext>`, flat. project.lock is YAML. Its `files` table has a row only for what a file can't say, where it stays and its
parts, and what's measured of a file is kept in `.measured.json`, which can be lost. The lock names nothing of this computer, so it travels with project.yaml. A yaml of an older format is refused
and left as it is, `next` naming `cozy project upgrade`, which the first touch of it runs: format 2 to 3, then 3 to 4, each box's state carried over.

An asset's state is its `approved` hash (a hash of a version's main file brings its whole file set), else its
newest version whose `inputs` match now, else none; with no `tool`, its newest version, which nothing puts out of date. A box is its words, an optional tool, and its versions: with no tool and no versions it's a text asset, whichever of `text`, `summary` or `instructions` holds its words. `inputs` is blind to names: eight hex digits of its `tool` (no release, no `_/`), `models`, `lora` and
its fields as written, after defaults, each box they mention replaced by its number in order of first mention (`<Amy.2>` is `<#0.2>`), then twelve for each of those boxes in that order: a box's `description` and the files of each version read, by sha256, or a text box's words, numbered on. So a rename puts nothing out of date, and a description edited does, `<Abr.2>`'s too. What a package is given has each text box's words in place. A tool Calcifer runs holds only what it reads of its inputs, and the files. A read gone from the project is no change. The seed, a pickup, the release and the machine aren't in it.

A mention reads an asset's state, and moves with it: `<abr>`. `<abr.2>` reads that version, fixed, its number never reused, and `<self.2>` a version of the asset whose text it is. Neither is a dependency: what's named is a file that's there, so it waits for nothing, closes no cycle, and a render of the asset that reads it makes none of what it names. A name holds no `.`: `<name.N>` is a version, `<name.word>` an extra output of its version (`<page.cleaned>`), and `<name:48-120>` frames 48 up to 120 of a video, `<name:48->` to its end, cut here before it's sent. Names match in any case, with `_` and `-` alike. `<self>` alone would change its own input on each render, so it fails like a mention of nothing, `input_missing`, with the fix; so does a version deleted or never made, whose name is in `missing`.

An operation is a tool (`[org/]package/function[@release]`, or local code, `./dir/function`), and its `kind` is the file type it produces, `image`, `video` or `audio`, by its output, its file or its version, never by the file saying so. `x/y` is root package x's function y where the Hub has one, else account x's package y, as `GET /v1/targets` resolves it. Its function is called directly, with no CPU run first, by one binder, the same for Render, `cozy project render`, `cozy run` and Caption, by the fields' types: a `str` takes words, a mentioned box with a file going as its name, with a warning; a prompt goes as `{text, references}`, its text as written with each mention spelled as its box spells itself, and `references` a list in order of first mention, `[{key, type, file, description}]`, `key` that spelling (`Amy`, `Amy.2`, `walk:48-120`), only of the kinds it takes: another goes as its name, with a warning; a slot takes only the file written in it, with its `description` and its version's extras of the same name (`latents`) when it's a `described` slot and the same package made it, and `own` true for one of the box's own versions; a slot written with a file of another kind is left empty for that render, with a warning naming it; a pickup comes only from `runs`' `pickup`, never the file: `{take, out_frame, …}`, its frame on the tool's `Snap`; a text box is put in as its words, anywhere. A tool naming a package with one public function that makes its kind calls that one. A number past its bounds is clamped, a choice it doesn't offer is its default, a shape the nearest it takes and a list cut to its count, each with a warning; a mention of nothing, a required field left empty, or one of a type this cozy can't fill (`next`: `cozy update`) fails that box before anything is sent. A failed call stops the list and makes no version; a function taking `seed` gets the one Calcifer drew, kept for the next try when the
`models` and `lora` go on its call: a tool's `model` is what they mean with no slot named, the one model its function runs, its `slot` the key in `models` and `family` what the header of its `default` says. A tool whose function runs several, or none, has no `model`. The version records its run, and each file by its output's name. The package `cozy` is reserved: `cozy/edit`, a video, and `cozy/frame`, a picture, run here, free, with no machine, once what they
read settles and on `approve`, reading each asset's state, else its newest version, which they say. An edit keeps its versions, each with the clips that made it, and the same edit isn't rendered again; a frame keeps one and its approved one.

- `POST /v1/projects` `{name}`: a new empty project at `~/<name>`, in this person's home folder, through `as_name` with `.` as `_` (`untitled` when nothing is left), taking `<name>_2` when anything is there, a project or not, which stays as it is and `said` says so; the same `Idempotency-Key` again answers the same project. `{path, create?}`: the project holding `path` (the nearest folder up from it with a
  project.yaml), or with `create` a new one made at `path` (`409 project_exists`). Answers `{id, path, name, next?, said?}` and
  records it in the index as used now. A project of format 2 upgrades here, with `said` saying so once; `GET /v1/projects/{id}` says it in `warnings` if that came first. What can't convert refuses with why. `404 project_not_found` when no folder up from `path` is a project.
- `POST /v1/projects/{id}/imports` `{source, name?, at?: {x, y, w?}, description?, exact?}`, with an `Idempotency-Key` (the same one again answers the same thing, for a day; on another body `409 idempotency_conflict`): `{name, sha256, field, path, already?}`. One call makes an element: the file in `files/`, `{approved, description?}` under its name in `project.yaml`, and with `at` its row on the canvas. `source` is `sha256:<hex>` (a held file, moved rather than copied, or one the project has; else `404 file_not_held`), an absolute path, or an `http(s)` URL (64 MiB at most, public addresses only at every hop). `name` absent: the stem of a path or URL, lower-cased, else `file`. It goes through `as_name`, and a name the project has, by the rule names match, gets `_2`, so `name` in the answer is the one it got. With `exact`, a person typed it: `409 asset_exists`, naming what has it. The same file twice is a second element with no new bytes, `already` saying so. `field` is `image`, `video` or `audio`, from the bytes; anything else is `400 import_failed`. A `project.yaml` that doesn't read is `400 project_yaml`, the file kept for a retry.
- `GET /v1/projects?limit=&before=`: the index, last used first: `{projects: [{id, name, path, used_at_ms,
  missing?, busy?}], older}`. `missing`: the folder no longer holds project.yaml.
- `DELETE /v1/projects/{id}`: `204`, forgotten from the index; the folder is untouched. `409 project_busy`
  while it renders.
- `GET /v1/projects/{id}` (`?assets=false`: all but its assets; `unsent`: what kept its last render from reaching a machine, each error once, until another is asked): worked out once per revision of its files, so another read costs only its answer. With a project.yaml that doesn't read and a render going, what it owes in `queue`, what's wrong in `warnings`, and no assets; with none going, that file is refused. `{id, path, board?, assets, queue?: [{name, running, run?}], missing, stale: [{name, why}], orphans: [{name, kind, versions, bytes, renamed_to?}], busy, moving,
  absent, cuts: [{cut, path, fps, cut_at_ms?, frames?, retimed?, skips?, without?}], warnings: [{name?, message}], next?}`. `next` is the commands to run given what it holds and lacks. Each asset is `{name, standing, making?, kind, noun, tool?, versions, prefixes?: [{number, prefix, pickup?, latents?, execution_ms?, run?, main?, from?, without?}],
  version?, approved?, pinned?, inputs?, files, path?, sha256?, field?, width?, height?, duration_ms?, alpha?, absent?, machine?, made_by?, status, reads?: [{name, version?, at, key?}], fields?, etag?, failure?, changed?, missing?}`: `standing` is `current`, `out_of_date`, `missing` or `making`, for a client that shows it, `making` the runs making it now, `version` the version that is its state, `approved` is the version a pin is, `pinned` that its `approved` is its state,
  `files` its state by output name, each `{path, sha256, absent?, machine?}`, `absent` that its bytes aren't on this computer and `machine` the machine that keeps them when one does; `path` its main file, `sha256` and `field` that file's hash and the field naming its type, `width`, `height` and `duration_ms` what is measured of it so far, `alpha` that part of it is fully transparent, `absent` and `machine` as for a file, and `status` is for people, in the asset's noun. A prefix's `pickup` is the take it picked up from, `{take, out_frame, beats_from?}`, and `latents` the machine keeping its latents, which a pickup of it needs. Its `main` is that version's main file, `{output, path, sha256, field, width?, height?, duration_ms?, fps?, alpha?, absent?, machine?}`, `output` being what `GET /v1/runs/{id}/play` takes of its `run`; `execution_ms` is how long its runs took. `from` and `without` are a cut's: what that version was cut from, and what it was cut without, each `<piece>: <why>`. `reads` is what its text mentions that names something, with `version` the one a `<name.N>` fixes, `at` the places that say it (`summary`, `beats.0.description`, `tracks.0.clips.1`), and `key` twelve hex digits of what that read sends now (its version's files and its box's description, or a text box's words), which moves only when what it reads does. `fields` is its inputs, flat, each by a control's `field`, as they apply now, its text as `instructions`, `tool` what makes it, `etag` what `PATCH`'s `If-Match` names. `failure` is what stopped its last try, the one error envelope: it stands until a version lands, it's tried again or its inputs change, and canceling is none. `changed` is a pin's: what changed since the version it holds, and it stands as `out_of_date` until `runs` with `outdated` (or naming it) re-runs it. An input gone from the project isn't a change: it is in `missing`, and `outdated` leaves that pin alone. `input_missing` is a `<name>` in its text, or a file link of an entity it reads, that names nothing and is no tag of its package: nothing was sent, so nothing was charged. `missing` is those names, once its package's tags are known; a pin lists them too, for its text, though it stands until it is run again. A warning's `name` is the asset or entity it's about, and it has none when it's about the file. A box renamed by hand in project.yaml keeps its versions, approval and canvas row (decision 6): when the file is read, a name with no versions takes the row of a name gone from it when exactly that row holds a version of the same content, by its name-blind key or the file it approves, and no other new name claims it. An orphan is an asset project.lock keeps
  versions of that project.yaml no longer names; its `bytes` count what `reclaim` would free of it, each file once; `renamed_to` the one new asset of
  its kind with none. A project's `absent` counts files the lock names that are not here and no machine keeps. `warnings` holds what project.yaml's reading said, what each
  state's version says, and each asset whose tool names a published package, as last heard of, with no one function that makes it. A read waits on no Hub and never writes `project.lock`: a package not heard of yet is asked of in the background, and a later read or a render says it.
- `GET /v1/projects/{id}/assets/{name}?call=`: `{asset, versions: [{number, files, made_by?, seed?, runs, approved?, warnings?, pickup?, execution_ms?, text?}], discarded?: [number], call?, call_warnings?}`. With `call=true`, `call` is what its binder would send now, the JSON `cozy run --input` takes, with what binding said (`cozy project status <name>` prints it). `text`: the words that version was made from. `discarded`: what `PUT` on a version can still bring back.
- `POST /v1/projects/{id}/assets` `{tool?, at?: {x, y, w?}, name?, fields?, exact?, kind?}`, with an `Idempotency-Key`: `201 {name, etag}`. A tool dropped on a canvas: an asset, and with `at` its row, in one call. `tool` is an `id` from `GET /v1/tools`, and `name` and `kind` what its row says, so a page's drop waits on no read. With no `kind` that isn't held already, or no `name`, the tool is read, refusing what can't be one (`400 tool_names_no_function`, `400 tool_makes_no_media`), and a Hub that's away refuses nothing: its render says. With none it's a text asset, `kind` `text`, its words in `fields.text` (`400 asset_says_nothing`, and `400 asset_name_reserved` for a tag of a package the project uses). `exact` says a person typed `name`: `409 asset_exists` if the project has it, naming what does, not `_2`. `name` goes through `as_name`, a `.` staying a character like any other but for an ending of `.` and digits, which says a version and is `_` instead, `400 project_name` with `exact`; `self` is taken; absent, it's the tool's function. A taken name, or one whose versions wait in `project.lock`, gets `_2`. The same key again answers the same asset; on another body `409 idempotency_conflict`.
- An Edit's `fields.timeline` is `{tracks: [{clips: [{clip: "<name>", in?, out?} | {clip: "<name>", frames?}]}]}`: one track for now, a clip's place in the list its place on the track, with no gaps. `in` and `out` count a video's frames from 0 at the Edit's rate, `out` exclusive, as OTIO and an EDL do, and no `out` is its end; `frames` is how long a still is held, 3 s when absent. Its row and each of its `prefixes` also say `timeline: {fps, tracks: [{clips: [{clip, in?, out?, frames?, name, field?, path?, sha256?, holds?, width?, height?, missing?}]}]}`: the clips as they read now, or as they read when that version was rendered. `holds` is how many frames a video has at that rate, and `missing` why a render goes on without the clip (`no longer in the project`, `no version yet`, `not on this computer`). `rendering`, 0 to 1, is how far a render here is, with `standing: making`; each change says `project.changed`.
- `PATCH /v1/projects/{id}/assets/{name}` `{fields?: {<field>: value | null}, tool?}`, with `If-Match` if it should not write over another's edit: `{name, etag, was, warnings?}`. `field` is exactly a control's `field`, or any key of the asset's `fields`, `description` among them; a null clears it. `tool` changes what makes it, and `""` takes it off, its fields with it (Paul, 11-06; Undo writes them back): its versions, their numbers, `approved`, `description`, `models`, `lora` and its words stay. Only a box with no versions and no `approved` moves its words: `text` to `instructions` as a tool goes on, `instructions` or `summary` to `text` as it comes off. A tool going on a picture no version holds lists it as the next version, as if it had made it, and starts from what the box shows, approved else newest: `<self.N>` in its first required picture slot, else at the start of its first prompt. Its approval, or none, stays as it was: a tool approves nothing. Naming `instructions` or `text` in `fields` says what they are instead. Only its own lines change, so a hand's comments and `|` blocks stay. Setting what it holds changes nothing, and a render going keeps the text it was asked with. `If-Match` is the asset's `etag`, the digest of the asset as it's written, so an edit of another asset never conflicts: `409 asset_changed` when it moved, and the answer's `was` tells a window whether a write of its own is all that moved it.
- `DELETE /v1/projects/{id}/assets/{name}`: `{name, text, at?}`, it and its row gone, at once, and what it made stands. Its versions wait as an orphan's, its name free, for `rename`, for `PUT`, or for `reclaim`. `text` is opaque, and is what puts it back, not a render it stopped: what a render still owes of it stops, queued with its row saying so, running canceled, and landing nothing. `400 project_edit` for what isn't under `assets:`; `404 asset_unknown`.
- `PUT /v1/projects/{id}/assets/{name}` `{text, at?}`: `{name, etag}`. What `DELETE` answered, back under its name, its versions with it. `409 asset_exists` if the file has that name again; `400 asset_text` if `text` isn't what deleting it answered.
- `POST /v1/projects/{id}/assets/{name}/versions` `{source}`: the same answer as below, with `already` when a version holds that file, then that version. A file joins the asset as its next version, so versions of one thing live on one box. `source` is an import's, and a file it holds with no version is a version first. What's approved stays. `404 asset_unknown`, `404 file_not_held`, `400 import_failed`.
- `DELETE /v1/projects/{id}/assets/{name}/versions/{number}`: `{name, number, noun, approved?, stands?, stale?, read_by?}`, that version off the asset at once, local, its number never given again and its files on record until `reclaim`. `approved`: it was the approved one, and `approved:` went with it. `stands`: what the asset stands on now, else its newest; absent, it has none left and stays with no take. `stale`: what is out of date now and wasn't. `read_by`: what fixes this version in its text: it doesn't go out of date, and a render of it fails until the version is back. A take that picked up from it (`pickup`) stands. `404 version_unknown` if it has no such version or it's discarded already; `409 version_running` while that number is being made, with `next` the `cancel`.
- `PUT /v1/projects/{id}/assets/{name}/versions/{number}` `{approved?}`: the same answer. What `DELETE` set aside, back in its place, and with `approved` from its answer its approval too, unless another has come since (`approved` absent). `404 version_unknown` once `reclaim` has cleared it.
- `POST /v1/projects/{id}/assets/{name}/caption` `{on?}` (`Idempotency-Key`, optional): `{name, text, run, warnings?}`, its description as a package would say it, for the page to put in the field. It writes nothing. It runs on a machine that's up (`409 caption_no_machine`), on the asset's state, else its newest version (`409 caption_no_file` with neither), and answers when the run ends. The package is the first that declares `project-adapter-text` of the local code this project's assets name, what that machine holds, and the Hub's first page (`404 caption_no_tool`). Its adapter, told the file as a read, says what to ask, so the words are the package's; the answer is the first text of the function's output. `caption_failed`: a run failed, and `next` names it.
- `GET /v1/projects/{id}/yaml`: project.yaml's bytes, its `ETag` the digest of them (`If-None-Match` is `304`).
- `PUT /v1/projects/{id}/yaml` with `If-Match` (`400 if_match_required` without): the body becomes project.yaml, if it still holds what that ETag read, else `409 project_changed` and it stands. `204`, the new `ETag`. A body that doesn't read as a project is `400 project_yaml`, the file untouched. It renders nothing, though a render going keeps the text it was asked with.
- `GET /v1/projects/{id}/history?limit=` (50, at most 200): `{entries, undo?, redo?}`, newest first, each `{id, at_ms, by: {via, client?}, words, names?, edits?, undone?: {at_ms, by}}`. Every edit of the project is one entry: each route below that changes project.yaml, a canvas row, a take or a name (imports, assets, versions, approve, unpin, rename, canvas, yaml), and a hand's edit of project.yaml, noticed by the look a stream makes or by the next write, `via: file`. `by` is what the request's `Cozy-Client` says, `page <tab>` or `cli`, else `api`. A request's `Cozy-Edit: <id>[, <id>…]` names what the client calls it: requests that share one are one entry, and undo and redo take it as `edit`. A canvas `PATCH` whose every row is in `was` is the page arranging its own boxes, and none. `words` say it plainly (`Deleted kyogre_3`, `Merged green into red`, `Switched image_2 to generate_turbo`, `Set instructions on image_2`, `Discarded take 3 of fight`, `Moved image_2`). Kept in `.history.json` beside project.yaml, never in it, across restarts. Renders, cancels, reclaim, push and pull are no entry: nothing is rendered, rented or ended by undoing.
- `POST /v1/projects/{id}/history/undo` `{entry?, edit?, to?}`: `{entries, already?, stopped?, undo?, redo?}`. That entry, by `id` or its `edit`, put back as it was; with `to`, each that stands back to it, newest first; with nothing, the newest that stands, whoever made it. Each thing it changed goes back only while it's as the edit left it, else `409 history_conflict` naming the later edit, with `next` undoing that first; refused part way, it changes nothing. A row is put back whatever moved it since. `already`: it was undone. `stopped`: what refused one of a `to`, after those before it. `404 history_unknown` for an entry it no longer keeps, `409 history_empty` with nothing to undo.
- `POST /v1/projects/{id}/history/redo` `{entry?, edit?, to?}`: the same answer, the other way: what undo took back, done again, only while it's as undo left it. With nothing, the entry undone last, unless an edit came after that undo; with `to`, each undone since, up to it.
- `POST /v1/projects/{id}/runs` `{names, n?, on, hub?, approve?, missing?, outdated?, pickup?: {take, out_frame, beats_from?}}` (`Idempotency-Key`, kept in `.runs/walk.json` while the walk goes and in `.keys.json` for a day, so a retry adds nothing even across a restart): answers `{id, path, joined?, added, skipped?, queued?: [{name, ahead?}], next, board}`. `pickup`, for one name only: Pick up from here, its new take going on from `take` (`<shot_1.3>`, its own or another box's) at `out_frame`, recorded on the take and never written to project.yaml. Each name is `n` new versions, and `all` one of every asset with an operation, whatever `n` says, with a warning, in reading order, unless `missing` asks only for what has no state. `skipped`: approved assets it was asked of by `all`, left as they are; naming one makes a version to look at, and its pin stays. `joined`, a render was going already; `added` 0 with it is no new work; `queued`, what it added, and the names ahead of each; `board`, the render as it now stands. It captures project.yaml as it is, kept in project.lock until it is done, and plans on that text alone; a file that doesn't read refuses it. Asks go at once. each name, or `all`, and what it reads: run A render is walked here and drives `on`, whatever its tools: a machine runs the calls it's sent, with the files each is given. Of several machines in `on`, each job goes to whichever ends it soonest, by what runs of its function took on each, else on another machine with its card, else as long as on the slowest that's known; `local` loses a tie, and takes nothing it would do poorly (as `fit` judges) while another can, unless what ran on each says it ends it first. The page and `cozy project render` with no `--on` send every rental of yours up or on its way, another computer's that's up, and `local` last (`MachineList::render_set`).
  its operation if it has no state, and nothing else. `new`, or `n` over 1, also draws `n` new seeds for those named; a pinned one's state doesn't move.
  `--take`, `--in`: one asset's new take keeps the start of that take, or the one in use (`400 take_unknown`), for a function that fills `continue_from` from `take`: it gets that take's `video`, `latents` and `at_s`, and one its package didn't make is `409 take_not_continued`. `202 {id, path}`, or `200` when
  its key asked it already. The work goes on in the project's walk and is followed on its events.
- `POST /v1/projects/{id}/fit` `{names, outdated?}`: `{tools: [{tool, label, names, line, bytes, here?}], rent?: {sku, gpus, card, vram_gb, usd_micros_per_hour, line}}`. What a render of `names` (or `all`, as `runs` reads it) on this computer alone would do poorly, a tool a row, said before it's sent. A tool is done poorly with no GPU when its function loads a model on one, as it would then run on the CPU, degraded, and on a GPU its weights fill more than three quarters of, the Hub's rule for a card that holds them. Its `bytes` are what the Hub places a run of its release by (`POST /v1/rental-placements`), local code's those of its models' releases, either with a checkpoint on a box in place of its model's; 0 when the Hub doesn't say in 5 s. `line` says how, and how long its last run here took; `rent` is what the Hub places them on: the biggest card of its picks for those not yet said for, else for all. `here`: Render here was said of it on this computer. A client asks this only of a render whose machines are `local` alone: with another, the walk gives `local` nothing it would do poorly. Nothing is listed while `local` hasn't said what it has, for what loads no model or runs on no GPU, or for what Calcifer makes itself. What the Hub says is kept 10 minutes.
- `POST /v1/projects/{id}/fit/here` `{names, outdated?}`: the same, each tool listed kept as rendered here in `~/.cozy/calcifer/here.json`, by what this computer's GPUs are (their names and usable memory, or none) and its weights: `here` then holds until another computer, or weights half as big again, such as a bigger checkpoint on its box.
- `POST /v1/projects/{id}/approve` `{name, version}`: `{name, noun, number, files, path, stale, warnings?}`. `version` is a short run of digits, its version, or anything else a hash prefix, which pins any file of the project; `number` is 0 when it is no version's. Writes the version's main file's hash as that asset's `approved`, over what the file holds this moment.
  Makes any tool whose reads settled, with no machine.
- `POST /v1/projects/{id}/unpin` `{name}`: `{name, version?, stale}`. Takes its `approved` away. A constant, nothing but its file, keeps it (`400 project_edit`).
- `POST /v1/projects/{id}/rename` `{from, to}`: `{from, to, versions}`. With `from` under `assets:` and `to` free, project.yaml's key and every `<from>` and `<from.N>` change. What project.lock keeps of `from` is kept as `to`, an asset project.yaml names now with no versions (`409 rename_taken`), and its canvas row moves. `400 project_name` when `to` can't be a name: it holds what a name can't, ends in `.` and digits, or is `self`. It happens at once, whatever is going, and changes nothing else: a render going says `to` from then on, what lands is `to`'s, and what reads it reads on. Nothing on disk moves. A name is no input: what read it stands as it did, each key written anew.
- `POST /v1/projects/{id}/reclaim` `{yes?}`: `{orphans, files, bytes, unrecorded?, deleted?, next?, lost?}`. `unrecorded`: how many of the files nothing has a record of, which `yes` leaves. `yes` clears discarded versions, and keeps what another version names, a clip of an Edit's version cut, or a `<name.N>` in any text reads, with its record, a deleted asset's too. `lost` says why `yes` would be `409 project_history_lost`: project.lock is gone, can't be read, or lists no version or run while `files/` holds what nothing names. Lists orphans and the files nothing else names; with `yes`, deletes them, an orphaned cut's film at the root too, and canvas rows nothing names (`409 project_busy` while it renders). Never automatic.
- `POST /v1/projects/{id}/push` `{hub?}`: `202 {id, path}`, `401 not_logged_in` signed out. Sends every file
  project.lock names that the Hub lacks, then records them as a private commit of `<account>/project-assets`. A file over 5 GiB goes as 1 GiB parts,
  each its own object, which its first push lists in project.lock under the file's `parts`. `transferred` on the events ends it.
- `POST /v1/projects/{id}/pull` `{hub?}`: the same, fetching every file `files/` lacks, as real files, each held to its digest.
- `POST /v1/projects/{id}/cancel` `{names}` (or `["all"]`): `{dropped?, canceled?}`. Drops their queued work and cancels their runs; nothing dropped is sent again.
- `GET /v1/projects/{id}/events` (SSE): `board` when it opens, when a render begins or ends and if the stream falls behind, `job {job, seq}` for each job as it changes, `ahead {ahead}`, `started`, `landed`, `said {message, name?}`, `warning {message, name?}`, `failed {error, name?, run?}`, `moved`, `transferred`, `cut {name, cut, …}`, `ended`. It opens with a `started` for each run already going, so one that comes late hears them at once; one that began just then can be told twice, and with the `board` as it stands. Each `board` is the whole render, sent at each change, with nothing queued behind a slow reader: all a view needs, with no run to follow. `{seq, on, began_at_ms, elapsed_ms?, shots, jobs: [{asset, state, noun, number?, shot?, why?, run?, machine?, started_at_ms?, took_ms?, stage?, detail?, fraction?, stalled?, preview?: {label, path}, path?, seconds?, error?}]}`. `seq` only rises: keep the highest heard. `jobs` go what has ended, then what is going, then what waits. `state`: `waiting`, `running`, `done`, `failed`, `canceled`, `not_sent`. `why` is a sentence: `after a, b`, `comes with c`, `waiting for a GPU, held by d`, or what came of it. `shot` is which of the render's `shots` it makes, 1 first, what's queued counted: a `kind: video` its package makes, not a cut or a still, the same number for the whole render, and `shots` grows only as another render joins. `stage` and `detail` say where it is: `segment 2/2, denoise`, `14/30`. `preview` is what the run has shown so far while it goes on, never its result. A `warning`'s `name` is the asset whose run gave it, and the same `message` can come from several. `ended`'s `error` is the first failure, or when more than that was asked, `render_incomplete`: `rendered 6 of 15; 9 failed: <the first>`, with the command for the rest in `next`. A second run failing the same way on a machine stops what that render sends there, and cancels what it sent that has not begun.
- `GET /v1/projects/{id}/files/{sha256}.{ext}`: a file (Range, ETag its sha256). `GET /v1/projects/{id}/cuts/{cut}`: a cut or its `.otio`, as last made.
- `GET /v1/projects/{id}/archive`: `application/x-tar`, streamed as it is read: every regular file in the project's folder on this machine as `<folder>/<path>`, symlinks and `.part`s left out. A file the lock names that isn't here (kept on a machine, or needing a pull) is named first in `<folder>/elsewhere.json`, `{elsewhere: [{sha256, machine, length?}]}`, and `Cozy-Elsewhere` counts them. A read that fails fails the stream, so a tar never ends cleanly and short.
- `GET /v1/projects/{id}/canvas`: `{etag, cards: {name: {x, y, w?}}}`, from `project.canvas.yaml`. A name with no row isn't listed, and the page places it. `PATCH` `{cards: {name: {x, y, w?} | null}, was?: {name: {x, y, w?} | null}}`: `{etag, kept?}`, these rows set and a null one gone, last move wins, to a hundredth, and `canvas` on `GET /v1/events`. A name in `was` is set only if its row is still that (null: none), in the same write; one that moved is left as it is and answered in `kept` with its row now. The page sends it with what it works out itself, a new box's place or a box moved off another, so another writer's newer row stands; a person's move goes without. Nothing in it is an input.
- `GET|HEAD /v1/projects/{id}/thumbs/{sha256}?w=&strip=&preview=`: a thumbnail, with `Cache-Control: immutable`, `sha256` with or without its `sha256:`. A picture keeps its alpha (PNG; JPEG with none); a video gives a poster, or with `strip=<n>` (1 to 32) one JPEG of `n` frames side by side; a sound gives a waveform (PNG). `w` (default 480, 16 to 2048) is its width, or each frame's, and it is never wider than the file. A video's poster is the first of its frames at 20, 35, 50, 65 and 80% whose luma isn't flat (a spread under 12 of 255, so not black), else the busiest of them. `preview=true` gives a video's preview, whatever `w` and `strip`: 3 s from its poster's frame on, or to its end, at most 480 px on its longer side, silent, as H.264 MP4 with Range, for a page to loop on a hover. It is made once per file, as the video lands or is imported, else on its first ask. `404 file_missing` when the project has no such file here, `400 thumb_failed` when ffmpeg can't read it, which is kept for a day. Made by Calcifer's own ffmpeg, a few at a time, and kept under `~/.cozy/cache/thumbs` until unused for 30 days.

### Tools and Calcifer
Calcifer fetches the tools it runs on first need, pinned builds held to their sha256, into `~/.cozy/tools`:
ffmpeg (cuts, frames, clips) and uv (local code). No route asks for them.

- `POST /v1/down`: `{stopping: true, owed}`. Admission closes, mutations in flight finish, every stream ends,
  then the server. Machines keep what they accepted; the next Calcifer resumes `owed`. Never refused.

## CLI verbs and their calls

| Verb | Calls |
|---|---|
| `cozy status` | `GET /v1/status`, `GET /v1/machines`, `GET /v1/runs?status=…` |
| `cozy open [path] [--no-open]` | `POST /v1/projects {path}`, in a project `GET /v1/projects/{id}?assets=false`, then `GET /`, `POST /v1/session-codes` |
| `cozy project new <path>` | `POST /v1/projects {path, create: true}` |
| `cozy project ls` | `GET /v1/projects` |
| `cozy project render …` | `POST /v1/projects {path}`, `GET /v1/projects/{id}/events`, with no `--on` `GET /v1/machines` for the machines, `POST /v1/projects/{id}/fit` on `local` alone (each line once, as a warning: it never asks), `POST /v1/projects/{id}/runs`; the live view and its lines draw the `board` and each `job`; `--json` follows each `started` run on `GET /v1/runs/{id}/events` |
| `cozy project approve <node> <n>` | `POST /v1/projects {path}`, `POST /v1/projects/{id}/approve` |
| `cozy project push`, `pull` | `POST /v1/projects {path}`, `GET /v1/projects/{id}/events`, `POST /v1/projects/{id}/push` or `/pull`; exits 3 when `skipped` names a file |
| `cozy project status` | `POST /v1/projects {path}`, `GET /v1/projects/{id}` |
| `cozy project rename <old> <new>` | `POST /v1/projects {path}`, `POST /v1/projects/{id}/rename` |
| `cozy project history [-n N]` | `POST /v1/projects {path}`, `GET /v1/projects/{id}/history` |
| `cozy project undo [ID \| --to ID]`, `cozy project redo [ID \| --to ID]` | `POST /v1/projects {path}`, `POST /v1/projects/{id}/history/undo` or `…/redo`; every CLI request says `Cozy-Client: cli` |
| `cozy project import <file\|hash> [name] [--description <words>]` | `POST /v1/projects {path}`, `POST /v1/projects/{id}/imports` |
| `cozy project upgrade [--status <file>]` | No route: the folder's files, here, in this process, for a Calcifer of this format can't read them yet |
| `cozy project add <tool\|text> [name] [field=value…]` | `POST /v1/projects {path}`, `POST /v1/projects/{id}/assets`; `at=` its `at` |
| `cozy project set <name> field=value…` | `POST /v1/projects {path}`, `PATCH /v1/projects/{id}/assets/{name}`, `tool=` its `tool`, empty to take it off; `at=` `GET` it, then `PATCH /v1/projects/{id}/canvas` |
| `cozy project import <file> --into <name>` | `POST /v1/projects {path}`, `POST /v1/projects/{id}/assets/{name}/versions` |
| `cozy project status <name>` | `POST /v1/projects {path}`, `GET /v1/projects/{id}/assets/{name}` |
| `cozy project delete <name>…` | `POST /v1/projects {path}`, `DELETE /v1/projects/{id}/assets/{name}`; prints the `restore` line, its `text` and `at` as one word |
| `cozy project restore <name> <kept>` | `POST /v1/projects {path}`, `PUT /v1/projects/{id}/assets/{name}` |
| `cozy project delete <name> --version <n>…` | `POST /v1/projects {path}`, `DELETE /v1/projects/{id}/assets/{name}/versions/{number}` for each; prints each `restore` line |
| `cozy project restore <name> --version <n>… [--approved]` | `POST /v1/projects {path}`, `PUT /v1/projects/{id}/assets/{name}/versions/{number}` for each |
| `cozy project unapprove <name>` | `POST /v1/projects {path}`, `POST /v1/projects/{id}/unpin` |
| any verb, Calcifer of another version | `POST /v1/down`, then the installed cozy's Calcifer started |
| `cozy run <target> [inputs] [flags]` | `POST /v1/run-quotes` (`--on new`), `POST /v1/runs` (again after a `preparing` install), then `GET /v1/runs/{id}/events` |
| `cozy run <target> --describe`, `cozy run <package>` | `GET /v1/targets` |
| `cozy run ls` | `GET /v1/runs`; live: `GET /v1/events`, then `GET /v1/runs` |
| `cozy run show <run>` | `GET /v1/runs/{id}?detail=full`, `GET /v1/runs/{id}/events` |
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
| `cozy model download` | `POST /v1/model-downloads` (`/v1/model-download-quotes` first for `new`); `GET /v1/runs/{id}/events` |
| `cozy model upload` | `POST /v1/model-uploads` (`/v1/model-upload-quotes` first for `new`); `GET /v1/runs/{id}/events` unless it answers `recorded` |
| `cozy model quantize` | `POST /v1/model-quantizations` (`/v1/model-quantization-quotes` first for `new`), again while `preparing`; `GET /v1/runs/{id}/events` |
| `cozy package ls [query]` | `GET /v1/packages` |
| `cozy package ls --on` | `GET /v1/machines/{name}`, `GET /v1/machines/{name}/packages?fresh=true`; `--on all`: `GET /v1/machines` |
| `cozy package info` | `GET /v1/packages/{org}/{name}` |
| `cozy package install`, `update` | `POST /v1/package-installs` (`/v1/package-install-quotes` first for `new`); `GET /v1/runs/{id}/events` |
| `cozy package remove` | `POST /v1/package-removals`; `GET /v1/runs/{id}/events` for each |
| `cozy package yank [--undo]` | `POST` (`DELETE`) `/v1/packages/{org}/{name}/releases/{release}/yank` |
| `cozy package delete` | `DELETE /v1/packages/{org}/{name}` |
| `cozy package lock` | `POST /v1/package-locks`, `GET /v1/package-locks/{id}/events`, `GET /v1/package-locks/{id}` |
| `cozy package publish` | `POST /v1/package-publications`, `GET /v1/package-publications/{id}/events`, `GET /v1/package-publications/{id}` |
<!-- each PR adds its verbs' rows -->
| `machine ls` | `GET /v1/machines?refresh=true`; on a terminal, `GET /v1/events`, then `GET /v1/machines` |
| `machine ls --history [--since D]` | `GET /v1/rentals?since_ms=` |
| `machine show <name>` | `GET /v1/machines/{name}`; live on a terminal, as `ls` |
| `machine logs <name> [log] [--since D] [-f]` | `GET /v1/machines/{name}/logs/{log}?lines=&since_ms=` (SSE with `-f`). `logs local calcifer` reads Calcifer's log file, so it works while Calcifer is down |
| `machine rent` | `GET /v1/rental-skus` |
| `machine rent <sku>` | `POST /v1/rental-quotes`, `POST /v1/machines` (rent), then `GET /v1/events` until ready or ended |
| `machine end <name> [--force]` | `DELETE /v1/machines/{name}`, then `GET /v1/events` until ended |
| `machine keepalive <name>` | `POST /v1/machines/{name}/keepalive` |
| `machine update <name> [--runtime\|--tensorfs\|--tensord V\|PATH]` | `POST /v1/machines/{name}/update`, then the run's events. A project directory is built here first |
| `machine add <name> <address>` | `POST /v1/machines` (add) |
| `machine authorize <key> [--remove]` | `PUT` or `DELETE /v1/machines/local/authorized-keys/{key}` |
| `machine ssh <name>` | `GET /v1/machines/{name}` for its address, then `ssh` here |
| `update` (moving `local` to the new cozy) | `POST /v1/machines/local/update` with `{"tensord": {"version": V}}` |
