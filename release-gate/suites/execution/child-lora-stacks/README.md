# child-lora-stacks (cl-687)

`compare` calls sdxl's public `generate` (a dependency's function) six times under the request's `--lora balloon`:
inherited, `Lora(balloon, 0)`, `Lora(ikea, 0.8)`, `Lora(balloon, 0.5)`, inherited at the next seed, and the first
request again. The cell passes when the calls' records show `[balloon r]`, `[]`, `[balloon r, ikea 0.8]`,
`[balloon 0.5]` and `[balloon r]` (r: the request's strength, the LoRA's recommended one, xs-183), the last call is memoized, and the five images landed with calls 0–3 distinct.
Earlier proof runs 20647, 20654 and 20671 (rtx-3060) predate the typed-call API. The caller calls SDXL (an ordinary
dependency from the Hub's root index, locked by cozy at the run, cl-871) through the generated `sdxl_calls.py`, passing
each selection as `models={"model": selection}`; regenerate it with `cozy-runtime --dir . calls sdxl ...`.
