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
