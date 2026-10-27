# exact-need: a reused resident construction is charged only what it lacks when TensorD makes room (#2181: its measured
# peak less what the driver already charges its process), so an idle neighbour is not evicted before every call. One
# rental, a red/green pair on one 16 GB pod, driven from a runner:
#   red   the pod's baked TensorD (the last release, charging a reuse its whole peak);
#   green the session's cozy (the candidate), put on the pod with `machine update --tensord`.
# Each phase warms BiRefNet and SDXL so both peaks are known and both executors resident, then reuses SDXL (the
# decision point) and runs BiRefNet again on a fresh input. At the decision point red's make_room offloads BiRefNet;
# green's does not. If the reuse then truly outgrows the card, green's in-call yield (`running_call`, the backstop) may
# still offload BiRefNet mid-call: reported, not failed. Green never ends BiRefNet's process for a reuse (same pid). If
# red's make_room offloads nothing, the pair cannot discriminate: that fails as inconclusive. params:
#   traffic_gb  GB the provider bills: the image, SDXL and BiRefNet (default 14)
. "$(dirname "$0")/lib.sh"
recpid() { jq -r 'first(.measurements.execution.gpus[]?.pid) // empty' "$OUT/$1.json"; }
store_bytes() { jq -r '[.measurements.memory.stats.varena[]?.stats.moved.store.bytes // 0] | add // 0' "$OUT/$1.json"; }
show() { cozy run show "$(runno "$1")" --once --json > "$OUT/$1.json" 2>&1; jq -r '.status // empty' "$OUT/$1.json"; }
quote "$(param traffic_gb 14)" 2400
rent
IMG=$REF/seed.jpg

# phase <name>: warm both, then the decision point (SDXL reused) and BiRefNet again; the memory log window is its own.
phase() {
  local p=$1 base
  step "$p-b1" run birefnet/remove_background --on "$NAME" --asset image="$IMG" || fail "$p birefnet 1"
  step "$p-s1" run sdxl/generate --on "$NAME" prompt="a lighthouse in fog, photo, $p" seed:=$RANDOM --out "$OUT/$p-s1" || fail "$p sdxl 1"
  local warm; warm=$(ls "$OUT/$p-s1"/*.webp "$OUT/$p-s1"/*.png 2>/dev/null | head -1)
  step "$p-b2" run birefnet/remove_background --on "$NAME" --asset image="$warm" || fail "$p birefnet 2"
  for s in b1 s1 b2; do [ "$(show "$p-$s")" = completed ] || fail "$p-$s did not complete"; done
  # The decision point's window is the memory log past this line count (the pod's clock, not the runner's).
  base=$(cozy machine logs "$NAME" memory -n 100000 2>/dev/null | wc -l)
  step "$p-s2" run sdxl/generate --on "$NAME" prompt="a desert road at noon, photo, $p" seed:=$RANDOM --out "$OUT/$p-s2" || fail "$p sdxl 2"
  local fresh; fresh=$(ls "$OUT/$p-s2"/*.webp "$OUT/$p-s2"/*.png 2>/dev/null | head -1)
  step "$p-b3" run birefnet/remove_background --on "$NAME" --asset image="$fresh" || fail "$p birefnet 3"
  for s in s2 b3; do [ "$(show "$p-$s")" = completed ] || fail "$p-$s did not complete"; done
  cozy machine logs "$NAME" memory -n 100000 > "$OUT/$p-memory.log" 2>&1
  # make_room's own offloads and ends at the decision point (its `for` is a construction), apart from the in-call
  # yield's (`running_call`), which only answers a call that truly outgrows the card.
  read -r START[$p] YIELD[$p] ENDED[$p] < <(tail -n +"$((base + 1))" "$OUT/$p-memory.log" | python3 -c '
import json, sys
start = yielded = ended = 0
for line in sys.stdin:
    _, _, body = line.partition("{")
    try:
        e = json.loads("{" + body)
    except ValueError:
        continue
    if e.get("failed"):
        continue
    if e.get("event") == "offload":
        yielded += e.get("for") == "running_call"
        start += e.get("for") != "running_call"
    elif e.get("event") == "ended_for_room":
        ended += 1
print(start, yielded, ended)')
  B1[$p]=$(recpid "$p-b2") B3[$p]=$(recpid "$p-b3") READ[$p]=$(store_bytes "$p-b3")
  say "$p: make_room offloads ${START[$p]}, ends ${ENDED[$p]}, in-call yields ${YIELD[$p]}; birefnet pid ${B1[$p]} -> ${B3[$p]}; its weights read again ${READ[$p]} B"
}
declare -A START YIELD ENDED B1 B3 READ

say "red: the pod's own TensorD $(cozy machine show "$NAME" --json | jq -r .status.version)"
phase red
step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
[ "$v" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not the session's"
say "green: the candidate TensorD $v"
phase green

[ "${START[red]}" -ge 1 ] || fail "inconclusive: red's make_room offloaded nothing for the reused SDXL"
[ "${START[green]}" = 0 ] || fail "green's make_room offloaded ${START[green]} executor(s) for a reused resident SDXL"
[ "${ENDED[green]}" = 0 ] || fail "green ended ${ENDED[green]} process(es) for a reuse"
{ [ -n "${B3[green]}" ] && [ "${B1[green]}" = "${B3[green]}" ]; } || fail "green's BiRefNet changed process (${B1[green]} -> ${B3[green]})"
say "PASS: red's make_room offloaded BiRefNet ${START[red]}x for the reused SDXL; green's none (in-call yields ${YIELD[green]}, BiRefNet kept its process ${B3[green]})"
