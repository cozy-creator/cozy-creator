# e2e

Real runs of `cozy` on GitHub's runners, chosen by what a change touches (CI v2: `tracker/design/ci-v2.md`).

- `ci/affected <repo> <base> <head>` prints what a change must run; `ci/suites.toml` holds the rules.
- **The candidate set** (`candidates/`): on Linux, TensorD, TensorFS and the Runtime, built from the change at its commit and the
  other repos at master (`stack_ref`, `runtime_ref`), go on the local machine together, so a mismatch between them fails the PR.
  A built cozy is the RC's `<x.y.z>-dev+g<sha8>` or a release's `<version>+dev.g<sha8>`, a built Runtime `<version>+dev.g<sha8>`; a taken one is its exact release version (a prerelease's too), which
  `cozy machine update` fetches and verifies.
- `hubcheck/`: every suite's cozy reaches tensorhub.com through it, and it holds each request and response to the API
  document the Hub serves (GET /openapi.yaml; tensorhub's server is the truth, the document follows it). A mismatch
  fails the suite: a request is the client's drift, a response the document's.
- `run.sh`: how every suite (and the release gate) runs `cozy run`: in its project, the record's `output[].path`, and a run
  followed by the CLI's own events to its end or a typed error; nothing is cut for time.
- `cli/run.sh`: install, up, the local machine, the candidates, a weightless request through them, BiRefNet resolved,
  downloaded and warmed (its forward pass on the CPU only when ci/affected selects `model`: the Runtime, TensorD,
  TensorFS, varena or the package changed), a second
  executor, down, uninstall. macOS and Windows run no models themselves yet (`platform::Os::local_inference`): there it checks
  what needs no machine and that every local verb refuses `local_unavailable`.
- `imports/run.sh`: every published first-party package release installs (`cozy package install`) and imports on the candidates;
  a Runtime that moves what a published package imports fails here.
- `cpu/run.sh`: the free CPU test. Every package, built from its own lock with CPU torch, runs its entry points through
  the candidate Runtime's real executor (`python -I -m cozy_runtime.internal.executor`), driven over the seam with
  TensorD's frames (`drive.py`), on tiny random weights its fixture builds from the package's own model classes
  (`tiny/<package>.py`, ingested by TensorFS's import or writer). Each cell judges its output (present, finite, not
  flat, thumbnails within tolerance of `refs/<package>.json`); a two-rank cell runs a gloo group through the executor
  and equals its one-rank twin sample by sample (0.5/255 allowed in the worst frame). An orchestrator's child calls run
  on a child executor through the driver's stand-in for TensorD's call broker. With `--cozy`, each package also goes
  through cozy's CLI (`cli.sh`): its code uploaded with an empty file and described as the executor's interface says,
  Calcifer's private lock API leaving its lock as committed, and every cell's request bound by Calcifer's binder against the
  published release (`--estimate`: nothing runs). Also a strict typecheck, `describe` (the static reader), and
  `surface.py` (static: every TensorFS name the Runtime reads exists). One line per cell, `PASS|FAIL|SKIP <package>
  <entry> <cell>`. No GPU, rental or sign-in. `e2e/cpu/run.sh --out DIR --packages DIR --runtime <wheel|dir|version|lock> --tensorfs <wheel|version|lock> [--cozy BIN --project DIR --stage-code BIN] [--only pkg,..] [--cells entry/cell,..] [--record]`;
  `--cozy` uses the API `stage-code` example, supplied by candidates or built once when not supplied.
  a fixture's `WAIVE` names the release-geometry checks a tiny model cannot meet, and `PATCH` the geometry constants it
  shrinks; both stay GPU-only.
- `capabilities/run.sh`: a run capability (AuthKit's workload grant) reads its private commit and nothing else: signed and
  redeemed as Calcifer and TensorD do, renewed, a copied token refused without its DPoP key, refused for another repository,
  a release, a delete, rentals, another account's repository, an altered or expired capability and, with `--rental`, once the
  rental ends. It speaks the Hub's API, runs nothing; the release suite runs it on its throwaway upload.
- `podsecrets/run.sh <tensorhub checkout> <cozy binary> <out>`: xs-157, by hand on a computer with Docker. The real Hub
  (throwaway Postgres, its Docker provider) rents a pod of that cozy binary: the pod's environment names no secret, the
  Hub pins the leaf at the provider's address, only that key's signature speaks for the worker (another key, a token, a
  stale clock and an endpoint report are refused), and a restart keeps the pin. On real pods: the `hub` release suite.
- `skew/run.sh --hub-src <tensorhub checkout> --client <version> --out DIR [--cozy-min X.Y.Z]` (xs-166, `select=skew`
  with `hub_ref`, `skew_clients`, `cozy_min`): a released cozy (`previous`: the previous minor's latest patch) against a
  Hub built from source on a throwaway Postgres and a MinIO as strict as R2 about SigV4 (`skew/sigv4.go` checks it
  first). The client signs in and runs the Hub suite's cells on its local machine, the model upload by the machine (the
  workload writer, as pods upload); a client older than `cozy_min` must be refused `client_too_old` at every step.
  Any 5xx fails. Before launch it reports; it is not part of `e2e`'s verdict.
- `.github/workflows/e2e.yaml` runs the selected suites on PRs here and for cozy-runtime, packages and the release gate. On Linux
  each candidate is one `candidate` job (`candidates/build.sh`), reused from an earlier merge-group or dispatched run that built
  the same build inputs, and `cli` and `imports` run in shards (`--shard`) on their own runners; `e2e` checks coverage over
  every shard.

Run one suite on a fresh computer: `e2e/cli/run.sh --out /tmp/e2e [--shard S] [--tensord ./cozy] [--runtime <wheel or version>] [--tensorfs <wheel or version>]`.
Against chosen versions: `gh workflow run e2e.yaml -f repo=cozy-runtime -f ref=<sha> -f stack_ref=<cozy-creator sha> -f select="cli imports build:runtime"`.

## A PR's proof is a cell

A behaviour a PR adds or fixes gets a cell here instead of a proof run by hand on a laptop: the merge queue runs it on
the PR's final state, on a fresh runner, through the real CLI and Calcifer. A cell is one part of the cli suite: a
function in `cli/<area>.sh` (`projects` in `cli/project.sh`, `lifetime` in `cli/lifetime.sh`), listed in a shard in
`cli/run.sh`. It uses lib.sh's `cozy` (each call logged for coverage), `check` (a failure recorded, the suite goes on)
and `cell` (one `cozy run` followed to its end), makes its fixtures at run time, and waits on events, never a fixed
sleep. A new verb or flag also needs its line in `coverage.toml`.

- **Earn its place.** It covers a real user path or a bug that escaped, and no other cell covers the same path; one that
  shares another's setup joins it. When you add one, look for one to remove.
- **Budget.** A typical cli/calcifer PR's `e2e` is at most 7 min and 11 jobs. Put a cell in the shard with room (its
  step times are in the run's log); a new shard needs a job the budget gives back elsewhere.
- **Prove it before the queue:** dispatch e2e on the PR's head,
  `gh workflow run e2e.yaml -R cozy-creator/cozy-creator --ref <branch> -f ref=<head sha> -f select=cli`. That run's
  candidate is reused by the PR's queue entry when the PR is up to date. Its PR's `## Proof` links that run.
- **On your own computer:** `e2e/cli/run.sh --out DIR --shard commands --tensord <dev cozy>` installs cozy as a person does
  into this computer's default home and puts the candidate on, so run it on a computer that is yours to replace (a VM,
  a runner); never on a shared one.

## Merging (Paul, 2026-10-24)

CI runs only in the merge queue, on the final PR + branch state, for PRs into `master` and `rc/*`. A pull request
runs placeholders, so it can be queued; nothing runs on a push. Merge with `gh pr merge --squash`, which enqueues,
never `--admin`. A direct push to master (admins) skips CI. Required checks:

- cozy-creator: `build`, `e2e`
- cozy-runtime, packages: `build`, `e2e / e2e`
- tensorhub: `build`, required once Actions runs there again (the org's budget)
- An RC branch gets its own queue: `ci/rc <repo> <name>` (GitHub refuses a merge queue on `rc/*`).
- tensorhub has no merge queue: it is private, and GitHub offers merge queues on private repos only with Enterprise Cloud.
