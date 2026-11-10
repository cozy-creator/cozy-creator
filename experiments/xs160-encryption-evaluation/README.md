# xs-160 encryption evaluation — throwaway

This branch contains isolated library experiments for [tracker xs-160](https://github.com/cozy-creator/tracker/blob/master/tracker/cross-cutting/xs-160-project-encryption-and-backup.md). It is evidence for choosing a design, not production integration or a release candidate. Do not merge it into the product.

Questions:

1. Can native Rust Automerge/Loro retain independent asset versions and expose meaningful conflicting edits across offline replicas, serialization/restart and reordered delivery?
2. What does OpenMLS actually provide for group membership, removal, epochs and recovering persistent encrypted project objects?
3. Which off-device recovery mechanisms preserve the required user experience without granting TensorHub unilateral decryption authority?

All data and keys are synthetic. No user account, paid rental or deployed service is involved. Results, exact versions, commands and limitations will be recorded with the experiments. Sui/Seal remains an optional candidate evaluated on the same requirements as the other approaches.
