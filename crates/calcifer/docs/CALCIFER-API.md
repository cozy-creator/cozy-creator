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
  - `notes` are what binding said, in order (`info`: a clip cut; `warning`: fields dropped, a machine with no
    GPU). `preparing`: the run is local code's install; follow it, then send the same request under the same key.
  - Refusals name the field: `function_unnamed` (with the commands that name one), `callable_unknown`,
    `callable_recipe`, `request_payload_invalid`, `asset_media_type`, `input_segment_invalid`, ….
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
- `GET /v1/runs/{id}/events` (`Last-Event-ID` or `?starting_after=`): `run.*`, `output_item.*`, `run.log`,
  `run.call`, and the live `run.progress` and `run.waiting`. It ends after the terminal event once every output
  has landed here or failed to.
- `GET /v1/runs/events`: every run's change as `run.updated`, each run whole. No backlog: subscribe, then list.
- `POST /v1/runs/{id}/cancel`: `202`, or `200` when it had ended.
- `GET /v1/runs/{id}/outputs/{name}`, `GET /v1/runs/{id}/play`: an output's bytes; a link that plays it.
- Bridge, until package install and removal and model upload and download are Calcifer's (cl-716 D):
  `POST /v1/runs/prepared` and `POST /v1/run-quotes/prepared` take a `SubmitRun` whose spec the client built.

### Machines and rentals
<!-- cl-716 C -->

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
- `POST /v1/package-publications` `{hub, dir, root}`: stages, relocks for this Hub's account index,
  builds, carries its git dependencies' wheels, describes, uploads what the Hub lacks and finalizes;
  ends `published` (`status: already published` for a version the Hub holds).
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
(`edit.mp4`, with `resolution:` also `preview.mp4`). Its id is fixed by its folder. Every file the project uses
is in `assets/<sha256>.<ext>`, flat: what cozy makes as real files, and each file project.yaml names (relative,
absolute or `~/…`) taken in on first use, by the first method that works: a copy-on-write clone (reflink:
Btrfs, XFS, ZFS 2.2+, APFS), else a symlink to the original, else (Windows without the right) a hard link on
the same volume, else a copy, said once. project.lock records each file's path, size, sha256 and method under
the path written. A changed original is taken in again and what read it goes out of date; for a link that is
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
- `GET /v1/projects/{id}`: `{id, path, nodes, missing, stale: [{name, why}], orphans: [{name, kind, versions,
  renamed_to?}], busy, cuts: [{cut, path, cut_at_ms?, skips}], warnings}`. Each node is `{name, kind, noun,
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
  `landed {name, noun, number, assets, path}`, `said {message}`, `warning {message}`, `cut {cut, seconds}` and
  `ended {error?}` when the walk owes nothing more. Not resumable: a client reads `GET /v1/projects/{id}`
  after reconnecting.
- `GET|HEAD /v1/projects/{id}/assets/{sha256}.{ext}`: the asset (Files). Its ETag is its sha256.
- `GET|HEAD /v1/projects/{id}/cuts/{cut}` (`edit.mp4`, `preview.mp4`): the cut as last made (Files), so a
  `<video>` seeks it; `404 file_missing` before the first cut. Its encoded edge pieces are kept in cozy's
  cache folder (`$XDG_CACHE_HOME/cozy/cuts`, `~/Library/Caches/cozy/cuts`, `%LOCALAPPDATA%\cozy\cache\cuts`) by
  take, span and encoding, unused ones pruned after 14 days and the oldest past 10 GiB.

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
