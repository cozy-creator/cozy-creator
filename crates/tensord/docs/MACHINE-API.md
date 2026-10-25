# Machine API

Every client of a machine uses it: Calcifer and the CLI, the Hub (readiness), the browser player
and a run's own child runs. A laptop's machine and a rental's are the same.

**HTTP + JSON** on the machine's one pinned-TLS listener (`api/server.rs`, `api/http.rs`):
control and SSE over HTTP/2, bulk bytes over HTTP/1.1. The routes, bodies and events are
`crates/tensord/api/openapi.yaml`, written by hand and changed in the same PR as the server
(`scripts/machine-routes.sh` holds the router and the client to it); how each former gRPC call maps
onto them is `~/cozy_v2/tracker/design/machine-http-api.md` §2. In short:

- `GET /v1/status` (identity and readiness receipt without a cap; the whole picture, as SSE on
  change, with one), `POST /v1/keepalive`, `PUT /v1/capability` (the signer's newest run capability at a Hub);
- `POST /v1/runs` (idempotent on id per signer), `GET /v1/runs/{id}/events` (SSE after
  `Last-Event-ID`; closing it never cancels), `POST /v1/runs/{id}/cancel`;
- `GET /v1/runs/{id}/outputs/{output}` (ranged, `ETag` = revision, readable while it grows),
  `GET /v1/runs/{id}/triage`, `GET /v1/logs/{name}`;
- `PUT`/`HEAD /v1/objects/sha256:<hex>`: content-addressed parts in any order, side by side,
  resumable from `Cozy-Held`; verified once whole.

**One browser channel**: WebRTC `cozy/1` (ICE-lite, passive ICE-TCP, DTLS pinned to the leaf,
the cap in its hello) serves a run's output log and bytes, live or finished. **One credential**:
`Cozy-Cap` in `authorization`, an authorized key's Ed25519 grant naming the machine, a scope
(`machine` = every call; or `run:<id>`, optionally limited to some outputs) and an expiry.
Refusals are `{"error":{kind, code, message}}`; a run's outcome carries the same typed reasons.
Unknown fields are ignored.

`RunSpec.gpus` is optional (`run-gpus/1`). Zero uses the widest GPU group supported by
all package-code model slots on the machine. A positive value requests exactly that many
GPUs; unsupported operations fail instead of reducing the count. This includes GPU child
calls of a deviceless root, whose explicit count reserves one subset while it runs. Checkpoint identity and weight
lanes never select parallelism. Busy GPUs cause queuing, not a smaller group.

`RunSpec.lane` and `WarmItem.lane` name the main model's lane (a label); children inherit it. Empty: the
Hub resolves each slot's commit for its GPU group's compute capabilities and free VRAM (plain lanes only for
a slot whose class takes no encoded leaves, or raw weights), and says why it passed a better lane over (one run
warning each). A model-only upload's lane labels the commit it makes.


## What disappears

- **Prepare and warm-up** are runs (`kind: warm`); uninstall is `kind: remove` (`remove/1`).
- **Acknowledge** is deleted. A read that reaches the final length of an output's final revision
  marks it delivered, and delivered bytes are evicted first.
- **Hub access**: released content is read anonymously. A run that names private commits by hash
  or publishes carries a run capability (`tensorfs.token`, with the Hub's `endpoint`, `ca_der`
  and `expires_unix_ms`), never the account's sign-in: see
  [What a machine and a package can reach](#what-a-machine-and-a-package-can-reach). The machine
  keeps the newest per signer and Hub, in memory only, and reads models through TensorFS's
  distribution `Endpoint` under it: runs, warm-set members, prefetches and calls alike. Each
  executor given it on a Load or Invoke hears of every newer one (`capability`, see
  [DEVICE-EXECUTOR.md](DEVICE-EXECUTOR.md)), so a call that outlives one still reads and
  uploads. It presents it only to that Hub's origin. A refused capability ends the run
  `hub_credential_refused`, naming its expiry. If the machine restarts before
  preparation completes, the run ends FAILED with a typed reason; started work is never re-run.
- **Names** (th-245): the machine resolves them at the run's Hub. A release (`org/name`, its
  release or none for the newest) installs from the card and locked requirements it reads there
  by name. A model slot is the run's choice, else its declared `default` release (a callee's
  too): `repository` at `release` (none: the newest) in its `lane`, else the commit the Hub picks for
  the hardware (`GET /v1/models/{org}/{name}/resolve`); the commit's tree names its one header.
  `manifest` names a commit (or a header) by hash: one the capability reads, or one the machine
  holds. A weights output the Runtime writes, the machine uploads itself as a commit of the run's
  `weights_destination` in the output's lane. Name
  resolutions and held preparations are kept per `catalog_revision`, so a warm run reads nothing
  at a Hub and a revision the caller moved resolves once more; there is no TTL guess or forget verb.
