# row-reads (tfs-303): row ranges of real checkpoints' weights, one per TensorFS lane, read with `Checkpoint.read_rows` and a
# varena Source's `read_rows` on one rental, against whole reads (fixtures/row-reads/rows.py): each part byte for byte, each
# encoded range's decode, host and CUDA outs, alone and batched, and the largest tensor's read speed against its whole read.
# Each model's lane is resolved to its checkpoint before renting (`cozy model show`), so a missing lane costs nothing.
# params:
#   tensorfs    the TensorFS under test: artifact:<dev-build run>/<wheel artifact> or an https wheel (required)
#   runtime     the Runtime beside it: a cozy-runtime dev-build run id or an https wheel (required)
#   models      [{"model": "org/name@release", "lane": "int8"}, ...] (required)
#   per_scheme  tensors checked per scheme in each checkpoint (default 6)
#   need_s      seconds the guard must buy (default 2400)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/row-reads/rows.py
[ -f "$FIX" ] || fail "no fixtures/row-reads"
for tool in jq python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no dev build: dispatch with cozy=<a dev-build run of the change>"
MODELS=$(param models '[]') PER=$(param per_scheme 6)
[[ $PER =~ ^[0-9]+$ ]] || fail "per_scheme $PER"
RT=$(wheel_from "$OUT/wheels/runtime" "$(param runtime)") TF=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs)")
[ -n "$RT" ] && [ -n "$TF" ] || fail "no Runtime or TensorFS wheel (params runtime, tensorfs)"
say "wheels: ${RT##*/}, ${TF##*/}"
COUNT=$(jq length <<<"$MODELS")
[ "$COUNT" -gt 0 ] || fail "no models"
REFS=() MISSING=()
for i in $(seq 0 $((COUNT - 1))); do
  m=$(jq -r ".[$i].model" <<<"$MODELS") lane=$(jq -r ".[$i].lane" <<<"$MODELS")
  if ! cozy --json model show "$m" > "$OUT/show-$i.json" 2> "$OUT/show-$i.err"; then
    MISSING+=("$m: $(tail -c 200 "$OUT/show-$i.err")"); continue
  fi
  say "$m: lanes $(jq -r '[.lanes[] | "\(.lane) \(.checkpoint)"] | join(", ")' "$OUT/show-$i.json")"
  ck=$(jq -r --arg l "$lane" 'first(.lanes[] | select(.lane == $l) | .checkpoint) // empty' "$OUT/show-$i.json")
  if [ -n "$ck" ]; then REFS+=("${m%@*}@$ck|$lane"); else MISSING+=("$m has no $lane lane"); fi
done
[ "${#MISSING[@]}" = 0 ] || fail "$(printf '%s; ' "${MISSING[@]}")"
quote "$(param need_s 2400)"
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$RT" --tensorfs "$TF" -y || fail "the update"
say "pod: $(cozy machine show "$NAME" --json | jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · TensorFS \(.status.tensorfs) · GPU 0 \(.status.gpus[0].name)"')"
SEED=$(date +%s) k=0
for r in "${REFS[@]}"; do
  ref=${r%|*} lane=${r#*|} k=$((k + 1))
  label=rows-$k-$lane
  if ! step "$label" run "$FIX/check" "model.weights=$ref" "seed:=$((SEED + k))" "per_scheme:=$PER" --on "$NAME"; then
    n=$(runno "$label")
    [ -z "$n" ] || (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$label.json" 2> "$OUT/$label.err"
    say "FAIL $label ($ref), run ${n:-none}: $(tail -n 3 "$OUT/$label.txt" | paste -sd' ' | cut -c1-300) $(jq -c '.error // empty' "$OUT/$label.json" 2> /dev/null | cut -c1-900)"
    RC=1; continue
  fi
  n=$(runno "$label")
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$label.json" 2> "$OUT/$label.err"
  jq -r '.result.report // empty' "$OUT/$label.json" > "$OUT/$label.report.json"
  jq -e . "$OUT/$label.report.json" > /dev/null 2>&1 || { say "FAIL $label: run $n has no report"; RC=1; continue; }
  # The call memo answers a repeated request with no read made (as ab.sh's cellstate tells it): such a run proves nothing.
  memo=$(jq -r '(.memo.memoized // false) or ([.calls[]?.memoized] | any)' "$OUT/$label.json")
  say "$label: run $n, $ref, memoized $memo"
  [ "$memo" = false ] || { say "FAIL $label: run $n was answered from the call memo"; RC=1; continue; }
  jq -r '.lines[]' "$OUT/$label.report.json" | while read -r line; do say "$label $line"; done
  say "$label speed: $(jq -c .speed "$OUT/$label.report.json")"
  [ "$(jq -r .failed "$OUT/$label.report.json")" = false ] || RC=1
done
{ echo "## row-reads on $NAME ($SKU)"; grep -E ' rows-[0-9]+-' "$R" | sed 's/^/- /'; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
exit "$RC"
