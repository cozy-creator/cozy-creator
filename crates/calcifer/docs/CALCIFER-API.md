# Calcifer API

Calcifer is the daemon on every computer that runs cozy. Its HTTP surface is the product: the `cozy` CLI and a
web UI are equal clients of it, and the CLI only parses arguments and prints (cl-716). Every behaviour a verb
offers is one or more of the calls below. Written by hand and changed in the same PR as the routes
(`crates/calcifer/src/api.rs`, `machines.rs`, `project/mod.rs`); the records are `crates/protocol`.

## Clients and discovery

Calcifer listens on `127.0.0.1` only: on `calcifer.port` from `~/.cozy/config.toml` and no other if it's set, else
on 7317 or the next free port, so a page's origin outlives a restart. It publishes a lock record at `~/.cozy/calcifer/calcifer.lock` (owner-only): `{address, token, pid, version,
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
- `project.changed {id, rev}`: `rev` is opaque, and changes whenever `project.yaml` or `project.lock` does, as a landing's does. While a stream is open, the projects open here are looked at about once a second for edits from outside.
- `canvas {id, etag, cards: {name: {x, y, w?} | null}}`: the rows of a project's canvas that changed.
- `resync`: this stream fell behind and goes on: read again what you hold.

### The page
The web UI, embedded when cozy is built (`crates/calcifer/build.rs`, from `web/app/dist`). It holds no secret, so these
two routes need no credential; the guard's first three checks still apply.
- `GET /`: its `index.html`.
- `GET /assets/{file}`: a file named by its hash, `immutable`; `404 asset_not_found` for any other.

Each answers `Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob: data:;
media-src 'self' blob:; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'` and
`Referrer-Policy: no-referrer`. A build without the page answers `503 page_not_built`, with how to build it.

### Status
- `GET /v1/status`: `{version, os, local_usable, signed_in, rental?: {machine_id, idle_deadline_ms?}}`. `rental` is there on a rental: `machine_id` is its Hub's name for this pod, which has no rental id of its own, and `idle_deadline_ms` when it releases itself unless a job comes, absent while one is queued or running. `os` is `linux`, `macos` or `windows`; `local_usable` is
  whether this computer runs models itself; `signed_in` is whether it holds a device key for its Hub, which it answers
  without asking the Hub (`GET /v1/account` does).

