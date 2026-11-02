# e2e

Real runs of `cozy` on GitHub's runners, chosen by what a change touches (CI v2: `tracker/design/ci-v2.md`).

- `ci/affected <repo> <base> <head>` prints what a change must run; `ci/suites.toml` holds the rules.
- **The candidate set** (`candidates/`): on Linux, TensorD, TensorFS and the Runtime, built from the change at its commit and the
  other repos at master (`stack_ref`, `runtime_ref`), go on the local machine together, so a mismatch between them fails the PR.
  A built cozy or Runtime is `<version>+dev.g<sha8>`; a taken one is its exact release version (a prerelease's too), which
  `cozy machine update` fetches and verifies.
- `hubcheck/`: every suite's cozy reaches tensorhub.com through it, and it holds each request and response to the API
  document the Hub serves (GET /openapi.yaml; tensorhub's server is the truth, the document follows it). A mismatch
  fails the suite: a request is the client's drift, a response the document's.
- `run.sh`: how every suite (and the release gate) runs `cozy run`: `--out`, the record's `output[].path`, and a run
  followed by the CLI's own events to its end or a typed error; nothing is cut for time.
- `cli/run.sh`: install, up, the local machine, the candidates, a weightless request through them, BiRefNet on the CPU, a second
  executor, down, uninstall. macOS and Windows run no models themselves yet (`platform::Os::local_inference`): there it checks
  what needs no machine and that every local verb refuses `local_unavailable`.
- `imports/run.sh`: every published first-party package release installs (`cozy package install`) and imports on the candidates;
  a Runtime that moves what a published package imports fails here.
- `mps/run.sh`: BiRefNet and SDXL on a Mac's GPU (explicit `select` only, until macOS runs models itself).
- `capabilities/run.sh`: a run capability (AuthKit's workload grant) reads its private commit and nothing else: signed and
  redeemed as Calcifer and TensorD do, renewed, a copied token refused without its DPoP key, refused for another repository,
  a release, a delete, rentals, another account's repository, an altered or expired capability and, with `--rental`, once the
  rental ends. It speaks the Hub's API, runs nothing; the release suite runs it on its throwaway upload.
- `.github/workflows/e2e.yaml` runs the selected suites on PRs here and for cozy-runtime, packages and the release gate. On Linux
  each candidate is one `candidate` job (`candidates/build.sh`), reused from an earlier cozy-creator run that built the same
  sources, and `cli` and `imports` run in shards (`--shard`) on their own runners; `e2e` checks coverage over every shard.

Run one suite on a fresh computer: `e2e/cli/run.sh --out /tmp/e2e [--tensord ./cozy] [--runtime <wheel or version>] [--tensorfs <wheel or version>]`.
Against chosen versions: `gh workflow run e2e.yaml -f repo=cozy-runtime -f ref=<sha> -f stack_ref=<cozy-creator sha> -f select="cli imports build:runtime"`.

## Merging (Paul, 2026-10-24)

CI runs only in the merge queue, on the final PR + branch state, for PRs into `master` and `rc/*`. A pull request
runs placeholders, so it can be queued; nothing runs on a push. Merge with `gh pr merge --squash`, which enqueues,
never `--admin`. A direct push to master (admins) skips CI. Required checks:

- cozy-creator: `build`, `e2e`
- cozy-runtime, packages: `build`, `e2e / e2e`
- tensorhub: `build`, required once Actions runs there again (the org's budget)
- An RC branch gets its own queue: `ci/rc <repo> <name>` (GitHub refuses a merge queue on `rc/*`).
- tensorhub has no merge queue: it is private, and GitHub offers merge queues on private repos only with Enterprise Cloud.
