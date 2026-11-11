# XS170 required-data preparation

This one-off client prepares a metadata-only repair for the five lanes of
`minimaxai/minimax-h3@1.0.0-rc.7` and the `bf16` lane of
`lbh-123-ai/h3-latent-upscaler@0.1.0`. It has no network client, credential handling,
publication operation, model execution or Store access. Its TensorFS dependency
has default network features disabled.

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
