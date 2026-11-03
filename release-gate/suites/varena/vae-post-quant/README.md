# H3 post-quant provider qualification

Real clients for the restricted Runtime post-quant operation. The implementation
uses a declared pointwise GEMM alternative to the actual biased 1x1x1 Conv3d;
it does not claim identical cuDNN algorithm or rounding behavior.

Runtime dependency: existing prepared dense provider from frozen
`5498a8f09e25672952abeb67ddccfc28205c3181`, plus the new provider/consumer files in
`codex/xs138-p2-vae-post-quant`. The coordinator composes shared export/build wiring
and exact native library cohorts. This fixture does not modify another owner's
environment, cache, models, source or installed artifacts.

Host declaration/layout clients and actual GPU qualification clients are being
prepared. GPU/rental execution is not authorized in this lane. GPU evidence must
use the ordinary Cozy child with exact artifact and device identities, compare
actual Conv3d full/ragged shapes and FP16/FP32 source weights, and retain failed
receipts. No unit mocks, guessed workspace or Python retry policy.

Outer VAE chooser removal and full-model qualification remain separate. The model
gate is 15 seconds, 362 frames, 1344x768, 30 steps and audio; 5 seconds is smoke only.
