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
- a route Calcifer does not serve (`404 unknown_route`), an answer without fields the CLI reads, or, from a
  Calcifer of another version, a request body it cannot read (`400 body_invalid`, the CLI's newer shape): the CLI
  says so, restarts Calcifer on the installed cozy and asks once more. If the restarted Calcifer is the same version,
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
`Accept-Ranges: bytes` and an `ETag`, and a single `Range: bytes=…` with `206` and `Content-Range` (`416` when
unsatisfiable), so a `<video>` seeks it.

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
<!-- cl-716 A -->

### Projects
<!-- cl-716 A -->

### Tools and Calcifer
<!-- cl-716 A: tools, status, down -->

## CLI verbs and their calls

| Verb | Calls |
|---|---|
<!-- each PR adds its verbs' rows -->
