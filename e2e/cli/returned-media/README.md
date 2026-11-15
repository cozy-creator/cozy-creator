# Returned media through the public CLI

This CPU fixture exercises `ImageFile.from_image` through actual package execution,
frame encoding, output custody, child calls and exported bytes. It also keeps explicit
`Outputs.publish` and bounded `save_video_stream` coverage. It uses the default Cozy
home and an existing normal project, runs only on `local`, and rents nothing.

Copy the fixture into a durable proof directory and pin an immutable candidate before
running. As with `../typed-calls`, the repository fixture carries no machine-specific
SDK path and is never published. Do not reference a live SDK worktree: its environments
and caches can be uploaded along with the source. A tracked-file snapshot is sufficient:

```sh
proof="<new durable proof directory>"
mkdir -p "$proof/sdk"
cp -a e2e/cli/returned-media/fixture "$proof/fixture"
git -C "<Runtime checkout>" archive <candidate-commit> | tar -x -C "$proof/sdk"
cat >> "$proof/fixture/pyproject.toml" <<'TOML'

[tool.uv.sources]
cozy-runtime = { path = "../sdk" }
TOML
uv lock --project "$proof/fixture"
chmod -R a-w "$proof/sdk"
```

An immutable wheel can instead be selected in the copied pyproject's uv source. Keep its
SHA256 alongside the receipts. The coordinator must install the compatible candidate CLI,
Calcifer and machine first; the runner never starts a separate daemon or changes installed
services. Do not set `COZY_HOME`. Choose a new output directory outside the fixture tree.

```sh
uv run --script e2e/cli/returned-media/run.py \
  --cozy "<candidate cozy binary>" \
  --project "<existing normal Cozy project>" \
  --fixture "$proof/fixture" \
  --out "$proof/receipts" \
  --sdk-identity "<candidate commit or wheel SHA256>"
```

The runner preserves each command, stdout NDJSON, stderr, exports, input file hashes, CLI
hash, SDK identity label, run IDs and verification result. `--sdk-identity` is the build
owner's provenance label; the runner does not infer a source commit from a package version.
It invokes all 21 cases sequentially, without a timeout or retries, and exits nonzero if any
case fails verification. Existing receipt directories are refused rather than overwritten.
Source description or an import is not process/custody evidence.

| Case | Expected outcome |
|---|---|
| default | Completed; lossless WebP, 4×3 RGBA `(15,63,127,95)` |
| png | Completed; explicit PNG, same pixels |
| webp | Completed; explicit WebP, same pixels |
| large-side | Completed; automatic PNG, 16384×1, same pixels |
| encode-failure | Failed `result_custody_failed`; WebP dimension-limit diagnostic |
| nested | Completed; nested list file, WebP, 2×2 RGB `(7,11,19)` |
| child-forward | Completed; identified successful child, original RGBA and metadata |
| child-nested | Completed; child file forwarded through a nested list |
| child-failure | Failed `result_custody_failed`; identified failed child with that reason |
| intermediate | Completed; intermediate PNG event followed by final WebP and original RGBA |
| streaming | Completed; exported H.264 MP4, three 16×16 frames at 24 fps |
| duplicate | Failed `duplicate_handle` |
| decoded-bound | Failed `result_decoded_bound` |
| encoded-bound | Failed `result_custody_failed`; declared one-byte encoded bound diagnostic |
| legacy-encoded-bound | Same failure for explicit `Outputs.save_image` |
| invalid-shape | Failed `frame_shape` |
| invalid-kind | Failed `frame_type` |
| foreign | Failed `foreign_handle` |
| empty-format | Failed `encoder_unavailable`; only `None` selects defaults |
| aliased-return | Failed `duplicate_handle` through a reused nested container |
| aliased-child | Failed `duplicate_handle` through a reused child-result container |

Every success must have a completed terminal call, a landed final output, and one exported
file whose SHA256 matches the custody receipt and result digest, and whose byte length
matches the custody receipt and any reported result length. Root result references remain
opaque handles; deferred frames may omit their encoded length from the result document.
Forwarded child grants must retain their canonical SHA256 reference and encoded length.
Images are decoded with Pillow: container format, geometry, alpha mode and every pixel must
match. The generating handler overwrites its original PIL image after constructing the
returned value, so these checks also exercise snapshot immutability. File metadata must
survive ordinary and child-returned results. Streaming output is decoded with PyAV to check
its codec, frame count, dimensions and presentation timestamps; lossy video pixels are not
required to equal the input pixels.

Every negative case must produce a failed terminal **call** with its exact reason code,
a nonzero CLI exit, and no final media result or exported bytes. Setup/installation errors
cannot satisfy a negative case. Deferred codec failures intentionally use the platform's
existing `result_custody_failed` code; field-bound failures also require the encoder's
specific byte-limit diagnostic. Intermediate publication is checked from actual output
item events, not inferred from source or the final export alone.
