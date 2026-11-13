# Hosted project containment qualification

The web shard also runs one fixed-public CPU request on a fresh Unix user's real
default-home worker. It binds that worker to loopback before authorizing the public
controller key, pins its TLS fingerprint, and explicitly selects it with `--on`.
Origin-project input bytes, output bytes and canonical run/log rows are checked
again after the exact remote processes retire. The remote invocation must contain
neither local private storage nor the origin mount path/proof token. Its ordinary
external working copy is preserved. All inspection output stays in RAM; no scan
exclusion, alternate Cozy home, rental, raw log or payload artifact is added.

This fixture is prepared for an isolated Linux hosted runner. Do not dispatch it until the
coordinator supplies the composed candidate and explicitly marks it ready. No part of this
fixture launches a browser or reads a real user's home on the development workstation.

The fixture uses the installed candidate CLI, its real Calcifer, the real encrypted provider,
and the actual `OwnedBrowser` Chrome process. Existing `page.py` route shapes are reused with
protocol-3 preflight. Existing browser proofs drive an independent Playwright browser; they
cannot establish this ownership contract.

## Access rule under test

There are no browser sessions (design/calcifer-access.md). Every request needs a loopback
peer, a `Host` of the listener and, if sent, an `Origin` of that Host. `/v1`, media and the dev
page also need the peer's Unix account to be Calcifer's; then `Sec-Fetch-Site: same-origin`
(the page), `none` on a GET navigation, or, from a non-browser, the lock record's bearer.
Nothing is injected into the owned page; its fetches, `<img>` and `<video>` carry no credential.

## Approved private observation seam

The fixture starts the installed `cozy calcifer` itself with one inherited Unix socket at
reserved fd 5. Calcifer may consume it only in the hosted qualification mode. No new HTTP,
TCP debugging port, user-profile attachment, arbitrary script evaluator or redeemable URL is
introduced. The fixture proves that socket's peer is its exact child process birth. A queued fixed domain
prelude is peeked before consuming the stream; an unrelated inherited protocol is left alone.

A closed request enum addresses only the already owned browser: `Snapshot`,
`Exercise { project_ref, action, payload }`, and `CloseOwnedWindows`. Exercise actions are fixed
code from this fixture, evaluated through the actor's existing private CDP pipe. Actions cover
page readiness, actual DOM edit, media fetch/status, storage observation, close, and a fixed
late-callback observation. There is no caller-selected JavaScript or CDP method. Snapshot's
process/profile identities stay in the fixture's RAM; retained receipts contain their hashes
and booleans only. Canary payloads and the bearer travel only in this private socket, stdin or
HTTP headers, never argv/environment. The production authority rules remain in force.

If the seam is unavailable, the fixture must report unavailable qualification;
it must not substitute a second browser and call that an OwnedBrowser pass.

## Required real execution

1. Refuse outside a fresh hosted Linux test-Hub runner; pin and hash the installed candidate.
   Open two born-encrypted project folders with credentials supplied on stdin. Capture CLI
   output in RAM. Write source, names, prompts, image metadata and all fixture-owned private
   outputs only inside their unlocked project views.
2. Open A and B through actual `cozy open`; verify the same owned Chrome birth and distinct
   live pages. Start separate fixture-owned normal and unrelated Incognito Chrome processes
   with no project data; prove exact births survive every Cozy retirement action.
3. Exercise real project reads, root content, thumbnails and media ranges from the owned page.
   Assert no-store on actual 200, 206, conditional requests and refused paths. A conditional
   request returning 200 rather than 304 is valid if it is explicitly no-store; it is not a
   fabricated 304 pass. Prove the native CLI's and the credential-free owned page's capability
   both name `user:<euid>` and this Calcifer's birth. On a project route and a media route,
   refuse another Unix account even with the bearer (`other_user`, containment-web's fresh
   account), `Sec-Fetch-Site` cross-site/same-site/non-navigation `none` (`not_own_page`), a
   foreign Origin (`origin_not_allowed`) and no bearer or browser headers (`unauthenticated`).
