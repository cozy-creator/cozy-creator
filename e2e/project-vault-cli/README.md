# Installed project-encryption CLI proof

The manual `dev-build` option `project-vault-cli` builds the actual candidate page
and single Cozy executable, embeds its pinned provider, and installs it at the
ordinary `~/.cozy/bin/cozy` path in a fresh GitHub runner. The installed CLI
qualifies default encrypted creation with secure file input and explicit `cozy new --unencrypted`
for ordinary/conversion controls, reads them with `cozy status` from their
directories, and exercises the `cozy vault` lifecycle. Encrypt and rotate run
from outside the project with `cozy vault --directory`, so Windows can rename
the backing directory. Calcifer starts through the normal CLI service path.

Media imports, field edits, history, and YAML reads use the installed
controller's authenticated browser API over loopback. Media uploaded after
encryption goes to `/v1/project-files/{digest}?project={id}` before the import.
The route uses the canonical project ID returned in JSON. The API authentication
comes from the default controller's holder record and stays in memory.

The fixture covers an ordinary project, conversion, encrypted image/audio/video
imports, a CRDT state edit and history, media decoding/seek, lock/read/fallback
refusal, wrong credentials, recovery/passphrase replacement, full rekey, explicit
plaintext export, parallel manual locks, idle/absolute expiration, recovery
of the new generation, and normal shutdown while still unlocked. It stops that
runner's Calcifer through its real authenticated graceful shutdown API, then
scans global retained files (CAS, journals, databases and logs included) plus the
encrypted backing for known synthetic canaries and credential strings. Only
runtime `view` directories are excluded. A canary scan is a functional privacy
check, not a cryptographic audit or proof against every possible encoding.
The fixture opens the intentionally plaintext export with `cozy status` after
the scan. It then compares the whole graph through the YAML endpoint and
requires a distinct project identity while the original is locked.

The parallel cell checks two concurrent manual locks. The controller's separate
qualification owns pending-unlock/lock ordering. Expiration checks a requested
two-second policy with status polling; desktop file reads do not automatically
renew Calcifer activity.

This fixture refuses to run outside an ephemeral GitHub Actions host, with a
`COZY_HOME` override, or when PATH chooses a different Cozy executable.
Synthetic inputs and intended plaintext exports live outside `~/.cozy`.
Credentials/recovery files are deleted afterward and are never uploaded. Only
`receipt.json` and credential-free command and API request metadata are retained
as artifacts. The Windows ARM64 media cell reads measured metadata and the
canonical project path from `cozy --json --full status`. The first cold media
budget is 300 seconds, and failure diagnostics have a separate 10-second budget.

The integration owner dispatches it against the final combined full SHA on each
native runner; source/syntax checks alone are not a product pass. The public
installer/update packaging keeps its existing separate release gate.
