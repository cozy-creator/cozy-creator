# child-lora-stacks (cl-687)

`compare` calls sdxl's public `generate` (a dependency's function) six times under the request's `--lora balloon`:
inherited, `Lora(balloon, 0)`, `Lora(ikea, 0.8)`, `Lora(balloon, 0.5)`, inherited at the next seed, and the first
request again. The cell passes when the calls' records show `[balloon 1]`, `[]`, `[balloon 1, ikea 0.8]`,
`[balloon 0.5]` and `[balloon 1]`, the last call is memoized, and the five images landed with calls 0–3 distinct.
Proof runs 20647, 20654 and 20671 (rtx-3060). sdxl (0.9 or newer, the releases on Runtime 0.45) comes from the Hub's root index through `{ index = "tensorhub" }`, locked by cozy at the run (cl-871).