### Session
A browser signs in with a single-use code. `cozy web` and `cozy project open` write it into a 0600 redirect file
(`~/.cozy/calcifer/open.html`) and open that, so it's in no process's arguments, and print the link, as a snap browser
can't read that file; `--no-open` only prints it. The
fragment may add `&to=<route>`, as `cozy project open`'s does (`#code=<code>&to=/projects/<id>`). `~/.cozy/calcifer/sessions.json` (0600) holds sessions' digests; 30 days unused ends one.
- `POST /v1/session-codes` (the lock record's bearer only, else `403 session_code_needs_bearer`):
  `{code, url, expires_ms}`, 32 random bytes in hex, good for 60 s. `url` is `http://<Host>/#code=<code>`.
- `POST /v1/session` `{code}`, needing no credential: `{token, media}`, new for each code. `400 session_code_invalid`
  when it's unknown, used or expired.
- `DELETE /v1/session`: `204`, the session whose token this carries has ended.

### Runs
A run is asked for unbound (`RunRequest`); Calcifer resolves its target, binds it against the function's
interface, cuts and stages its files and builds the machine's spec (`crates/calcifer/src/runs`). No client builds
a spec.

- `POST /v1/runs` (`Idempotency-Key`): a `RunRequest`. A run needs a `project`, its id or a folder in or holding one: without it `400 not_in_a_project`, an unknown id `404 project_unknown`, and its outputs land in that project's `files/` and `project.lock`'s `runs`. Answers `202` (`200` when the key had made it) with
  `Submitted {run, replay, notes, preparing}`:
  - `target`: `[org/]pkg/fn[@release]` (no `@`: the newest release its machine holds that isn't yanked, else the
    Hub's newest, which it installs; a yanked one moves to the newest with a warning; `x/y` is root package x's function y, else account x's package y), or local code `./dir[/fn]`,
    `./script.py[/fn]`.
  - `on` (`local`, a machine, `any`, `new[:sku]`), `hub`, `gpus`, `lane`, `attention_kernel`, `project`, `warm`
    (a level, instead of running), `agreed` (a `new` run's quote).
  - The payload: `input` (an object), `input_file` (a JSON or YAML file here; files it names are relative to
    it) or `input_text` (the same read from text; files it names are relative to `cwd`); then `terms` override it (`key=value` typed as declared, `key:=<json>`, `model.<slot>=<ref>`, one
    positional value, input files), `loras` (`[slot=]ref[,strength]`), `files` (`{name?, file}`: an input's
    field path, a label, or none for the declared files) and `fidelities`. A file is a path here (a video's may
    end `#t=<start>,<end>`, cut once into `~/.cozy/cache/clips`), a directory at a tree input, a held
    `sha256:<hex>` (`404 file_not_held` when neither Calcifer nor the project holds it), or six or more hex digits that start the hash of a file of the
    project. Relative paths start at `cwd`; without one they are refused.
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
- `GET /v1/targets?target=&cwd=&hub=&on=`: what a run of `target` on that machine calls, resolved as `POST /v1/runs` resolves it:
  `{package, function?, release, release_from: asked|installed|latest|local, summary, interface, functions, contract?, request?, next?}`:
  `interface` is the package's interface document (local code's read from its source, installing nothing); `functions` `[{function, description}]` is what a
  person runs of it, with no internal function, recipe or project adapter. A named function is checked as a run checks it, and `contract` is its contract as people
  read it, `request` its schema. `next` names the functions to describe.
- `PUT /v1/files/sha256:<hex>`: a file's bytes, which must hash to its name: `201`, or `200` when held, with
  `{digest, length, media_type}`. `HEAD` asks whether it is held. A held file unnamed for a day is dropped.
- `GET /v1/runs` (`status`, `package`, `machine`, `project`, `before`, `limit`): `{runs, older, machine?, unreadable?}`. `project` is an id (`404 project_unknown`), or a folder:
  the runs of the project holding it; in none, all of them. Each run names its `project`.
- `GET /v1/runs/{id}`: the run. While it waits, `queue_position` (1 is next); while it runs, `progress` with
  `done_fraction`, `bytes_per_s` and `eta_s`. `?detail=full` adds `measurements`, `calls` and `report` (each
  executor's stages and unaccounted time, models, kernels, and each GPU's memory, with rates against its
  ceiling).
  A run settled over 30 days ago has dropped `measurements` and each call's, so its `report` is gone. Nothing else
  of a run is ever dropped: its row, `result`, reason, outputs, logs and `calls` stay for good.
- `GET /v1/runs/{id}/events` (`Last-Event-ID` or `?starting_after=`): `run.*`, `output_item.*`, `run.log`,
  `run.call`, and the live `run.progress` and `run.waiting`. It ends after the terminal event once every output
  has landed here or failed to.
- `POST /v1/runs/{id}/cancel`: `202`, or `200` when it had ended.
- `GET /v1/runs/{id}/play?output=&expires=` (`expires`: `90s`, `30m`, `24h`, `7d`, default `1h`): `Play {address, fingerprint, token, url, expires_s}`: how a browser plays that output straight from its machine, and `url`, the hosted player's link. `400 play_expires_invalid`, `400 play_output_invalid`.
- `GET /v1/runs/{id}/outputs/{name}`: an output's bytes. An item with `stays`, untyped bytes of a run that made something else, stays on its machine: no `path`, nothing waits for it, and this answers `409 output_on_its_machine`.

### A run that stopped moving

A running run with no progress for 10 minutes carries `stalled {since_ms, machine, usd_micros_per_hour?}` until it
moves, and its record gets one `no_progress` warning naming the machine and its price. Nothing ends it: only its owner does.

### Machines and rentals
<!-- cl-716 C -->
A machine is this computer's (`local`, started when used), a rental, or another computer's added by address.
`{name}` is a live machine's name or former name, or any machine's rental id. Records: `protocol::calcifer`.

- `GET /v1/machines[?refresh=true]`: `MachineList {machines, hubs, unanswered, signed_out, rentals}`. It lists
  every live machine, and the live rentals of this account that another controller rented (`recorded: false`).
  `refresh` lists every Hub again first. `rentals` sums the live rentals: count, `usd_micros_per_hour`, and
  `accrued_usd_micros` on its weakest `accrued_basis` (`accrued_floor` when some Hub said nothing).
- `GET /v1/machines/{name}[?fresh=true]`: `Machine`. A rental rented with an SSH key has `ssh {args, line}`: the command that logs in, with a known-hosts file of its own, as an argument list and as a line to paste. `fresh` reads its Status now, or refuses
  `machine_unavailable`.
- `POST /v1/machines` (`NewMachine`):
  - `{"kind": "rent", sku, hub, gpus?, disk_gb?, ssh_key?, agreed}`, with an `Idempotency-Key`. It answers
    `202 Rented {machine, quote, replay}`, or `200` on a replay. Follow the boot on the events until the
    machine is `ready`, or `ended` without one. `agreed` is the quote's `agreed()`: a dearer order rents
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
  TensorFS and TensorD (its `cozy`) in one update run and one restart.
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
- `GET /v1/machines/{name}/models[?fresh=true]`: `MachineModels {machine, models: [{machine, model, release, lane, precision, checkpoint, bytes, shared_bytes}], silent?, repositories, bytes}`.
  `silent`: it has reported no Status yet. `repositories` and `bytes` are its store's, shared content counted once.
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
- `POST /v1/rental-quotes` (`NewMachine` rent): `Quote`, the Hub's price for exactly that order. Nothing is
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
sends it back. A code is answered once.
- `GET /v1/account`: `Account`, who this computer signs in as, its credit (`billing`,
  `balance_usd_micros`, none where unmetered) and where to buy more; `key_refused` when the Hub no
  longer accepts the key.
- `POST /v1/account/codes` `{hub, purpose, email}`: `Code {id, email}`. `purpose` is `sign_in`,
  `export` (a key for another computer, such as CI) or `sign_out_everywhere` (mails the signed-in
  key's address; `not_logged_in` without one).
- `POST /v1/account/codes/{id}` `{code}`: `Proven`, tagged by `purpose`: `sign_in` keeps the key and
  answers the `Account` (`account` none until named); `export` answers the key file's text, kept
  nowhere here; `sign_out_everywhere` revokes every other key, then this one (`SignedOut`).
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
- `GET /v1/models?q=&family=&limit=` (1–100, 20): `ModelSearch {models, total, older}`, each model's
  newest release and lanes; signed in, also the caller's private ones. `q` that is exactly
  `org/name` is that model.
- `GET /v1/models/{org}/{name}?release=`: `ModelDetail`, its releases (the one named) and, to its
  org's members, every commit.
- `PATCH /v1/models/{org}/{name}` `{hub, family}`: `ModelFamily`; an empty family clears it.
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
A project is a folder holding `project.yaml` (people's; Calcifer writes one line of it, at `approve` and `unpin`), `project.lock`
(Calcifer's), `files/`, and its cuts at the root (`<name>.mp4` and `<name>.otio`). Its id is fixed by its folder. Every file the project
uses is a real file in `files/<sha256>.<ext>`, flat, in the lock's `files` table with what is measured of it once: `{path, sha256, width?,
height?, fps?, frames?, alpha?}`. The lock names nothing of this computer, so it travels with project.yaml. A format 1 yaml or lock is refused
and left as it is. `design/project-graph.md` in the tracker is the model.

An asset's state is the hash in project.yaml (`image`, `video`, `audio`; a hash of a version's main file brings its whole file set), else its
newest version whose `inputs` match now, else none. `inputs` is a digest of `with` (no release, no `_/`), `models`, `lora`, `options`, `kind`,
every other field after defaults, each asset it mentions (name, kind, fields, files by sha256) and the world states it writes: exactly what its
adapter is given. A tool's holds only what it reads of its fields, and the files. Calcifer knows no kind's meaning and no field's shape.

An operation is a package (`with`, `[org/]package[@release]`, or local code) and a `kind`, the file type it produces: `image`, `video` or `audio`. Calcifer runs the package's `project-adapter-<kind>` on
the render machine, on `{version: 2, asset, kind, fields, options, reads, writes, keep}`, and runs the `invocations` it answers in order as one version: a
string in `input` that is exactly `$0.video` is that field of call 0's result, as its digest. `[]` is no run: the asset's files are what it
reads. A failed call stops the list and makes no version; a function taking `seed` gets the one Calcifer drew, kept for the next try when the
run failed. `models` and `lora` go on the first call. `options`, every key of `render` but `with`, `prompt`, `models` and `lora`, merged over `defaults.<type>`, goes only to the adapter, and `prompt` reaches it as `fields.instructions`. The version records each call as `invocations`, and
each file by its output's name, later over earlier. `written`, a list, is a function's frames for `op.writes`, in order; a write with none is
cut from the landed video. The package `cozy` is reserved: a `video` (a cut) and an `image` (a frame) run here, free, with no machine, once what they
read settles and on `approve`, reading each asset's state, else its newest version, which they say. A tool keeps one version and its pinned one.

- `POST /v1/projects` `{path, create?}`: the project holding `path` (the nearest folder up from it with a
  project.yaml), or with `create` a new one made at `path` (`409 project_exists`). Answers `{id, path, next?}` and
  records it in the index as used now. `404 project_not_found` when no folder up from `path` is a project.
- `POST /v1/projects/{id}/imports` `{source, remove_background?, on?}`: `{path, sha256, prefix, line, already?, run?, note?}`. With `remove_background`, a picture not already transparent goes to `birefnet/remove_background` as a run of the project, the one place Calcifer names that package: `run` is it to follow, its output the one file kept, and the other fields empty. `note` says why not, and a plain copy stands. `already`: its bytes were in `files/`, so nothing was copied. `line` is what to paste: `image: ab12cd34ef56`, or `video:`, `audio:`. A URL: 64 MiB at most, public addresses only at every hop.
- `GET /v1/projects?limit=&before=`: the index, last used first: `{projects: [{id, name, path, used_at_ms,
  missing?, busy?}], older}`. `missing`: the folder no longer holds project.yaml.
- `DELETE /v1/projects/{id}`: `204`, forgotten from the index; the folder is untouched. `409 project_busy`
  while it renders.
- `GET /v1/projects/{id}`: with a project.yaml that doesn't read and a render going, what it owes in `queue`, what's wrong in `warnings`, and no assets; with none going, that file is refused. `{id, path, assets, queue?: [{name, running, run?}], entities: [{name, kind, description?, links, subs}], missing, stale: [{name, why}], orphans: [{name, kind, versions, bytes, renamed_to?}], busy, moving,
  absent, cuts: [{cut, path, fps, cut_at_ms?, frames?, retimed?, skips?}], warnings, next?}`. `next` is the commands to run given what it holds and lacks. Each asset is `{name, standing, making?, kind, noun, with?, versions, prefixes?,
  approved?, pinned?, inputs?, files, path?, made_by?, status}`: `standing` is `current`, `out_of_date`, `missing` or `making`, for a client that shows it, `making` the runs making it now, `approved` is the version a pin is, `pinned` that a hash in project.yaml is its state,
  `files` its state by output name, `path` its main file, and `status` is for people, in the asset's noun. An orphan is an asset project.lock keeps
  versions of that project.yaml no longer names; its `bytes` count what `reclaim` would free of it, each file once; `renamed_to` the one new asset of
  its kind with none. `absent` counts files the lock names that are not here. `warnings` holds what project.yaml's reading said, what each
  state's version says, and each asset whose known published package has no adapter for its kind.
- `GET /v1/projects/{id}/assets/{name}`: `{asset, versions: [{number, files, made_by?, seed?, runs, approved?, warnings?, kept?}]}`.
- `GET /v1/projects/{id}/yaml`: project.yaml's bytes, its `ETag` the digest of them (`If-None-Match` is `304`).
- `PUT /v1/projects/{id}/yaml` with `If-Match` (`400 if_match_required` without): the body becomes project.yaml, if it still holds what that ETag read, else `409 project_changed` and it stands. `204`, the new `ETag`. A body that doesn't read as a project is `400 project_yaml`, the file untouched. A client keeps comments and formatting by changing only the lines it means to. It renders nothing, though a render going keeps the text it was asked with.
- `POST /v1/projects/{id}/runs` `{names, n?, on, hub?, approve?, missing?, take?, in_s?}` (`Idempotency-Key`, kept in project.lock while the walk goes, so a retry adds nothing even across a restart): answers `{id, path, joined?, added, skipped?, queued?: [{name, ahead?}], next}`. Each name is a new version, and `all` one of every asset with an operation, in reading order, unless `missing` asks only for what has no state. `skipped`: pinned assets it was asked of by `all` or an entity's name, left as they are; naming one makes a version to look at, and its pin stays. `joined`, a render was going already; `added` 0 with it is no new work; `queued`, what it added, and the names ahead of each. It captures project.yaml as it is, kept in project.lock until it is done, and plans on that text alone; a file that doesn't read refuses it. Asks go at once. each name, or `all`, and what it reads: run
  its operation if it has no state, and nothing else. `new`, or `n` over 1, also draws `n` new seeds for those named; a pinned one's state doesn't move.
  `--take`, `--in`: one asset's new take keeps the start of that take, or the one in use (`400 take_unknown`); the adapter gets it as `keep`. `202 {id, path}`, or `200` when
  its key asked it already. The work goes on in the project's walk and is followed on its events.
- `POST /v1/projects/{id}/approve` `{name, version}`: `{name, noun, number, files, path, stale, warnings?}`. `version` is a short run of digits, its version, or anything else a hash prefix, which pins any file of the project; `number` is 0 when it is no version's. Writes the version's main file's hash into
  that asset's lines in project.yaml, editing that one line, re-read just before it writes, and checks what it wrote reads as the same project with that one change (`400 project_edit` otherwise, saying what to write by hand). A line holding an input keeps it, `video: <hash>  # was <clash>`.
  Makes any tool whose reads settled, with no machine.
- `POST /v1/projects/{id}/unpin` `{name}`: `{name, version?, stale}`. Removes that line, or restores the input it was over. A constant, nothing but its file, keeps it (`400 project_edit`).
- `POST /v1/projects/{id}/rename` `{from, to}`: `{from, to, versions}`. What project.lock keeps of `from` is kept as `to`, an asset project.yaml names `400 project_name` when `to` can't be a name.
  now with no versions (`409 rename_taken`, `409 project_busy` while it renders). Nothing on disk moves. What reads it goes out of date unless pinned.
- `POST /v1/projects/{id}/reclaim` `{yes?}`: `{orphans, files, bytes, deleted?, next?}`. Lists orphans and the files nothing else names; with `yes`, deletes them, an orphaned cut's film at the root too (`409 project_busy` while it renders). Never automatic.
- `POST /v1/projects/{id}/push` `{hub?}`: `202 {id, path}`, `401 not_logged_in` signed out. Sends every file
  project.lock names that the Hub lacks, then records them as a private commit of `<account>/project-assets`. A file over 5 GiB goes as 1 GiB parts,
  each its own object, which its first push lists in project.lock under the file's `parts`. `transferred` on the events ends it.
- `POST /v1/projects/{id}/pull` `{hub?}`: the same, fetching every file `files/` lacks, as real files, each held to its digest.
- `POST /v1/projects/{id}/cancel` `{names}` (or `["all"]`): `{dropped?, canceled?}`. Drops their queued work and cancels their runs; nothing dropped is sent again. It needs no file but to follow an entity's name, using the newest render's text if project.yaml doesn't read.
- `GET /v1/projects/{id}/events` (SSE): `started`, `landed`, `said`, `warning`, `failed`, `moved`, `transferred`, `cut`, `ended`. It opens with a `started` for each run already going, so one that comes late hears them at once; one that began just then can be told twice.
- `GET /v1/projects/{id}/files/{sha256}.{ext}`: a file (Range, ETag its sha256). `GET /v1/projects/{id}/cuts/{cut}`: a cut or its `.otio`, as last made.
- `GET /v1/projects/{id}/archive`: `application/x-tar`, streamed as it is read: every regular file in the project's folder on this machine as `<folder>/<path>`, symlinks left out. A file the lock names that isn't here (kept on a machine, or needing a pull) is named first in `<folder>/elsewhere.json`, `{elsewhere: [{sha256, machine, length?}]}`, and `Cozy-Elsewhere` counts them. A read that fails fails the stream, so a tar never ends cleanly and short.

### Tools and Calcifer
Calcifer fetches the tools it runs on first need, pinned builds held to their sha256, into `~/.cozy/tools`:
ffmpeg (cuts, frames, clips) and uv (local code). No route asks for them.

- `POST /v1/down`: `{stopping: true, owed}`. Admission closes, mutations in flight finish, every stream ends,
  then the server. Machines keep what they accepted; the next Calcifer resumes `owed`. Never refused.

## CLI verbs and their calls

| Verb | Calls |
|---|---|
| `cozy status` | `GET /v1/status`, `GET /v1/machines`, `GET /v1/runs?status=…` |
| `cozy web [--no-open]` | `GET /`, `POST /v1/session-codes` |
| `cozy project open [path] [--no-open]` | `POST /v1/projects {path}`, then as `cozy web` |
| `cozy project new <path>` | `POST /v1/projects {path, create: true}` |
| `cozy project ls` | `GET /v1/projects` |
| `cozy project render …` | `POST /v1/projects {path}`, `GET /v1/projects/{id}/events`, `POST /v1/projects/{id}/runs`; each `started` run followed on `GET /v1/runs/{id}/events` |
| `cozy project approve <node> <n>` | `POST /v1/projects {path}`, `POST /v1/projects/{id}/approve` |
| `cozy project push`, `pull` | `POST /v1/projects {path}`, `GET /v1/projects/{id}/events`, `POST /v1/projects/{id}/push` or `/pull`; exits 3 when `skipped` names a file |
| `cozy project status` | `POST /v1/projects {path}`, `GET /v1/projects/{id}` |
| `cozy project rename <old> <new>` | `POST /v1/projects {path}`, `POST /v1/projects/{id}/rename` |
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
| `cozy model family` | `PATCH /v1/models/{org}/{name}` |
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
