# H3 attention receipts (Runtime 0.34.12 pre-roll, proof-only). Sourced by gate.sh --cells.
# route <cell>: the last route the run selected, "<kernel>[<dense rows>]" (a cold executor's first request also
# names the routes it served while kernels opened); served <cell>: the transformer kernel each GPU observed;
# exact_sites <cell>: the sites served exact; degree <cell>: the widest call's degree and its GPUs.
route() { jq -r '[.calls[]?.measurements.observations[]? | select(.fields.dense_reference and .value != "sdpa")] | sort_by(.at_unix_ms, .seq) | last | if . then "\(.value)[\(.fields.dense_reference)]" else "" end' "$1.json" 2>/dev/null; }
served() { jq -r '[.calls[]?.measurements.execution.gpus[]?.attention.observed // empty | split(",")[] | select(startswith("transformer"))] | unique | join(" ")' "$1.json" 2>/dev/null; }
exact_sites() { jq -r '[.calls[]?.measurements.observations[]? | select(.name == "attention.sol.kernel" and .value == "exact") | .fields.site] | unique | join(",")' "$1.json" 2>/dev/null; }
degree() { jq -r '[.calls[]?.measurements.execution // {} | select(.degree) | [.degree, (.gpus // [] | map(.uuid) | unique | length)]] | max_by(.[0]) // [1, 0] | @tsv' "$1.json" 2>/dev/null; }
# lora <cell>: how the denoise call served its LoRA: "baked" (bytes moved from baked pages, the bake room) or "per step".
lora() { jq -r '[.calls[]?.measurements.memory.stats.varena[]?.stats | select(.baked_pages) | {b: .moved.baked.bytes, p: .baked_pages.bytes, room: .baked_pages.room}] | max_by(.b + .p) // {b: 0, p: 0, room: null}
  | if .b + .p > 0 then "baked (\(.b / 1073741824 * 10 | floor / 10) GiB moved baked, room \(if .room > 0 then "\(.room / 1e9 * 10 | floor / 10) GB" else .room end))" else "per step (baked 0, room \(.room))" end' "$1.json" 2>/dev/null; }
# vae <cell>: the video VAE's attention kernel; decode_s <cell>: the decode_video stage's seconds.
vae() { jq -r '[.calls[]?.measurements.execution.gpus[]?.attention.observed // empty | split(",")[] | select(startswith("vae="))] | unique | join(" ")' "$1.json" 2>/dev/null; }
decode_s() { jq -r '[.. | objects | select(has("steps")) | .steps | objects | .decode_video.total_ms? // empty] | add // 0 | . / 1000 | floor' "$1.json" 2>/dev/null; }
