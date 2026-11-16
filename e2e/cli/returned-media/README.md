# Returned media through the public CLI

This CPU fixture exercises `ImageFile.from_image` through actual package execution,
frame encoding, output custody, child calls and exported bytes. It also keeps explicit
`Outputs.publish` and bounded `save_video_stream` coverage. It runs only on `local`
and rents nothing.

The `typed-calls` CLI shard (`../typedcalls.sh`) copies the fixture, pins it to the run's candidate Runtime and
TensorFS wheels, locks it and runs `run.py`, which invokes the 27 cases below in order and fails if any case does.
Every command, NDJSON, export and verification result is kept under the shard's output.

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
| encoded-bound-copy | Same encoded bound enforced for a labeled pending image |
| invalid-shape | Failed `frame_shape` |
| invalid-kind | Failed `frame_type` |
| foreign | Failed `foreign_handle` |
| empty-format | Failed `encoder_unavailable`; only `None` selects defaults |
| aliased-return | Failed `duplicate_handle` through a reused nested container |
| aliased-child | Failed `duplicate_handle` through a reused child-result container |
| api-removed | Completed; public `Outputs.save_image` is absent |
| child-input | Completed; child reads a pending image without an output service |
| child-input-label | Completed; child consumes a labeled copy and the original reads those bytes |
| pending-probe | Completed; captured dimensions are available before encoding |
| publish-copy | Completed; publication consumes a pending copy and the original reads those bytes |
| duplicate-label | Failed `duplicate_handle` for two copies of one pending image |

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

Pending-input cases also compare the child's decoded geometry/pixel and encoded digest
with the original's readable bytes and the final export. A labeled copy must share one
adopted file, including the duplicate-handle rejection when both copies are returned.

Every negative case must produce a failed terminal **call** with its exact reason code,
a nonzero CLI exit, and no final media result or exported bytes. Setup/installation errors
cannot satisfy a negative case. Deferred codec failures intentionally use the platform's
existing `result_custody_failed` code; field-bound failures also require the encoder's
specific byte-limit diagnostic. Intermediate publication is checked from actual output
item events, not inferred from source or the final export alone.