4. Hold a real SQLite write transaction in B, enqueue a real browser edit, and request A's lock
   through the CLI. Observe busy/pending work without discarding it. Release the transaction;
   verify B's exact value from its own project DB, A locked, all owned pages/processes retired,
   and unrelated fixture browser births still live. Late reads/preloads must not resurrect A.
5. Reopen, close all owned windows, crash only the exact owned browser, and cold-restart only
   the fixture's Calcifer. Observe the old bearer refused and a fresh owned browser. Verify
   process/profile retirement or retain an explicit incomplete-retirement failure.
6. Run read-only scans of the exact home, global DB+WAL/SHM, service/log captures, configured
   temporary/cache locations and browser profile. Scan raw bytes, filenames and SQLite logical
   values; decode supported gzip/zlib bodies. While the owned browser lives, only the verified tmpfs
   browser lease is an additional volatile allowance. After retirement it must be absent.
   After locking A, its former mounted view is not an exclusion. B's still-unlocked view is
   excluded only by its exact provider root and is scanned after B locks.
7. Save one deliberate export through the browser's actual download prompt to a selected
   external destination. Record its hash separately; never exempt it from the automatic-copy
   scan without naming that deliberate export. Bookmarks inside the owned volatile profile
   retire with the profile; this differs from a normal Chrome profile's Incognito behavior.

On an unexpected CLI exit the receipt keeps only allowlisted code/kind/cause booleans,
exit status and hashes. Real Chrome startup stderr goes to an anonymous RAM memfd;
only fixed startup-literal flags and a bounded hash can leave it. The memfd is sealed
against further writes before the app page navigates.
No raw CLI output, Chrome stderr, path, project reference or credential is evidence.

Receipts retain fixed case names, checks, counts, hashes, status codes and timing boundaries.
They never retain raw response/error bodies, SQL values, canaries, project-derived filenames,
HARs, screenshots or browser console output. On failure, write sanitized counts/hashes and
leave the owned synthetic project folders intact for coordinator-owned investigation. Never
collect mixed raw daemon logs into the uploaded output directory.

## Runtime entry and artifact limits

`uv run --no-project e2e/cli/containment/run.py --ready-file READY.json --out OUT` refuses
unless this is a hosted Linux runner, its isolated Hub is independently verified, a real display
is present, the default installed Calcifer is stopped, and the coordinator's ready file contains
`{"ready":true,"sha256":"<exact installed candidate SHA-256>"}`. That ready file is supplied by
the coordinator after composition, never generated by this fixture. No workflow dispatch or
local browser run is part of preparing these sources.

The scanner reads every regular file and filename under the declared roots. Files up to
64 MiB additionally receive SQLite-in-RAM, ZIP, gzip/zlib, LevelDB/Snappy and image decoding;
larger files are counted as `large_raw_only`. The receipt preserves that limit explicitly.
The fixed random black/white tile pattern recognizes a re-encoded private image/thumbnail
without requiring the original PNG metadata to survive. Missing decoders, unreadable files,
or undecodable recognized formats fail the case. No screenshot, HAR, tracing archive, raw
process output or copied log directory is retained. The final receipt is the only generated
outside evidence file and contains fixed case codes plus counts, hashes and statuses.

The current sequence covers native capability, two encrypted projects, browser root/media/
thumbnail requests, ranges, conditional requests, access-rule refusals, empty durable browser
stores, a pending B edit while A locks, whole-browser retirement, unrelated browser survival,
last-window closure, exact-browser crash, Caf SIGKILL with live pages followed by startup
retirement of the exact owned process/profile receipt, and cold Calcifer restart. An actual native Save-As
prompt/export journey and a managed-service capture/panic journey require their own explicit
cases before those boundaries are marked qualified; this sequence does not substitute a CDP
download override or a fake panic and call either one complete.

## Actual managed execution storage

