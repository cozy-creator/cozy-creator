# reliability: an idle executor's process ends for room when only that frees it (#2174/#2181 `ended_for_room`).
# One rental, driven from a runner; the session's cozy carries the candidate TensorD onto the pod. On an 8 GB card an
# idle holder (fixtures/gpu-hold: 2.5 GB of package-global GPU memory, which offloading weights cannot free) and SDXL
# do not co-fit. SDXL runs once first so its need is measured (a process ends only for a known need); then the holder;
# then SDXL again must end the holder's process to fit; then the holder runs again in a fresh process. params:
#   hold_mib    MiB the holder keeps (default 2560)
#   traffic_gb  GB the provider bills: the image, SDXL and the holder's environment (default 12)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures" && pwd)
MIB=$(param hold_mib 2560)
status() { cozy run show "$(runno "$1")" --once --json > "$OUT/$1.json" 2>&1; jq -r '.status // empty' "$OUT/$1.json"; }
holder_pid() { jq -r '.result.pid // (.result | fromjson? | .pid) // empty' "$OUT/$1.json"; }
[ -f "$FIX/gpu-hold/gpu_hold/__init__.py" ] || fail "no fixture checkout"
quote "$(param traffic_gb 12)" 2400
rent

step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
[ "$v" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not the session's"
say "pod TensorD $v"

cd "$FIX" || fail "no fixtures"
step warm run sdxl/generate --on "$NAME" prompt="a red barn in snow, photo" seed:=$RANDOM || fail "sdxl warm-up"
[ "$(status warm)" = completed ] || fail "the warm-up did not complete"
step hold1 run ./gpu-hold/hold --on "$NAME" seed:=$RANDOM mib:="$MIB" || fail "holder 1"
[ "$(status hold1)" = completed ] || fail "holder 1 did not complete"
base=$(cozy machine logs "$NAME" memory -n 100000 2>/dev/null | wc -l)
step big run sdxl/generate --on "$NAME" prompt="a quiet harbor at dawn, photo" seed:=$RANDOM || fail "sdxl"
[ "$(status big)" = completed ] || fail "sdxl did not complete beside the holder"
step hold2 run ./gpu-hold/hold --on "$NAME" seed:=$RANDOM mib:="$MIB" || fail "holder 2"
[ "$(status hold2)" = completed ] || fail "holder 2 did not complete"

cozy machine logs "$NAME" memory -n 100000 > "$OUT/memory.log" 2>&1
tail -n +"$((base + 1))" "$OUT/memory.log" > "$OUT/memory-window.log"
ended=$(grep -c '"event":"ended_for_room"' "$OUT/memory-window.log")
need=$(grep '"event":"ended_for_room"' "$OUT/memory-window.log" | head -1 | grep -oE '"need":[0-9a-z]+' | cut -d: -f2)
h1=$(holder_pid hold1) h2=$(holder_pid hold2)
say "ended_for_room=$ended (need ${need:-?}); holder pid $h1 -> $h2"
[ "$ended" -ge 1 ] || fail "SDXL did not end the holder for room: $(grep -E '"event":"offload"' "$OUT/memory-window.log" | head -2 | cut -c1-300)"
[[ ${need:-null} =~ ^[0-9]+$ ]] || fail "the holder was ended for an unknown need ($need)"
{ [ -n "$h1" ] && [ -n "$h2" ] && [ "$h1" != "$h2" ]; } || fail "the holder did not come back in a fresh process ($h1 -> $h2)"
say "PASS: SDXL's measured need ($need B) ended the idle holder's process ($h1) to fit; the holder then ran fresh ($h2)"
