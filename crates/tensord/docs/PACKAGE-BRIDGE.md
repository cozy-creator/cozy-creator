# Package bridge

A package's environment is built one way, for a release (`published.rs`) and for local code
(`local_source.rs`): `published/environment.rs`. Every call runs on a pool executor
([GPU service](GPU-SERVICE.md)); the journal side is in `DURABLE-EXECUTION.md`.

## Generations

Layout: `<generations>/<32-hex>/` with `env/`, `.hold` and `generation.json` (`identity`, `package`,
`version`, `application`, `python`, `dependencies` (the Runtime pair), `interface`, `sdk`,
`sdk_fallback`, `runtime_build`, `source_digest`, `callees`). `generation.json` is published
atomically last. A generation is never updated. `catalog.rs` resolves one only under a shared
`.hold` flock. A local package's `env` links its environment (`editable/…/env-<key>`, kept while its
lock, pyproject, path dependencies, carried wheels and the machine's pair stand); each source
digest of it is a generation.

The build (`Publisher::build`), each step uv under one environment (the machine's, less credentials
and the seal's names, over this machine's uv cache and Pythons):
- `uv venv` for the package's Python (a release's card names it; local code: 3.12, its
  `requires-python` held by uv).
- Locked: `uv sync --frozen --inexact --no-dev --no-editable --no-install-project`, less the rows
  installed from files and the machine's pair; local directories rebuilt. Unlocked local code:
  `uv pip install --editable` of the project, offered the machine's pair.
- Wheels from files (`--no-deps --no-index --link-mode copy`): a release's Hub-hosted wheels (fetched by
  TensorFS), local code's carried ones. Then local code itself, editable, its `.pth` paths spelled as
  `import` lines so the static reader never follows them.
- The machine's Runtime/TensorFS pair over it, every other installed version a constraint. A pin the
  pair rejects keeps its version and the pair goes in without its dependencies (`sdk_fallback` says
  so). A floor the package puts on the pair that the pair misses goes in this environment alone
  (`sdk: own`): the version its lock names when that meets the floor, else the newest that does on uv's
  index, else among the GitHub prereleases `cozy machine update` also fetches. The machine, its pair and
  its other environments stay as they are, with no update and no restart. `runtime_floor_unmet` says
  what it needs and what to do when none is published; a network that can't answer is waited for, visibly.
  A development Runtime (a wheel file sent with `--runtime`, never a numbered prerelease) runs regardless,
  as its owner chose. If the package then doesn't import, the reason names the floor and the update that
  meets it. Other `uv pip check` findings are the run's warning.
- The last step compiles the environment's bytecode.
- Described inside the env by its own Runtime with `python -I -S` (no `.pth` execution): the
  environment's callees and digest, and local code's interface from its source.
- `Service::installed`: its kernels compile and a call's imports start, for both kinds. A warm install
  waits for those imports: one that fails is the install's warning, and its failure when a floor its
  development Runtime missed explains it.

Nothing assumes one Runtime per machine. Kernels compile under the environment's own Runtime and seal.
The describe step asks that Runtime. The executor seam is tolerant in both directions, and a room is
sent only to an executor whose hello offers it. `cozy package ls --on` and `cozy machine show --json`
give each environment's Runtime beside the machine's own. Memory on a GPU is shared through the driver, with no
state between executors.

## Calls between packages

A generation also records `callees`: the other installed distributions with a statically
described `cozy.application`, their application, interface, package identity and source digest.
Their child runs use the caller's immutable environment and the callee's own App. A callee
can call its own internal exports; another package can call only its public exports. Memoized
calls use the callee's source digest.

A local-source manifest may carry an optional `callees` map from normalized distribution names
to package identities. It names the project's files (or, on this computer, its directory) and
the wheels it carries. A published release's map is its lock: every row its Hub publishes is
`<release org>/<row name>` (a Hub lock pins only its own org). A wheel's URL never names a
package; anything unmapped, and older records without a package identity, read as
`local/<normalized-distribution>`. Description
reads metadata and AST inside the environment without importing package code.

Model slots and resolution-cache entries belong to the callee package. The caller's model
choices do not override another package's slots; a choice addressed to a callee slot names it. GPU
execution uses the callee's interface; import-only parents are keyed by generation and App.
These are additive generation and manifest fields; the machine API is unchanged.
An export registered by distinct Apps is refused only when that ambiguous call is made;
other calls and same-App aliases continue to work.
