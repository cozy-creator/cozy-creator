# Public ABI mixed-binding proof

[Contract and examples](../../../../crates/varena/PUBLIC-API.md).

`prove-host.sh OUTPUT` builds actual C, C++, Rust and TensorFS clients into a new
durable output directory. It never initializes CUDA. `mixed.py` defaults to this
host boundary; `--gpu` is only for the coordinating GPU owner's ordinary Cozy lane.
The C and Rust libraries load simultaneously with the actual TensorFS extension,
and the proof runs native-first and Python-first in separate processes.

GPU invocation after the owner builds the matching Torch shim:

```sh
python mixed.py --c <output>/mixed-c.so --rust target/debug/libvarena_mixed_client.so \
  --order native-first --gpu
python mixed.py --c <output>/mixed-c.so --rust target/debug/libvarena_mixed_client.so \
  --order python-first --gpu
```

Record the exact SDK/wheel/native hashes, driver, Torch/CUDA shim, device UUID and
both receipts. Follow with the native engine, Rust, Python and cache examples.
Retain failed receipts. Host linking or a shared identity alone is not shared GPU
allocation/reclamation proof; no GPU result has been recorded by the P4 owner.
