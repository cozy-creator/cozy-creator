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
A run is asked for unbound (`RunRequest`); Calcifer resolves its target, binds it against the function's
interface, cuts and stages its files and builds the machine's spec (`crates/calcifer/src/runs`). No client builds
a spec.

- `POST /v1/runs` (`Idempotency-Key`): a `RunRequest`. Answers `202` (`200` when the key had made it) with
  `Submitted {run, replay, notes, preparing}`:
  - `target`: `[org/]pkg/fn[@release]` (no `@`: the newest release this computer keeps, refreshed from its Hub;
    `x/y` is root package x's function y, else account x's package y), or local code `./dir[/fn]`,
    `./script.py[/fn]`.
  - `on` (`local`, a machine, `any`, `new[:sku]`), `hub`, `gpus`, `lane`, `attention_kernel`, `out`, `warm`
    (a level, instead of running), `agreed` (a `new` run's quote).
  - The payload: `input` (an object) or `input_file` (a JSON or YAML file here; files it names are relative to
    it); then `terms` override it (`key=value` typed as declared, `key:=<json>`, `model.<slot>=<ref>`, one
    positional value, input files), `loras` (`[slot=]ref[,strength]`), `files` (`{name?, file}`: an input's
    field path, a label, or none for the declared files) and `fidelities`. A file is a path here (a video's may
    end `#t=<start>,<end>`, cut once into `~/.cozy/cache/clips`), a directory at a tree input, or an uploaded
    `sha256:<hex>`. Relative paths start at `cwd`; without one they are refused.
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
- `GET /v1/targets?target=&cwd=&hub=`: what a run of `target` calls, resolved as `POST /v1/runs` resolves it:
  `{package, function?, release, release_from: asked|latest|local, summary, interface}` (the package's interface
  document; local code's read from its source, installing nothing). A named function is checked as a run checks
  it.
- `PUT /v1/files/sha256:<hex>`: a file's bytes, which must hash to its name: `201`, or `200` when held, with
  `{digest, length, media_type}`. `HEAD` asks whether it is held. A held file unnamed for a day is dropped.
- `GET /v1/runs` (`status`, `package`, `machine`, `before`, `limit`): `{runs, older, machine?, unreadable?}`.
- `GET /v1/runs/{id}`: the run. While it waits, `queue_position` (1 is next); while it runs, `progress` with
  `done_fraction`, `bytes_per_s` and `eta_s`. `?detail=full` adds `measurements`, `calls` and `report` (each
  executor's stages and unaccounted time, models, kernels, and each GPU's memory, with rates against its
  ceiling).
  A run settled over 30 days ago has dropped `measurements` and each call's, so its `report` is gone. Nothing else
  of a run is ever dropped: its row, `result`, reason, outputs, logs and `calls` stay for good.
- `GET /v1/runs/{id}/events` (`Last-Event-ID` or `?starting_after=`): `run.*`, `output_item.*`, `run.log`,
  `run.call`, and the live `run.progress` and `run.waiting`. It ends after the terminal event once every output
  has landed here or failed to.
- `GET /v1/runs/events`: every run's change as `run.updated`, each run whole. No backlog: subscribe, then list.
- `POST /v1/runs/{id}/cancel`: `202`, or `200` when it had ended.
- `GET /v1/runs/{id}/outputs/{name}`, `GET /v1/runs/{id}/play`: an output's bytes; a link that plays it.

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
- `GET /v1/machines/events`: `machine.updated {machine}` each time a machine changes, and `machine.removed
  {name}`. There is no backlog: subscribe, then list.
- `GET /v1/machines/{name}[?fresh=true]`: `Machine`. `fresh` reads its Status now, or refuses
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
- `DELETE /v1/machines/{name}[?force=true]`: `Ended {machine, changed, abandoned}`, `202` while `ending`.
  - The end is recorded at once and survives a restart. Calcifer carries it out once the outputs of the
    machine's runs have landed here, and `machine.ending` says what it waits for.
  - `force` ends it now; `abandoned` names the outputs given up. Asked again with `force`, it stops the wait.
  - A rental is `ending` until its Hub affirms the pod gone, then `ended`. An added machine is removed.
  - An ending machine takes no new runs (`409 machine_ending`), and `local` is refused.
  - An order its Hub has not answered stops at once (`ending`, no `rental_id`): it buys nothing more, and leaves
    the list once its Hub answers and anything it placed is ending. So does one whose run ended.
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
- `GET /v1/machines/{name}/logs/{tensord|tensorfs|memory}?lines=N&since_ms=T`:
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
- `POST /v1/models/{org}/{name}/commits` `{hub, from, lane, mirror, trigger, strength}`:
  `RecordedCommit`. Commit `from` (`org/name@sha256:…`) of another model, recorded as one of this
  model when the caller may read its whole closure: nothing is uploaded. Its lane is checked before
  the mirror is recorded.

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
  `local/` alias, an absolute path to a file or a CozyTensors folder, or `run:N[#output]`, which runs
  on the machine that holds the output. Its trigger and strength (the flags, else the source's own)
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
A project is a folder holding `project.yaml` (people's), `project.lock` (Calcifer's), `assets/`, and its cuts
(`edit.mp4`, with `resolution:` also `preview.mp4`), each with its timeline beside it (`edit.otio`). Its id is
fixed by its folder. Every file the project uses
is a real file in `assets/<sha256>.<ext>`, flat: what cozy makes, and what `POST /v1/projects/{id}/imports` brought in.
project.lock records each import under its sha256 (`{path, sha256, video?, mark?, clean?}`), and with every video asset
its shape, probed once: `{width, height, fps, frames}`. It names nothing of this computer, so it travels with project.yaml.
A file field (an entity's `image`, `voice`, a setup's or world state's `image`, an asset's `image`, `video`, `audio`,
a shot's `plate.video`, `first_frame`, `last_frame`, `video`) holds a hex prefix of an asset's hash: any file project.lock
names, imported or made. A value that is no prefix (a path, a URL), one that fits no asset, and one that fits several
each give a warning, and that node renders without the file. An asset project.lock names that is not on this computer
holds what reads it (`not on this computer: cozy project pull`). A marked import's hash resolves to its clean once made;
until then what reads it waits, and the walk that owes one of those readers sends the clean to its machines first. A
failed clean stops only what reads it, and the next walk tries again. Calcifer deletes from `assets/` only what
project.lock no longer names; an import is never dropped.

A shot's `summary` and `beats` are opaque text but for their `<mentions>`. Calcifer sends a node to its package's project
adapter (`project-adapter-<kind>`, on the render machine), and runs the function and input it answers, recording them on the
take as `invocations`; a file in them is its digest. A function with no adapter gets one flat `prompt`, with a warning.

```yaml
version: 1
shots:
  - name: fight
    summary: <theo> and <juno> circle each other on <dock>.
    beats:
      - duration: 5
        description: They circle.
      - duration: 15
        description: <juno> lunges.
```

A shot is as long as its beats. An adapter's record gives it as `duration_s`, and each beat's start, the sum of the
durations before it, as `at`.

A made version is out of date when what the author wrote that reaches it changed since it was made or approved. project.lock
keeps that as `text`: by field, a digest of the words, trimmed. The fields are a shot's `summary`, `beats` (each `duration` and
`description`), `soundscape`, `music` and `style`, and `<name>` for each description of what it reads; an entity's `description`,
its own `render: instructions:` and `style`; a setup's `camera`, its environment's `description` and `style`; a world state's `init`, `blocking` and `style`. Only
fields both sides have are compared and a field's meaning never changes, so a cozy update never puts anything out of date. A
version with no `text`, made before this, has what it says now recorded the first time it is read. A file in a node's own
`image` or `video` pins it: that is what it reads, whatever is approved, and nothing is made of it. Otherwise it reads as what
`approve` or `--approve` chose, else its newest, which is the only way approval is written. A cut holding what it held is not
made again (`cuts` in project.lock). `approve` records the text
and files as they stand, and `rename` the text of what reads the node, since it rewrites the mentions.

A shot's and an upscale's alternatives are takes, every other node's versions: records carry one generic
field (`versions`, `version`) and a `noun`. Each is numbered (1 first, never reused) and is a set of assets
(`{path, sha256}`).

- `POST /v1/projects` `{path, create?}`: the project holding `path` (the nearest folder up from it with a
  project.yaml), or with `create` a new one made at `path` (`409 project_exists`). Answers `{id, path}` and
  records it in the index as used now. `404 project_not_found` when no folder up from `path` is a project.
- `POST /v1/projects/{id}/imports` `{source, clean?, hub}`: `{path, sha256, prefix, line, already?, clean, warnings?}`. `already`: its bytes were in `assets/`, so nothing was copied. Keeps the
  file at `source` (an absolute path, or an `http(s)` URL) as a real asset, its type read from its bytes; `line` is what to
  paste into project.yaml (`image: ab12cd34ef56`, or `video:`, `audio:`). A URL: 64 MiB at most, public addresses only at
  every redirect. `400 import_failed` says why not. `clean` marks a picture (a later import of it says again): `clean` answers `{state:
  "untouched"}`, `"removed"` (an earlier clean stands, and no run was sent), `"removing", run` (a run sent to this computer's machine; follow it, then `cleaned` on the events)
  or `"later"` (this computer runs no models, or the run could not be sent: the first machine to render a reader cleans it).
- `GET /v1/projects?limit=&before=`: the index, last used first: `{projects: [{id, name, path, used_at_ms,
  missing?, busy?}], older}`. `missing`: the folder no longer holds project.yaml.
- `DELETE /v1/projects/{id}`: `204`, forgotten from the index; the folder is untouched. `409 project_busy`
  while it renders.
- `GET /v1/projects/{id}`: `{id, path, fps, nodes, missing, stale: [{name, why}], orphans: [{name, kind, versions,
  renamed_to?}], busy, cuts: [{cut, path, cut_at_ms?, frames?, retimed?, skips}], warnings}`. `fps` is the
  edit's frame rate; a cut's `frames` its length at it and `retimed` the takes in it at another rate
  (`<node> (<fps> fps)`), both from the shapes project.lock records. Each node is `{name, kind, noun,
  versions, prefixes?, approved?, assets, path?, made_by?, status}`: `versions` counts them, `prefixes` gives each one's `{number, prefix}` for project.yaml, `approved` is the number of the one chosen by `approve` or `--approve`, `pinned` says a file in project.yaml is its result, `assets` what the node reads as now by output name,
  `path` its primary file, and `status` is for people, in the node's noun. An orphan is a node project.lock
  keeps versions of that project.yaml no longer names; `renamed_to` the one new node of its kind with none. A
  file under `assets:` is a node of kind `asset`: shots read it by mention, and it has no versions.
  `warnings` holds what project.yaml's reading said and what each node's current version says: its function's
  warnings and what was left out of its request. A cut past a take's end is left out and said here.
- `GET /v1/projects/{id}/nodes/{name}`: `{node, versions: [{number, assets, made_by?, runs, approved?, warnings?}]}`.
- `POST /v1/projects/{id}/runs` `{names, n?, on, hub, preview?, approve?, new?}` (`Idempotency-Key`): each name, or
  `names: ["all"]`, brought up to date with what it reads: what is missing or out of date gets `n` new takes or versions, any
  approval it had goes, and `all` ends with the cuts. A current name is left, with a `said` event; a pinned one too. With `new`,
  what is current gets them as well, its approval kept, and what a named node reads with no version yet is made first.
  `approve` approves the new one when it lands (`400 approve_one` with `n` > 1). A name that is no node, or a reference, is warned of and the rest go on; only an ask with
  nothing left is refused (`node_unknown`, `400 asset_file`). `202 {id, path}`; the
  work goes on in the project's walk and is followed on its events.
- `POST /v1/projects/{id}/rename` `{from, to}`: `{from, to, versions}`. What project.lock keeps of `from`
  (versions, approval, what other versions read of it, what is owed) is kept as `to`, a node project.yaml names
  now, of the same kind and with no versions (`409 rename_taken`, `400 rename_kind`, `409 project_busy` while it
  renders). Nothing on disk moves.
- `POST /v1/projects/{id}/push` `{hub?}`: `202 {id, path}`, `401 not_logged_in` signed out. Sends every file
  project.lock names that the Hub lacks, by content (`/blobs/…`: what it holds never moves; a killed push sends only the
  files that had not landed), reading through links, and records them all as a private commit of the account's model
  `project-assets`, which keeps them until its owner deletes it (`cozy model delete`). A file not on this computer
  needs only be at the Hub. A file over 5 GiB, or on neither, is left out and named in `skipped`; everything else goes.
- `POST /v1/projects/{id}/pull` `{hub?}`: `202 {id, path}`, `401 not_logged_in` signed out. Fetches every file
  project.lock names that `assets/` lacks, as a real file: a killed pull goes on from the byte it reached
  (`assets/.<name>.part`), and each is held to its sha256 before it is named. A file not at the Hub is left out and named
  in `skipped`; everything else comes. Both say how they go on the events
  (`moved`, `transferred`) and go on when their client goes; one project moves one at a time.
- `POST /v1/projects/{id}/approve` `{name, version}` (`version` its number): `{name, noun, number, assets, path,
  stale, warnings}`. `path` is its primary asset; `stale` names what this put out of date; the cuts are made
  again when a shot's take changed.
- `GET /v1/projects/{id}/events`: `started {name, run, number}` (follow the run on `GET /v1/runs/{id}/events`),
  `landed {name, noun, number, assets, path, execution_ms?}`, `said {message}`, `warning {message}`, `failed {error}` (one version failed, said once; the walk goes on), `cut {cut, frames, fps, retimed?, took_s, unchanged?}` and
  `moved {file?, bytes}` (a push or pull landed `file`, or is under way), `transferred {done?: {files, moved, bytes,
  skipped?: [{path, why}], kept_in?, commit?}, error?}` when it ends, `cleaned {sha256, error?}` when the run cleaning an import ended, `ended {error?, rendered?: {takes, versions, elapsed_ms, execution_ms}}` (the first failure) when the walk owes nothing more. Not resumable: a client reads `GET /v1/projects/{id}`
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
| `cozy project push`, `pull` | `POST /v1/projects {path}`, `GET /v1/projects/{id}/events`, `POST /v1/projects/{id}/push` or `/pull`; exits 3 when `skipped` names a file |
| `cozy project status` | `POST /v1/projects {path}`, `GET /v1/projects/{id}` |
| `cozy project rename <old> <new>` | `POST /v1/projects {path}`, `POST /v1/projects/{id}/rename` |
| any verb, Calcifer of another version | `POST /v1/down`, then the installed cozy's Calcifer started |
| `cozy run <target> [inputs] [flags]` | `POST /v1/run-quotes` (`--on new`), `POST /v1/runs` (again after a `preparing` install), then `GET /v1/runs/{id}/events` |
| `cozy run <target> --describe`, `cozy run <package>` | `GET /v1/targets` |
| `cozy run ls` | `GET /v1/runs`; live: `GET /v1/runs/events` |
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
| `cozy model ls --on` | `GET /v1/machines` |
| `cozy model info` | `GET /v1/models/{org}/{name}` |
| `cozy model family` | `PATCH /v1/models/{org}/{name}` |
| `cozy model publish` | `POST /v1/models/{org}/{name}/releases` |
| `cozy model delete` | `DELETE /v1/models/{org}/{name}[/releases/{v}\|/commits/{c}]` |
| `cozy model download` | `POST /v1/model-downloads` (`/v1/model-download-quotes` first for `new`); `GET /v1/runs/{id}/events` |
| `cozy model upload` | `POST /v1/model-uploads` (`/v1/model-upload-quotes` first for `new`); `GET /v1/runs/{id}/events`. A Hub commit: `POST /v1/models/{org}/{name}/commits` |
| `cozy model quantize` | `POST /v1/model-quantizations` (`/v1/model-quantization-quotes` first for `new`), again while `preparing`; `GET /v1/runs/{id}/events` |
| `cozy package ls [query]` | `GET /v1/packages` |
| `cozy package ls --on` | `GET /v1/machines`, `GET /v1/machines/{name}?fresh=true` |
| `cozy package info` | `GET /v1/packages/{org}/{name}` |
| `cozy package install` | `POST /v1/package-installs` (`/v1/package-install-quotes` first for `new`); `GET /v1/runs/{id}/events` |
| `cozy package remove` | `POST /v1/package-removals`; `GET /v1/runs/{id}/events` for each |
| `cozy package yank [--undo]` | `POST` (`DELETE`) `/v1/packages/{org}/{name}/releases/{release}/yank` |
| `cozy package delete` | `DELETE /v1/packages/{org}/{name}` |
| `cozy package lock` | `POST /v1/package-locks`, `GET /v1/package-locks/{id}/events`, `GET /v1/package-locks/{id}` |
| `cozy package publish` | `POST /v1/package-publications`, `GET /v1/package-publications/{id}/events`, `GET /v1/package-publications/{id}` |
<!-- each PR adds its verbs' rows -->
| `machine ls` | `GET /v1/machines?refresh=true`; on a terminal, `GET /v1/machines/events`, then `GET /v1/machines` |
| `machine ls --history [--since D]` | `GET /v1/rentals?since_ms=` |
| `machine show <name>` | `GET /v1/machines/{name}`; live on a terminal, as `ls` |
| `machine logs <name> [log] [--since D] [-f]` | `GET /v1/machines/{name}/logs/{log}?lines=&since_ms=` (SSE with `-f`). `logs local calcifer` reads Calcifer's log file, so it works while Calcifer is down |
| `machine rent` | `GET /v1/rental-skus` |
| `machine rent <sku>` | `POST /v1/rental-quotes`, `POST /v1/machines` (rent), then `GET /v1/machines/events` until ready or ended |
| `machine end <name> [--force]` | `DELETE /v1/machines/{name}`, then `GET /v1/machines/events` until ended |
| `machine keepalive <name>` | `POST /v1/machines/{name}/keepalive` |
| `machine update <name> [--runtime\|--tensorfs\|--tensord V\|PATH]` | `POST /v1/machines/{name}/update`, then the run's events. A project directory is built here first |
| `machine add <name> <address>` | `POST /v1/machines` (add) |
| `machine authorize <key> [--remove]` | `PUT` or `DELETE /v1/machines/local/authorized-keys/{key}` |
| `machine ssh <name>` | `GET /v1/machines/{name}` for its address, then `ssh` here |
| `update` (moving `local` to the new cozy) | `POST /v1/machines/local/update` with `{"tensord": {"version": V}}` |
