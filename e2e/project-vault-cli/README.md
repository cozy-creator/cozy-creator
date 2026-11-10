# Installed project-encryption CLI proof

The manual `dev-build` option `project-vault-cli` builds the actual candidate page
and single Cozy executable, embeds its pinned provider, and installs it at the
ordinary `~/.cozy/bin/cozy` path in a fresh GitHub runner. The script invokes only
that normal CLI. Calcifer starts through its normal client/service path; no
alternate home, extra daemon, model, account or rental is created.

The fixture covers an ordinary project, conversion, encrypted image/audio/video
imports, a CRDT state edit and history, media decoding/seek, lock/read/fallback
refusal, wrong credentials, recovery/passphrase replacement, full rekey, explicit
plaintext export, parallel manual locks, idle/absolute expiration, recovery
of the new generation, and normal shutdown while still unlocked. It stops that
runner's Calcifer with `cozy down`, then
scans global retained files (CAS, journals, databases and logs included) plus the
encrypted backing for known synthetic canaries and credential strings. Only
runtime `view` directories are excluded. A canary scan is a functional privacy
check, not a cryptographic audit or proof against every possible encoding.
The intentionally plaintext exported copy is opened through the CLI only after
this scan, so ordinary-project caching cannot contaminate the encrypted-only
privacy check. It must open independently with the original locked.

The parallel cell checks two concurrent manual locks. The controller's separate
qualification owns pending-unlock/lock ordering. Expiration checks a requested
two-second policy with status polling; desktop file reads do not automatically
renew Calcifer activity.

This fixture refuses to run outside an ephemeral GitHub Actions host, with
`COZY_HOME`/`COZY_PROJECT`, or when PATH chooses a different Cozy executable.
Synthetic inputs and intended plaintext exports live outside `~/.cozy`.
Credentials/recovery files are deleted afterward and are never uploaded. Only
`receipt.json` and credential-free command metadata are retained as artifacts.

The integration owner dispatches it against the final combined full SHA on each
native runner; source/syntax checks alone are not a product pass. The public
installer/update packaging keeps its existing separate release gate.
