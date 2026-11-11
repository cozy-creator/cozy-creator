# Suite `varena` (gate.sh --suite varena): the memory manager through its paces on three representative models, SDXL
# (small), Klein (mid) and H3 (large, 5 s), on the architectures in ./skus. Not every package: the runtime and
# packages suites hold those. Sourced in gate.sh's shell: `cell`, `pkg`, `log`, $CLI, $M, $R, $REF, $SKU, $HERE and
# the scenario's $CHAR $MOTION $OBJECT $SCENE. Every request has its own prompt and seed.
#
#   ceiling          this machine's pinned host-to-device and cold disk-read bandwidth (ceiling.py)
#   cold-*           each model loaded from disk into a fresh process
#   warm-sdxl        the next call: a graph must serve it (check.py)
#   co-*             all three again on one card: what does not fit is evicted and refilled
#   reload-*         after an unload (a restart ends every executor), each model back from page cache or disk
#   s8-* s6-* s3-*   under 8, 6 and 3 GiB software caps; at 6 GiB SDXL and Klein alternate three times
#   (≤ 8 GB card)    cold, co and reload only: the driver's own out-of-memory paths (a capped card never fills)
#   h3-8gb           (≤ 8 GB card) H3 Turbo 15 s, the 8 GB tier's product shape
#   host-*           on a CPU rental, under its cgroup's memory.max (host-cap.py): pages must spill to disk
#   core-*           (every GPU card, last) the memory authority's rules, one fresh process per mode (programs/); the
#                    rules only the Runtime's own varena adapter reaches, one fresh process per case (drive/, built-ins)
#   offload-*        vt.offload's promises, and SDXL through it beside stock Torch (programs/, --env diffusers); inside
#                    an executor (drive/, a built-in)
#   public-*         C, Rust and Python clients on one core; sizing and Region priority (public-api/, a program)
#   fault-*          deliberate faults: correct bytes or a typed error, the GPU recovered, no hang, no Xid (faults.sh)
#   diff-*           real models repeat byte for byte, under a cap and with memory taken and perturbed (differential/cell.sh)
#   memcheck         compute-sanitizer finds nothing in SDXL through vt.offload's graphs that stock doesn't have (memcheck.sh)
#
# check.py judges caps, host RAM, alternation and load/refill bandwidth as a share of `ceiling`; analyze.py, time
# and peak VRAM against the last pass. A cell named in known-failing.json's `cells` for this SKU or "*" is skipped.
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
SDXL=$(pkg sdxl generate) KLEIN=$(pkg flux.3 generate_turbo) H3=$(pkg minimax-h3 generate)
# One character picture, as a project sends it: no model but H3's.
H3REF="[{\"name\": \"Lead\", \"kind\": \"character\", \"image\": \"$REF\"}]"
h3() { vcell "$1" "$H3" references:="$H3REF" prompt="<Lead> $MOTION $4" seed:=$((R + $2)) duration_s:="$3"; }
ln -sfn "$V/ceiling.py" ceiling.py; ln -sfn "$V/host-cap.py" host-cap.py
# After every model cell, as a fault may leave the card unusable. A restart first: no executor stays on the card.
authority() {
  local mode i=0 rule
  cap off
  # The memory authority's rules: a program on the rental over its public API, not a package (programs.sh, xs-174).
  programs core "$V/programs" varena_core.py
  # The rules only the Runtime's own varena adapter reaches (weights Regions with staging pages and a derive, phased
  # Plans, cache owners, capture guards) and vt.offload inside an executor: Runtime built-ins (`bench.varena_core`,
  # `bench.offload_in_executor`) that drive/ calls (xs-174).
  for rule in abandoned-fill pending-working-before-weights split-waits-and-caps fill-keeps-phase-pages eviction-order \
      prefetch-rule fill-paths-agree owner-wait-physical owner-wait-capped short-holders-wait capture-refuses-a-fill; do
    vcell "core-$rule" "$V/drive/core" case="$rule" --gpus=1
  done
  vcell offload-in-executor "$V/drive/offload_in_executor" --gpus=1
  # vt.offload's promises and SDXL through it beside stock Torch: programs over the public API, in the Diffusers
  # environment (programs.sh --env diffusers, xs-174). Rows offload-adapter, offload-stock/eager/graphs.
  programs offload "$V/programs" --env diffusers varena_offload.py
  # The public API across bindings: a program on the rental (programs.sh), not a package (xs-174). Rows public-*.
  "$V/public-api/pack.sh" "$OUT/varena-public" && programs public "$OUT/varena-public" public_api.py
  . "$V/faults.sh"
  . "$V/differential/cell.sh"
  . "$V/memcheck.sh"
}

if ! grep -q '^GPU 0' machine.txt; then
  vcell host-cap ./host-cap.py
  # A CPU step is about 2 min on a busy host: four show its memory, twenty cost $0.60.
  vcell host-sdxl "$SDXL" prompt="$SCENE, photo" seed:=$((R + 51)) steps:=4
  vcell host-klein "$KLEIN" prompt="$CHAR, studio portrait" seed:=$((R + 52))
  return 0
fi
vcell ceiling ./ceiling.py
vcell cold-sdxl "$SDXL" prompt="$SCENE, photo" seed:=$((R + 1))
vcell warm-sdxl "$SDXL" prompt="$SCENE, photo, late light" seed:=$((R + 16))
vcell cold-klein "$KLEIN" prompt="$CHAR" seed:=$((R + 2))
h3 cold-h3 3 5 ""
vcell co-sdxl "$SDXL" prompt="$OBJECT on a wooden table, product photo" seed:=$((R + 4))
vcell co-klein "$KLEIN" prompt="$CHAR, close-up portrait, soft window light" seed:=$((R + 5))
h3 co-h3 6 5 "The camera slowly pushes in."
cap off  # unload
vcell reload-sdxl "$SDXL" prompt="$SCENE at dusk, wide shot, photo" seed:=$((R + 7))
vcell reload-klein "$KLEIN" prompt="$OBJECT, macro photo, shallow depth of field" seed:=$((R + 8))
if [ "$(grep -oE '· [0-9]+ GiB ·' machine.txt | head -1 | grep -oE '[0-9]+')" -le 8 ]; then
  # The 8 GB tier's product shape, H3 Turbo 15 s (1344x768 with audio): 0.35.0 ran out of memory in its step 2.
  vcell h3-8gb "$H3" prompt="$MOTION Wide, steady." seed:=$((R + 15)) duration_s:=15
  authority
  return 0
fi
h3 reload-h3 9 5 "A slow pan follows the action."
cap 8GiB
vcell s8-sdxl "$SDXL" prompt="$OBJECT in a museum display case, photo" seed:=$((R + 11))
vcell s8-klein "$KLEIN" prompt="$SCENE, watercolor" seed:=$((R + 12))
h3 s8-h3 13 5 "Handheld, close."
h3 s8-h3-15 14 15 "Wide, steady."
cap 6GiB
for i in 1 2 3; do
  vcell s6-sdxl-$i "$SDXL" prompt="$CHAR in $SCENE, film still $i" seed:=$((R + 20 + i))
  vcell s6-klein-$i "$KLEIN" prompt="$OBJECT, studio photo, angle $i" seed:=$((R + 30 + i))
done
cap 3GiB
vcell s3-sdxl "$SDXL" prompt="$CHAR, oil painting" seed:=$((R + 41))
vcell s3-klein "$KLEIN" prompt="an aerial photo of $SCENE" seed:=$((R + 42))
authority
