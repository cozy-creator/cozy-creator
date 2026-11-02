# Host boundary only

`source-cpu.json` is real Torch 2.14.1+cpu execution of the immutable Source client; warnings are preserved in `source-cpu.stderr`. `native-host.json` is the production built extension/core bridge, checking same-core identity, ten native malformed layouts and six valid layouts refused without the Torch consumer hook installed. The DLPack offset observation records the real CPU compatibility failure that motivated pointer normalization.

Native Cargo checks/build and exact source hashes are recorded in `manifest.json`. This is an unoptimized host build, loaded directly from this task's own target directory. It does not qualify installed packaging, native GPU Pin/reclamation, producer/consumer stream execution, capture, the offload adapter or stock models. `gpu.py` is prepared for the coordinator and was not executed here. Frozen GPU artifacts are unchanged.
