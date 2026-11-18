# kv-tiers (xs-213 phase 2, cr-336): on one rental of $SKU, the Runtime's own cells for the Caches an engine keeps K/V in, and
# the measurement that decides how K/V off the card is served. Each is a case of the varena suite's drive
# (suites/varena/drive `core`: a fresh process of the candidate Runtime, `cozy_runtime.bench.varena_core`):
#   host-cache     what the GPU answers about reading registered host memory in place (the attribute's value, whether a
#                  registered page sits at its own address, each call's refusal), and where it says yes a window committed,
#                  read and written by a kernel in place, copied to the card, released. Its probe line is reported first.
#   runtime-cache  cr-336 V2's card half: exact pages at a base that never moves, a view before any page, a refused commit
#                  that changes nothing, a release behind its stream, close only with no page and no tensor, a shrink
#                  callback that gives its pages to an allocation. A Region's Use export still reads its bytes.
#   kv-tiers       decode attention at Qwen3.5-9B's K/V layout over 65,536 tokens, every attention layer a step, with none,
#                  a quarter, half and all of the row's blocks off the card, read in place against copied through a window on
#                  the card: ms a step and steps/s (attention only: no weights are read, so this is the K/V's share of a
#                  token, not a token), the host GiB a step, the card's utilisation and power, and the link's copy GB/s. Every
#                  arm's output is held against a float32 reference before it is timed.
# The pod runs this session's cozy as its TensorD and the candidate TensorFS and Runtime (wheel sources as lib.sh `wheel_from`
# takes). Nothing here is judged against a bar: it reports, and fails only where a case does.
# params: tensorfs, runtime (required), seconds (1500).
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
DRIVE=$(cd "$HERE/../suites/varena/drive" && pwd)
TENSORFS=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs '')") && [ -n "$TENSORFS" ] || fail "no TensorFS wheel"
RUNTIME=$(wheel_from "$OUT/wheels/runtime" "$(param runtime '')") && [ -n "$RUNTIME" ] || fail "no Runtime wheel"
say "tensorfs ${TENSORFS##*/} · runtime ${RUNTIME##*/} · cozy $(cozy --version)"
# The drive's environment and lock, checked on the runner before anything is rented (fixtures/ggml-decode/preflight.sh).
. "$HERE/fixtures/ggml-decode/preflight.sh"
drive_check "$DRIVE" varena_drive
drive_lock "$DRIVE"
quote "$(param seconds 1500)"
rent
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod's TensorD"
step candidate machine update "$NAME" --tensorfs "$TENSORFS" --runtime "$RUNTIME" -y || fail "the pod's TensorFS and Runtime"
cozy machine show "$NAME" --json > "$OUT/show.json" 2>&1
say "on: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · " + ([.status.gpus[] | "\(.name) sm\(.compute_capability // "?")"] | join(", "))' "$OUT/show.json")"
(while sleep 480; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done) & BG="$BG $!"
pod_install stage-drive "$DRIVE"

# ran <case>: the case on the pod, its document in $OUT/<case>.json; a case that fails ends the session with its tail.
ran() {
  local n
  step "$1" run "$DRIVE/core" "case=$1" --on "$NAME" || { tail -c 3000 "$OUT/$1.txt" | tee -a "$R"; fail "$1"; }
  on_candidates "$1"
  n=$(runno "$1"); (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) 2> /dev/null | jq -r '.result.detail' > "$OUT/$1.json"
  jq -e '.rows[0]' "$OUT/$1.json" > /dev/null || fail "$1 returned no row: $(head -c 400 "$OUT/$1.json")"
}

ran host-cache
say "probe: $(jq -c '.rows[0].probe' "$OUT/host-cache.json")$(jq -r '.rows[0].host // empty | " · " + .' "$OUT/host-cache.json")"
ran runtime-cache
say "runtime-cache: $(jq -c '.rows[0] | del(.case)' "$OUT/runtime-cache.json")"
ran kv-tiers
{
  jq -r '.rows[0] | "K/V \(.layout.kv_gib) GiB (\(.layout.tokens) tokens, \(.layout.attn_layers) layers, \(.layout.parts) parts) · link: H2D \(.h2d_gb_s // "?") GB/s, D2H \(.d2h_gb_s // "?") GB/s\(if .host then " · host tier: " + .host else "" end)\(if .cleanup then " · cleanup: " + .cleanup else "" end)"' "$OUT/kv-tiers.json"
  echo
  echo "| off the card | served | ms a step | steps/s | host GiB a step | utilisation % | power W | worst error |"
  echo "|---|---|---|---|---|---|---|---|"
  jq -r '.rows[0].arms[] | "| \(.off_card) | \(.mode) | \(.ms_step * 100 | round / 100) | \(.steps_s * 100 | round / 100) | \(.host_gib_step * 100 | round / 100) | \(if .watched then (.utilization_pct | round) else "-" end) | \(if .watched then (.power_w | round) else "-" end) | \(.worst_error) |"' "$OUT/kv-tiers.json"
} > "$OUT/summary.md"
cat "$OUT/summary.md" >> "$R"; cat "$OUT/summary.md" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
say "PASS: the Caches' cells and the K/V serving measurement on $(jq -r '.status.gpus[0].name' "$OUT/show.json")"
