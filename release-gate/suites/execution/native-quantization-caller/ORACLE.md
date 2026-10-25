# Independent historical oracle

`ci_native_quantization_caller/oracle.py` ships inside this standalone gate package; it needs no parent-directory
import, old wheel, private interpreter or current production numeric encoder. Its producer/helper
function bodies are copied verbatim from Runtime commit `1c092cd8474f1a3d042d9e1ee69d61410e1adb04`:

- `src/cozy_runtime/derive/microscale.py`: SHA256 `3171a1d9ddfa01b793cd2b9f96f6826493a13c53c465722d2b89ab1197e5b99b`
- `src/cozy_runtime/derive/safetensors_io.py`: SHA256 `f5ef98c05ff4bcc00b5a8689323f215b088ee181370f02bf8a4172025344dd59` (row_chunks only)

Oracle file SHA256 `2576e23a37f96a7e129c4c0b10d54facb228e6ac45a7fad0ef973d00f9c64a61`. Names/types and public `UnsupportedInput` remain unchanged.
Reference code is gate-only and is never a serving fallback. Arithmetic changes require explicit
review against preserved receipts; updating the installed Runtime must not silently update the oracle.
