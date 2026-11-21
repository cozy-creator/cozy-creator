# XS170 required-data preparation

This one-off client prepares a metadata-only repair for the five lanes of
`minimaxai/minimax-h3@1.0.0-rc.7` and the `bf16` lane of
`lbh-123-ai/h3-latent-upscaler@0.1.0`. The default preparation binary has no network
client, credential handling, publication operation, model execution or Store access.
Its default TensorFS dependency has network features disabled. A separate operator
binary requires the explicit `operator` build feature and coordinator execution.

```text
verified old header + complete old tree
  -> clone all existing model data and supporting file references
  -> H3: add text_encoder.assets[processor/<file>]
  -> upscaler: add model.config.latent_normalization
  -> native Header/Tree encoding + strict roundtrip and preservation checks
  -> local objects and old-to-new manifest
```

Run with an empty output path:

```sh
cargo run --manifest-path scripts/xs170-artifact-reissue/Cargo.toml --locked -- \
  <verified-input-inventory.json> <processor-snapshot-directory> \
  <latent-normalization-proposal.json> <new-output-directory>
```

The input inventory names the exact old checkpoint, header and tree paths and
digests. The separate processor snapshot contains the exact currently served
package files. The normalization proposal names its source file and digest;
the client verifies that its learned values equal that source.

Every old header and tree must re-encode to its original bytes with TensorFS's
native encoders. Removing only the intentional additions from each result must
then reproduce those old header bytes exactly. All original chunks, tensors,
configuration, timestep plans, metadata, ancillary file references and component
order are preserved. The complete old tree is cloned; only its existing header
reference changes. No tensor objects are read.

Prepared `.header` objects are thin CAS metadata, not complete portable model
files. A complete `.cozytensors` export still requires its existing weight
closure. This client does not download or materialize that closure.

Publication is a separate coordinator operation. Review the manifest first;
upload only its new asset/header/tree objects, admit the six new checkpoints,
then publish a new immutable release mapping. The proposed H3 release is
`1.0.0-rc.8`; the proposed upscaler release is `0.1.1`. Preserve old versions and
exact references. Update package defaults only after admission, permissions and
consumer data checks pass. Rollback restores the previous package/default
reference while retaining both immutable model releases; it does not overwrite
or delete the old checkpoint objects.

## Explicit operator phases

`xs170-artifact-operator` is a one-off operator client, not an installed Cozy
command. Compile/verify it on hosted CI with the explicit `operator` feature.
Its default phase is `check`. Every phase requires the exact frozen manifest,
the original input inventory, a new receipt path, the existing user's home and
the literal trusted Hub origin. It does not read general Hub/provider settings.
It uses the existing `api::hub::Client` and device-key `UserSession`; tokens,
keys, signed object URLs and raw remote error bodies are never printed.

```sh
xs170-artifact-operator check \
  --manifest <prepared-final/manifest.json> --inputs <inputs/inventory.json> \
  --hub https://tensorhub.com --user-home <existing user home> --receipt <new-check.json>

xs170-artifact-operator admit \
  --manifest <prepared-final/manifest.json> --inputs <inputs/inventory.json> \
  --hub https://tensorhub.com --user-home <existing user home> --receipt <new-admit.json>

xs170-artifact-operator publish --model minimaxai/minimax-h3 \
  --manifest <prepared-final/manifest.json> --inputs <inputs/inventory.json> \
  --hub https://tensorhub.com --user-home <existing user home> --receipt <new-h3-publish.json>

xs170-artifact-operator publish --model lbh-123-ai/h3-latent-upscaler \
  --manifest <prepared-final/manifest.json> --inputs <inputs/inventory.json> \
  --hub https://tensorhub.com --user-home <existing user home> --receipt <new-upscaler-publish.json>
```

Check validates the pinned manifest SHA256, every local object, the undo-to-old
header/tree invariants, fresh authenticated old public release mappings, base
model identities and proposed version absence using PEP 440 equivalence.
Admit repeats those checks, confirms only the 17 new objects in asset/header/tree
order through `Endpoint::upload`, then records the six exact checkpoints. It
never walks, downloads or uploads carried tensor objects. Publication requires
one explicit reviewed model and verifies its exact new checkpoints were already
admitted. It rechecks old public mappings and proposed version absence before
the immutable release PUT, then verifies the published result. Existing versions
and 409 collisions stop; the client never chooses a new version or overwrites one.
One release per invocation makes partial publication explicit and reviewable.

Receipts are created exclusively and updated durably after each completed step.
They contain object IDs, lane/version mappings, confirmation/admission results
and concise errors. All live publication and namespace permission checks remain
server-authoritative. No phase starts Calcifer, workers, rentals or model execution.

Hosted verification uses only generated data and loopback Hub/storage fixtures.
It checks the 17-object/six-admission flow, absence of weight requests, no bearer
on a storage redirect, read-only checks, missing admissions, changed old mappings,
existing versions and a race-time 409 with an unsafe URL in its raw refusal.
The operator is not run against production by its verification workflow.