`native_execution: true` invokes a real weightless function through the installed CLI,
worker and Runtime. It verifies project-owned source, environment, HOME, cache, temporary
files, output bytes and history retained across lock and cold restart. It checks Cozy's
requested core limit is zero; it never changes the host's kernel core handler.

This fixture does not claim an OS sandbox. Deliberate external writes made by custom
Python and exports are outside the automatic-storage guarantee. Older failed sandbox
receipts remain failures and are not relabeled as managed-storage passes.

## Optional real development/HMR case

`development_web: true` in the coordinator's ready file additionally requires a hosted runner
with pnpm and the checkout's web dependencies installed. The fixture owns one Vite process,
sets only top-level `dev = true` and `[calcifer].development_web` before its foreground Calcifer starts,
and creates one independent source-only HMR probe. Through the same actual owned browser it
loads that module, changes the file, and requires value 2 without a document replacement. The
ordinary project reads/edits/lock/restart cases then run through that same Calcifer origin.
Probe source and config are restored by exact identity/content checks; no laptop configuration,
ordinary browser or raw browser trace is used. Vite itself answers every `/v1` path with 404
`development_api_in_calcifer`.

## Old-first published legacy subject

`legacy_seed.publish_old(old_cli, work)` publishes the static `legacy-package` directory with
the installed released 0.24.0 CLI before private canaries. Its pinned target is
`e2e-containment-legacy/echo@0.1.0`; request fields are `source: File`, `text: str`, and optional
`fail: bool`. It prints the text, returns a text file containing source bytes + newline + text,
and raises with that text for the intentional failure case. The package embeds no canary.

Bootstrap the outer owned test-Hub runner with the OLD CLI, pass the candidate separately to
the legacy command, and keep the default home on schema 7 until old subjects are written.
The helper verifies old installed/holder hashes, schema 7, owned Hub release/interface, and
returns exact public bootstrap run IDs plus source/interface hashes. It neither clears nor
reclassifies history or caches. Any explicitly authorized public-only reset belongs before
creation of every project/canary; never use the normal isolated.sh end-of-seed home reset
between private writer and candidate migration phases.

The standalone `legacy.sh` command takes skew's `OUT OLD_COZY RUNTIME TENSORFS`
arguments followed by `--candidate NEW_COZY --ready-file READY.json`. Bootstrap
skew with the checksum-verified released 0.24.0 executable via `--cozy-bin`,
`--admin`, and `--command e2e/cli/containment/legacy.sh`; pass the candidate only
after `--`. The coordinator must disable skew's mixed machine-log capture for
this privacy command. `scan.py` from the composed containment fixture is required.

On a fresh GitHub-hosted runner, before any canary or project exists, the command
verifies that the only old controller runs are the exact public publication IDs,
all settled typed package-publication jobs, and that the project index is empty.
After proving both old writer locks/process cohorts stopped, it performs the
explicitly approved public-only reset of exact controller SQL/code/input/after and
digest-cache paths. Account keys, machine identity and installed tools survive.
No reset is allowed after the first project is created.

The actual released writer then records successful and failed published CPU runs,
File inputs, stdout/failure logs, output bytes, and a known original idempotent ask.
The candidate sees the same real V7 home after both old cohorts stop. Real SQLite
contention keeps the first automatic adoption pending; a native, TLS-pinned,
owner-signed request proves that a wrong actual worker binding refuses while both
canonical legacy sources survive. The native worker then completes its real
adoption and durable retirement. The fixture kills the exact Calcifer before it
can keep that receipt, cold-starts its next holder, and requires history to recover
the worker's complete inside receipt without a resubmission. History drives the real
binding bridge and verifies exact numbers/keys/digests/cursors/log/output rows,
inside input closure, and unchanged worker attempt/acceptance/start/finish facts.
An actual old client is refused. Finally the command encrypts, locks, scans outside
storage, moves the encrypted folder, cold-unlocks, verifies history and the same
old request retry, then locks and scans again. The only retained outside result is
the sanitized `legacy-receipt.json`; this source preparation is not execution proof.
