# Independent residual reference

`ci_svdq_calibration/oracle.py` is copied unchanged from Creator
`e521cc7797f93c0c97e20ef82ecbf7842cba4ff9`, file
`release-gate/suites/tensorfs/nvfp4-encode/ci_nvfp4_encode/oracle.py`.
SHA256: `b34b3cb1b5b40129d6df30c4f4e15e456a7da8ac888231c8a45c18dcf18f1f1f`.

That source preserves the independent Runtime NVFP4 producer/helpers from
`1c092cd8474f1a3d042d9e1ee69d61410e1adb04`. It never imports the current production
encoder or invokes TensorFS quantization. No parent-package or private-SDK import exists.

The gate uses the actual fit's alpha and BF16 factors to reconstruct its selected residual,
then calls this independent producer. The public Output conversion must publish exactly the
same residual/companion bytes. It does not copy the SVDQ fit algorithm. The held-out decoder
uses the stored value law and these historical format tables, not a production decode call.


## Native fitter reference

`historical_svdq_source.py.txt` preserves the full pre-cutover Runtime
`src/cozy_runtime/derive/svdq.py` from Runtime commit
`8c057e77d2fc0b25f7d3e4700930163cc9d4dd53` verbatim.
SHA256: `1c19029b480d60f38396eca9b830ce153422565884a1762c16fbcfd732bdc4d0`.

`historical_svdq.py` retains that source's Fitted, companions, BF16 rounding,
eight-alpha fit, randomized QR/SVD, scoring and decode functions. Its import plumbing
uses this package's frozen NVFP4 producer and an inline frozen copy of the serving fake
quantization law. It imports no current production fitter, serving decoder or encoder.
It is a numerical quality reference, never a Runtime fallback.

The native strategy uses a different pinned RNG and faer implementation. The gate compares
whole-weight and calibration quality, and compares each selected low-rank reconstruction
with an exact small SVD; singular-vector signs and Torch/native bytes are not compared.
For exact final NVFP4 packing, the independent residual reconstructs the native strategy's
canonical scale `s = F32(1 / stored_pre_quant_scale)` and sums BF16 up/down products in
increasing rank order with separate F32 multiply/add operations. The existing frozen
min-SSE NVFP4 oracle must reproduce every residual payload/scale byte exactly.
