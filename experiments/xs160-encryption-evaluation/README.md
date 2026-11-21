# xs-160 encryption evaluation — throwaway

This branch contains isolated library experiments for [tracker xs-160](https://github.com/cozy-creator/tracker/blob/master/tracker/cross-cutting/xs-160-project-encryption-and-backup.md). It is evidence for choosing a design, not production integration or a release candidate. Do not merge it into the product.

Questions:

1. Can native Rust Automerge/Loro retain independent asset versions and expose meaningful conflicting edits across offline replicas, serialization/restart and reordered delivery?
2. What does OpenMLS actually provide for group membership, removal, epochs and recovering persistent encrypted project objects?
3. Which off-device recovery mechanisms preserve the required user experience without granting TensorHub unilateral decryption authority?

All data and keys are synthetic. No user account, paid rental or deployed service is involved. Current implementation scope is [Calcifer serving locally or on a trusted CPU worker](https://github.com/cozy-creator/tracker/blob/master/design/project-serving-model.md): the owner/server holds the archive key, while ordinary collaborators receive authenticated access. Owner-requested rotation and storage-key compromise use a full retained-project rekey; access-only revocation closes server access. V1 recovery uses a saved off-device secret. OpenMLS, PRE, PRF, threshold and Sui/Seal results remain research, not required v1 subsystems.

## Executed evidence

- [Automerge/Loro results](merge/RESULTS.md): real native Rust convergence, current conflict alternatives versus historical checkout, version-ID collisions, deletes, snapshot/restart behavior and reordered delivery. This favors Automerge's conflict API for project metadata; semantic deletion/immutability policy remains application work.
- [OpenMLS membership results](membership/RESULTS.md): real group crypto, joins, storage reload, epoch/history behavior, concurrent membership commits and removal. Persistent project archives and application permissions remain separate work.
- [Recovery options](recovery/EVALUATION.md): concrete Rust components and service/provider boundaries; includes an executed [vsss-rs primitive receipt](recovery/threshold-smoke/receipt.json) and read-only Seal endpoint receipts. No passkey provider recovery or hosted decryption was executed.
- [Filesystem results](filesystem/README.md): actual securefs Linux mount, file I/O, media seeking, busy unmount, remount and idle unmount. Windows/macOS/native GUI player remain untested; securefs is not the selected Rust backend.
- [Revocation/PRE probe](revocation/README.md): real recrypt 0.16.0 (APIs 256 and 480) transforms small key capsules with an unchanged 4 MiB payload. Demonstrates honest-proxy revocation, cached-capability survival, malicious retained grants, new-epoch exclusion and administrator/server key augmentation. Policy is simulated; no AuthKit/network integration. The current plan uses server-side access revocation for collaborators and full retained-project re-encryption for storage-key compromise or owner-requested rotation. PRE remains research evidence, not an adopted dependency.

Each standalone Rust manifest has its own workspace and lockfile. Build with the documented toolchain/commands, outside the main Creator workspace. Do not interpret these fixtures as production authorization, key custody, protocol or performance qualification.
