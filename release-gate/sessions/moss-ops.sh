# moss-ops (proof-only, never merge): kernels/moss_ops.py on one rental, MOSS-Image's DiT block op by op, the Runtime's
# ops against Comfy Kitchen's, twice: with the Runtime's reviewed comfy-kitchen build, then with the comfy-kitchen
# release ComfyUI ran in the benchmark (a copy of the script whose dependencies pin it). params:
#   update     `cozy machine update` flags (["--runtime", "0.35.6", "--tensorfs", "0.17.13"])
#   kitchen    the second run's comfy-kitchen release (0.2.40)
#   traffic_gb GB the provider bills (10); need_s seconds the guard must buy (1200)
. "$(dirname "$0")/lib.sh"
mapfile -t update < <(param update '["--runtime", "0.35.6", "--tensorfs", "0.17.13"]' | jq -r '.[]')
KITCHEN=$(param kitchen 0.2.40) DIR=$(dirname "$0")/kernels
[[ $KITCHEN =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] && [ -f "$DIR/moss_ops.py" ] || fail "kitchen or kernels/moss_ops.py"
sed "s/\"torch>=2.14\"]/\"torch>=2.14\", \"comfy-kitchen==$KITCHEN\"]/" "$DIR/moss_ops.py" > "$DIR/moss_ops_kitchen.py"
grep -q "comfy-kitchen==$KITCHEN" "$DIR/moss_ops_kitchen.py" || fail "the pinned copy"
quote "$(param traffic_gb 10)" "$(param need_s 1200)"
rent
step update machine update "$NAME" "${update[@]}" -y || fail "update"
cozy machine show "$NAME" 2>&1 | grep -E '^(runtime|tensorfs|GPU) ' | tee -a "$R"
cd "$DIR" || fail "no kernels/"
for arm in moss_ops moss_ops_kitchen; do
  step "$arm" run "./$arm.py" --gpus=1 --on "$NAME" || say "WARNING $arm: $(tail -c 700 "$OUT/$arm.txt" | tr '\n' ' ')"
  n=$(runno "$arm"); [ -z "$n" ] || cozy run show "$n" --once --json > "$OUT/$arm.json" 2>&1
  say "$arm: $(jq -r '.result.value // .status' "$OUT/$arm.json" 2>/dev/null | head -c 3000)"
done
for r in "${RIDS[@]}"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1 || cozy machine end "$r" -y --force >> "$OUT/end.txt" 2>&1; done
say "END requested after the last run"
for arm in moss_ops moss_ops_kitchen; do jq -e '.result.value' "$OUT/$arm.json" >/dev/null 2>&1 || fail "$arm returned no result"; done
say "PASS"
