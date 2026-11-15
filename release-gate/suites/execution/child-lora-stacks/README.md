# child-lora-stacks (cl-687)

`compare` calls sdxl's public `generate` (a dependency's function) six times under the request's `--lora balloon`:
inherited, `Lora(balloon, 0)`, `Lora(ikea, 0.8)`, `Lora(balloon, 0.5)`, inherited at the next seed, and the first
request again. The cell passes when the calls' records show `[balloon 1]`, `[]`, `[balloon 1, ikea 0.8]`,
`[balloon 0.5]` and `[balloon 1]`, the last call is memoized, and the five images landed with calls 0–3 distinct.
Earlier proof runs 20647, 20654 and 20671 (rtx-3060) predate the typed-call migration.
The maintained fixture uses a generated `Operation` and passes each selection through
`models={"model": selection}`. SDXL 0.11.0rc2 or newer is a `cozy-calls` dependency from
the Hub's root index; the CPU caller imports no SDXL implementation or local compute stack.
Regenerate `ci_child_lora_stacks/sdxl_calls.py` with `cozy-runtime --dir . calls sdxl ...`
from the matching locked dependency before qualifying a changed SDXL interface.
The current reference was generated with Runtime `87c0db669` from packages `fa7d5146`
(SDXL `0.11.0rc2`); its model slot is still `model`, including explicit `None` inheritance.
