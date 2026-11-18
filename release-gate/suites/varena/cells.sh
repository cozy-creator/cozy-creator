# Suite `varena` (gate.sh --suite varena): the memory manager's hardest cases on the cards in ./skus. Sourced in gate.sh's
# shell: `cell`, `pkg`, `log`, $CLI, $M, $R, $REF, $SKU, $HERE and the scenario's $CHAR $MOTION $OBJECT $SCENE. Every request
# has its own prompt and seed.
#
#   cold-h3          H3 (5 s, with a character reference) loaded from disk into a fresh process
#   co-*             SDXL and Klein beside it on one card, then H3 again: what does not fit is evicted and refilled
#   h3-8gb           (≤ 8 GB card) H3 Turbo 15 s, the 8 GB tier's product shape
#   diff-sdxl-*      SDXL repeats byte for byte: warm, with memory unmapped and perturbed, under a 6 GiB cap, with a kept
#                    graph's weights gone (refill) and with no room for them (noroom), and with graphs off (differential/cell.sh)
#
# check.py judges each H3 video and backing left without a page; analyze.py, time and peak VRAM against the last pass. A
# cell named in known-failing.json's `cells` for this SKU or "*" is skipped.
V=$HERE/suites/varena
parked() {
  python3 -c 'import json, sys
sys.exit(0 if any(sys.argv[3] in k.get("cells", []) and k["sku"] in (sys.argv[2], "*") for k in json.load(open(sys.argv[1]))) else 1)' \
    "$HERE/known-failing.json" "$SKU" "$1"
}
vcell() { if parked "$1"; then log "$1 parked (known-failing.json)"; else cell "$@"; fi; }
cap() {
  "$CLI" machine update "$M" --gpu-memory-limit "$1" -y >"cap-$1.log" 2>&1
  local i said want=0
  [ "$1" = off ] || want=$(( ${1%GiB} << 30 ))
  for i in $(seq 12); do  # the restarted machine reports its limit once it is up
    "$CLI" --json machine show "$M" >"cap-$1.show.json" 2>/dev/null
    said=$(jq -r '[.status.gpus[]? | .memory_limit_bytes // 0 | tostring] | unique | join(",")' "cap-$1.show.json" 2>/dev/null)
    [ "$said" = "$want" ] && break
    sleep 5
  done
  log "cap $1 on $M: machine show says ${said:-nothing} bytes; $(grep -E '^status' "cap-$1.log")"
  [ "$said" = "$want" ] || expect "cap-$1" FAIL "machine show says ${said:-nothing} bytes, not $want"
}
SDXL=$(pkg sdxl generate) KLEIN=$(pkg flux.3-klein generate_turbo) H3=$(pkg minimax-h3 generate)
# One character picture, as a project sends it: no model but H3's.
H3REF="[{\"name\": \"Lead\", \"kind\": \"character\", \"image\": \"$REF\"}]"
h3() { vcell "$1" "$H3" references:="$H3REF" prompt="<Lead> $MOTION $4" seed:=$((R + $2)) duration_s:="$3"; }
h3 cold-h3 3 5 ""
vcell co-sdxl "$SDXL" prompt="$OBJECT on a wooden table, product photo" seed:=$((R + 4))
vcell co-klein "$KLEIN" prompt="$CHAR, close-up portrait, soft window light" seed:=$((R + 5))
h3 co-h3 6 5 "The camera slowly pushes in."
if [ "$(grep -oE '· [0-9]+ GiB ·' machine.txt | head -1 | grep -oE '[0-9]+')" -le 8 ]; then
  cap off  # unload
  # The 8 GB tier's product shape, H3 Turbo 15 s (1344x768 with audio): 0.35.0 ran out of memory in its step 2.
  vcell h3-8gb "$H3" prompt="$MOTION Wide, steady." seed:=$((R + 15)) duration_s:=15
fi
# Last, after a restart: no executor stays on the card.
cap off
. "$V/differential/cell.sh"
