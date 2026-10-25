# Basic MPS execution proof — 2026-10-25

[Manual workflow 40174863409](https://github.com/cozy-creator/cozy-creator/actions/runs/40174863409)
passed on macOS 15.7.9, Apple M1 (Virtual), with 7 GiB physical RAM and an Apple Paravirtual
GPU reporting 5,010,096,128 bytes. The normal default-home CLI updated the binary and SDK,
preserved the local TLS fingerprint, and invoked real GPU calls with `--gpus=1`.

## Exact cohort

| Source | Commit |
|---|---|
| Creator | `7b5f6d86ce5c23c9add24b816039489b637f64b3` |
| Runtime | `efcb6bee9bb71c90e1494513afb7075bc1016137` |
| Unchanged development packages | `0bfcacd7fb2cbdc5ae52f6a17467241cd5fa7d9c` |

Torch was 2.14.1; TensorFS was 0.15.3. Candidate artifact SHA256 values:

| Artifact | SHA256 |
|---|---|
| Native Mac CLI | `7c975d0a8f4b9c8c6ff2089b40c8a643e488355697d835bd125367d617011041` |
| TensorFS Mac wheel | `c0eb2ecaff2462535f5fa3b42306ace0a480c4370a3a80988f6184e0a63a460c` |
| Runtime pure wheel | `dfb6b91892923ab44e57b421ce323e148ce243c902bf080c19ece7e586f6622f` |

## Actual outcomes

- **Checkpoint cell 4 completed:** a 67,112,960-byte part crossed the 64 MiB staging window;
  warm reuse read zero bytes; F16 and empty parts loaded; an escaped alias retained storage;
  dropping it released storage; refill read exactly 67,112,960 bytes. This uses real TensorFS
  checkpoints and Torch MPS computation/storage, with no Metal allocator or mocked backend.
- **BiRefNet cell 6 completed:** actual MPS execution recorded 448,878,336 allocated bytes and
  444,386,812 weight bytes read. Encoding and delivery produced an 87,714-byte WebP with
  SHA256 `4699bd9945760c6a64a5880cd4e0d839380e32dae110f76ab0ab8897558b4065`.
  The gate checked source/output geometry and a white-background fraction of 0.91. Visual
  inspection confirmed the corgi remained on a white background.
- **SDXL cell 8 refused capacity:** its 6.5 GiB weights exceed this runner's roughly 4.7 GiB
  GPU budget. The explicit `memory_exhausted` refusal was the permitted outcome. SDXL did
  not generate an image or complete the requested 12 steps.

The workflow's `e2e-mps` artifact contains the source/artifact manifest, hardware facts, actual
run records, logs and delivered image at `out/birefnet/6-image.webp`. Raw records and their
SHA256 digests are also retained in the owned task handoff; they were not rewritten.

## Reproductions and fixes

- [40164029239](https://github.com/cozy-creator/cozy-creator/actions/runs/40164029239): Runtime
  wheel construction hit an unpublished serving-index URL. Artifact builds now use `uv --no-config`.
- [40166134142](https://github.com/cozy-creator/cozy-creator/actions/runs/40166134142): the old
  local boot's empty GPU list differed from newly available Apple discovery. The separate
  unsigned-local receipt repair preserves identity checks and all signed-rental rules.
- [40167193519](https://github.com/cozy-creator/cozy-creator/actions/runs/40167193519): ordinary
  binary/SDK updates succeeded, then a newly recorded MPS event was queried too soon at completion.
  The terminal boundary now uses the existing MPS synchronization fence.
- [40168083697](https://github.com/cozy-creator/cozy-creator/actions/runs/40168083697): the memory
  cell passed and BiRefNet computed on MPS, but Linux-only `O_PATH` failed output encoding.
  The final source reads from the same verified regular-file descriptor without `/proc`.

## Qualification boundary

This proves basic MPS checkpoint residency and real BiRefNet image output. It is not a full SDXL
run, benchmark, ComfyUI comparison, broad pressure/cancellation qualification or release. Varena
remains CUDA-only; MPS uses Torch allocation. Published package restrictions and consumer
remote-only shipping gates remain unchanged. Mac process containment and host-pressure watching
remain separate gaps reported by the existing service.

[Windows compilation 40175280228](https://github.com/cozy-creator/cozy-creator/actions/runs/40175280228)
passed at the same Creator source on `windows-2025`: Varena, TensorFS, Python bindings and the
separate store-free TensorFS core. This is compilation only, not Windows CUDA execution,
Runtime native-loop support or Windows image-output qualification. No unit tests or mocks were added.
