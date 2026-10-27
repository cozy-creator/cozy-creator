# reliability: idle-executor context reclaim on a real small card (PR #2174, fix 2's `ended_for_room`).
# One rental, driven from a runner. The session's cozy carries the candidate TensorD onto the pod. On an 8 GB card
# SDXL and small siblings (BiRefNet, Anima) do not co-fit, so stacking them forces make_room to end an idle process
# and reclaim its CUDA context, not merely offload its weights; a later sibling run must still complete in a fresh
# executor. All targets are released first-party packages; the Hub picks each model's lane for the card. params:
#   traffic_gb  GB the provider bills: the image and the models (default 14)
. "$(dirname "$0")/lib.sh"
recpid() { cozy run show "$1" --json 2>/dev/null | jq -r 'first(.measurements.execution.gpus[]?.pid) // empty'; }
recstatus() { cozy run show "$1" --json 2>/dev/null | jq -r '.status // empty'; }
quote "$(param traffic_gb 14)" 2400
rent

# The pod runs the candidate TensorD (the runner's cozy), as the capability session does.
step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
[ "$v" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not the session's"
say "pod TensorD $v"

# Leave BiRefNet's executor idle-resident, then run SDXL: on 8 GB the two do not co-fit, so make_room
# offloads BiRefNet's weights and, still short, ends its process to reclaim the context. Then a successor.
step bg1 run birefnet/remove_background --on "$NAME" --asset image="$REF/seed.jpg" || fail "birefnet 1"
B1=$(runno bg1)
step big run sdxl/generate --on "$NAME" prompt="a quiet harbor at dawn, photo" seed:=$RANDOM || fail "sdxl"
BIG_N=$(runno big)
step bg2 run birefnet/remove_background --on "$NAME" --asset image="$REF/seed.jpg" || fail "birefnet successor"
B2=$(runno bg2)

cozy machine logs "$NAME" memory -n 100000 > "$OUT/memory.log" 2>&1
say "memory events: $(grep -oE '"event":"[a-z_]+"' "$OUT/memory.log" | sort | uniq -c | tr -s ' \n' ' ')"
ended=$(grep -c '"event":"ended_for_room"' "$OUT/memory.log")
offloads=$(grep -c '"event":"offload"' "$OUT/memory.log")
running_call=$(grep -c '"for":"running_call"' "$OUT/memory.log")
for n in "$B1" "$BIG_N" "$B2"; do [ "$(recstatus "$n")" = completed ] || fail "run $n is $(recstatus "$n"), not completed"; done
p1=$(recpid "$B1") p2=$(recpid "$B2")
say "birefnet pid $p1 -> successor pid $p2; ended_for_room=$ended offload=$offloads running_call=$running_call"
# The card is small, so stacking must have reclaimed at least one context; the successor completed in a fresh process.
[ "$ended" -ge 1 ] || fail "no ended_for_room on an 8 GB card (sizes may co-fit): $(grep -E 'ledger|manager' "$OUT/memory.log" | head -1)"
{ [ -n "$p2" ] && [ "$p1" != "$p2" ]; } || fail "the successor did not run in a fresh executor ($p1 -> $p2)"
say "PASS: $ended context(s) reclaimed for room; successor $B2 completed in a fresh executor (pid $p2)"
