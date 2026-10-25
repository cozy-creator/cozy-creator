# Independent historical oracle

`ci_convrot_encode/oracle.py` ships inside this standalone gate package; it needs no parent-directory
import, old wheel, private interpreter or current production numeric encoder. Its producer/helper
function bodies are copied verbatim from Runtime commit `1c092cd8474f1a3d042d9e1ee69d61410e1adb04`:

- `src/cozy_runtime/derive/microscale.py`: SHA256 `3171a1d9ddfa01b793cd2b9f96f6826493a13c53c465722d2b89ab1197e5b99b`
- `src/cozy_runtime/derive/safetensors_io.py`: SHA256 `f5ef98c05ff4bcc00b5a8689323f215b088ee181370f02bf8a4172025344dd59` (row_chunks only)

Oracle file SHA256 `7db0ef53e49d66327e6a8ccd90e474b2fe54f712c4fd52b7fc5c6111b0099494`. Names/types and public `UnsupportedInput` remain unchanged.
Reference code is gate-only and is never a serving fallback. Arithmetic changes require explicit
review against preserved receipts; updating the installed Runtime must not silently update the oracle.
