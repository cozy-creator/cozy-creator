# lora-parked: the lora-overlay-refill cell (Qwen-Image-Edit-2511 int8 + Lightning LoRA armed in the DiT forward) on one
# rental, once per named Runtime in order: its verdict, and each warm call's DiT block fills (a switch back to an
# overlay whose pages stayed parked refills nothing). Proof-only (rc/baked-room-proof-20261027). params:
#   runtimes   [{"name", "runtime": "<URL#sha256=…>", "seed"}], in order; tensorfs "<URL#sha256=…>" (with the first)
#   rounds     the cell's warm rounds (2); traffic_gb (60); need_s (2400)
. "$(dirname "$0")/lib.sh"
CELL=$(cd "$(dirname "$0")/lora-overlay-refill" && pwd)
fetch() {
  local f=$OUT/wheels/$(basename "${1%%#*}" | sed 's/%2B/+/g')
  [[ $1 == https://*#sha256=* ]] || { echo "$1 is not https://…#sha256=" >&2; return 1; }
  mkdir -p "$OUT/wheels"
  [ -f "$f" ] || curl -fsSL --retry 5 --retry-all-errors "${1%%#*}" -o "$f" || { echo "cannot fetch ${1%%#*}" >&2; return 1; }
  [ "$(sha256sum "$f" | cut -d' ' -f1)" = "${1##*#sha256=}" ] || { echo "$f does not hash to ${1##*#sha256=}" >&2; return 1; }
  echo "$f"
}
version() { python3 -I -c 'import sys, zipfile; z = zipfile.ZipFile(sys.argv[1]); m = next(n for n in z.namelist() if n.endswith(".dist-info/METADATA")); print(next(l.split(": ", 1)[1] for l in z.read(m).decode().splitlines() if l.startswith("Version: ")))' "$1"; }
RTS=$(param runtimes '[]'); ROUNDS=$(param rounds 2)
mapfile -t names < <(jq -r '.[].name' <<<"$RTS")
[ "${#names[@]}" -gt 0 ] || fail "no runtimes"
declare -A WHL VER
for i in "${!names[@]}"; do
  k=${names[i]}; WHL[$k]=$(fetch "$(jq -r ".[$i].runtime" <<<"$RTS")") || fail "runtime $k"; VER[$k]=$(version "${WHL[$k]}")
  say "runtime $k: ${VER[$k]}, seed $(jq -r ".[$i].seed" <<<"$RTS")"
done
TFS=$(fetch "$(param tensorfs)") || fail "tensorfs"
say "tensorfs $(version "$TFS"); tensord $(cozy --version); cell $(git -C "$CELL" rev-parse --short HEAD 2>/dev/null)"
printf 'run 5 · sync local/x (source changed)\nrun 6 · local/x/gpu\nrun      6\n' > "$OUT/probe-cell.txt"
[ "$(runno probe-cell)" = 6 ] || fail "parsers"
quote "$(param traffic_gb 60)" "$(param need_s 2400)"
rent
grep -oE '· [0-9]+ GiB ·' "$OUT/show.txt" | head -1 | grep -qE '· ([3-9][0-9]|[1-9][0-9]{2}) GiB ·' || fail "the cell needs a card of 30 GiB or more"
for i in "${!names[@]}"; do
  k=${names[i]}
  if [ "$i" = 0 ]; then
    step "update-$k" --json machine update "$NAME" --runtime "${WHL[$k]}" --tensorfs "$TFS" --tensord "$(command -v cozy)" -y || fail "$k did not go on"
  else
    step "update-$k" --json machine update "$NAME" --runtime "${WHL[$k]}" -y || fail "$k did not go on"
  fi
  cozy machine show "$NAME" > "$OUT/machine-$k.txt" 2>&1
  grep -E '^runtime ' "$OUT/machine-$k.txt" | grep -qF "${VER[$k]%%+*}" || fail "$NAME does not run ${VER[$k]}"
  step "$k" run "$CELL/gpu" seed:="$(jq -r ".[$i].seed" <<<"$RTS")" rounds:="$ROUNDS" --gpus=1 --lane int8 --attention-kernel sdpa --on "$NAME"
  n=$(runno "$k"); [ -n "$n" ] || fail "$k: no run number"
  cozy run show "$n" --once --json > "$OUT/$k.json" 2>/dev/null
  st=$(jq -r '.status // "none"' "$OUT/$k.json")
  say "$k (${VER[$k]}) run $n $st"
  python3 -I - "$OUT/$k.json" <<'PY' | tee -a "$R"
import json, sys
run = json.load(open(sys.argv[1]))
def strings(o):
    if isinstance(o, dict):
        for v in o.values(): yield from strings(v)
    elif isinstance(o, list):
        for v in o: yield from strings(v)
    elif isinstance(o, str): yield o
doc = next((json.loads(s) for s in strings(run.get("result")) if '"failures"' in s), None)
if doc is None:
    print("    no cell report:", json.dumps(run.get("reason"))[:600]); sys.exit()
print("    passed", doc.get("passed"), "failures", doc.get("failures"), "lora_effect", doc.get("lora_effect"), "bake_error", doc.get("bake_error"))
for i, row in enumerate(doc.get("warm") or doc.get("calls") or []):
    print(f"    warm {i}: {row.get('kind')} paging={row.get('paging')} fills={row.get('block_fills')} resident_at_start={row.get('resident_at_start')} baked={row.get('baked')}")
PY
  [ "$st" = completed ] || cozy machine logs "$NAME" -n 300 > "$OUT/$k.tensord.log" 2>&1
done