- **List** is deleted. Status shows live runs; history is the CLI's own records plus a run's events.
- **Update** is a run of `kind: update`. It stages and verifies a software cohort, activates it at
  measured idle and keeps the known-good install for rollback. The rental keeps its downloaded
  weights; progress and outcome come with the run. The payload is `{runtime, tensorfs}`: a
  published version, or a wheel file name whose bytes an input binds to an uploaded object. A
  client that loses the stream across the restart follows the events again after its cursor.
  A pair the machine already runs succeeds unchanged at once. An update submitted while another
  runs is queued (`queued`), never refused: it starts when that one ends, after its restart too.
  Wheels stage at any time. Cancel ends an update before it
  activates (`canceled`, candidate removed, admission reopened); once activating it is refused
  `cancel_unavailable`. Published wheels land through the store as package files do: a download
  that stops moving fails on its measured stillness.
- **Memo, derived retention, forget and prune** become one self-managing cache: TTL plus low-disk
  eviction, delivered entries first, and no verbs.
- **Memo** is the machine's own: a memoized function's success, run directly or as a call, is
  held per signer and answers the same computation (its code, environment with its Runtime build,
  request with files by content, attention choice and models) while the machine holds every file it
  names, so a Runtime update runs it again; the answered run starts
  nothing and its Outcome says `memoized`. An author's `memo_requires` names parameters a request
  must give to be memoizable; one that omits them runs, is not held, and says why (`unmemoized`).
  The client carries no results; a memo answers where its results are (measured 2026-10-06: no
  cross-machine hit ever in the owner's runs).
- **Jobs** are ordinary calls: any function may write weights outputs, and with a weights
  destination (the requester's repository) the machine pushes them to the Hub and settles the
  publication itself. A writer is never memoized.
- **Child runs** are submitted and canceled by a run through its executor seam, scoped to the
  parent: same records, no number, canceled with the parent, outputs granted to the parent's
  spool. Child outputs reach the parent by the seam, not by output reads.

**Pause/resume** are deleted (Paul, 2026-10-14). A canceled or failed run is submitted again; its
finished memoized calls are answered from the memo (`DURABLE-EXECUTION.md`).

**Activity** (a rental's idle release, `rental-lifetime.md`) is a run queued or running (warm-ups
included), and an explicit keepalive restarts the clock once. Uploads, updates, open streams and
other calls are not activity: a Calcifer that holds a stream must not bill forever.

## What a machine and a package can reach

A machine (a rented pod or this computer's own) never holds the account's sign-in (cl-670). At
the run's Hub it holds, per signer, one **run capability**: a grant the Hub signs at
`POST /v1/capabilities` for the computer that sent the runs, which keeps its sign-in.

- **Reads**: the closures of the commits this computer's live runs on that machine name by hash,
  where the account may read them (its own, or released). Public releases need no capability.
- **Writes**: the repositories those runs publish into (their `weights_destination`): blob
  uploads and `POST /v1/models/{org}/{name}/commits` there, nothing else.
- **Refused**: everything else: other commits and repositories, publishing a release, deleting,
  packages, rentals, billing, minting capabilities. The Hub answers 403
  `capability.operation_forbidden`.
- **Lifetime**: 15 minutes. Calcifer mints the machine a newer one at each send that needs more
  and five minutes before expiry while those runs are queued or running (`PUT /v1/capability`),
  and stops when they end, so it lapses at most 15 minutes after the last.

A package's code runs in an executor, which TensorD gives the same capability for its endpoint
(on a Load or Invoke, and each newer one): code can read it, and reaches what the machine's runs
already reach, no more. A rental's provider holds what the machine holds.
