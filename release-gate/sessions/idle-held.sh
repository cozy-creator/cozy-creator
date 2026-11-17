# idle-held (cl-1159): an idle executor offers every byte of GPU memory it holds. In tensord 44053333998, SDXL's
# executor sat idle after four LoRA stacks on Runtime 0.46 / TensorFS 0.25. It held 10.3 GiB of a 16 GB card and counted
# 3.9 GiB of that in use, so a new process asking 11 GiB was refused `device_out_of_memory`. 0.25.0 did the same; the
# RC stack (Runtime 0.47, TensorFS 0.26) keeps 260 MiB (44069488893). Change-gated: when varena or TensorFS changes.
# One 16 GB card, each arm on the same pod:
# - the machine is updated (its restart ends every executor) and its derived state cleared;
# - SDXL runs alone, then tensord-adapter-views' four LoRA stacks, each followed by `idle_s` idle and a ledger read;
# - every ledger row of a process running nothing (varena's pages) must count what it holds as idle, within 512 MiB;
# - a new process then holds three quarters of the card (suites/tensord/teardown.py).
# params:
#   arms     [{"name", "update": [`machine update` flags], "sdxl"}] in order. A flag's value is a version, or `cozy` for this
#            session's (`--tensord cozy`). `sdxl` is the target the arm runs: `sdxl/generate` (the default), or
#            `./<tree>/sdxl/generate` from a packages checkout (packages_refs <tree>=<ref>), for a Runtime no release runs on.
#   idle_s   seconds the machine stays idle before its ledgers are read (default 20)
#   need_s   seconds the guard must buy (default 2400)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")/.." && pwd)
ARMS=$(param arms '[]'); IDLE=$(param idle_s 20)
[ "$(jq 'length' <<< "$ARMS")" -ge 1 ] || fail "no arms"
for t in $(jq -r '.[] | .sdxl // empty' <<< "$ARMS"); do
  [[ $t != ./* ]] || [ -d "$PACKAGES/$(cut -d/ -f2 <<< "$t")" ] || fail "no packages checkout for $t"
done
[ -f ~/.ssh/id_ed25519.pub ] || { mkdir -p ~/.ssh && ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519; } || fail "no SSH key"
RENT_ARGS=--ssh
quote "$(param need_s 2400)"
rent

LEDGERS=$(base64 -w0 "$HERE/suites/tensord/ledgers.py")
# Each GPU's rows of live processes (ledgers.py: v6 and v7 pages, not the host's).
ledgers() { cozy machine ssh "$NAME" "echo $LEDGERS | base64 -d | python3 -" 2> /dev/null | tail -n 1; }
smi() { cozy machine ssh "$NAME" "nvidia-smi --query-gpu=memory.used,memory.total --format=csv,noheader,nounits" 2> /dev/null | head -n 1; }
status() { (cd "$RUN_PROJECT" && cozy run watch "$(runno "$1")" --once --json --full) > "$OUT/$1.json" 2>&1; jq -r '.status // "none"' "$OUT/$1.json"; }
ikea=ostris/ikea-lora-sdxl balloon=proomptengineer/balloon-lora-sdxl
sdxl() {
  local n=$1; shift
  if [[ $SDXL == ./* ]]; then (cd "$PACKAGES" && step "$n" run "$SDXL" "$@" --on "$NAME"); else step "$n" run "$SDXL" "$@" --on "$NAME"; fi
  [ "$(status "$n")" = completed ] || fail "$n: $(jq -r '.reason.message // empty' "$OUT/$n.json" | head -c 300)"
}

bad=()
for i in $(seq 0 $(( $(jq length <<< "$ARMS") - 1 ))); do
  a=$(jq -r ".[$i].name" <<< "$ARMS")
  mapfile -t flags < <(jq -r ".[$i].update[]" <<< "$ARMS" | sed "s#^cozy\$#$(command -v cozy)#")
  SDXL=$(jq -r ".[$i].sdxl // \"sdxl/generate\"" <<< "$ARMS")
  derived_state "$a" reset
  step "$a-update" machine update "$NAME" "${flags[@]}" --gpu-memory-limit off -y || fail "$a: the update"
  derived_state "$a" census
  say "$a: $(cozy machine show "$NAME" --json | jq -r '.status | "TensorD \(.version), Runtime \(.runtime), TensorFS \(.tensorfs)"')"
  sdxl "$a-plain" prompt="a copper kettle on a stove, morning light, photo" seed:=$((RANDOM + 1))
  sleep "$IDLE"; l=$(ledgers); echo "$l" > "$OUT/$a-ledger-plain.json"
  say "$a: after SDXL alone, idle $IDLE s: $(jq -r '[.[][] | select(.running == 0) | "pid \(.pid) held \(.held / 1048576 | floor) idle \(.idle / 1048576 | floor)"] | join("; ")' <<< "$l")"
  for l in $ikea $balloon; do step "$a-lora-$(basename "$l")" model install "$l" --on "$NAME" -y || fail "$a: $l"; done
  sdxl "$a-v0" prompt="a brass compass as flat-pack furniture, catalog photo" seed:=$((RANDOM + 2)) --lora "$ikea"
  sdxl "$a-v1" prompt="PEBalloonStyle, a fox made of balloons" seed:=$((RANDOM + 3)) --lora "$balloon"
  sdxl "$a-v2" prompt="PEBalloonStyle, a harbor as a flat-pack kit" seed:=$((RANDOM + 4)) --lora "$ikea,0.6" --lora "$balloon,0.8"
  sdxl "$a-v3" prompt="a brass compass, flat-pack, half strength" seed:=$((RANDOM + 5)) --lora "$ikea,0.5"
  sleep "$IDLE"
  l=$(ledgers); echo "$l" > "$OUT/$a-ledger.json"; used=$(smi)
  rows=$(jq -r '[.[][] | select(.running == 0) | "pid \(.pid) held \(.held / 1048576 | floor) idle \(.idle / 1048576 | floor)"] | join("; ")' <<< "$l")
  kept=$(jq '[.[][] | select(.running == 0) | .held - .idle] | max // 0 | . / 1048576 | floor' <<< "$l")
  total=${used#*, }; gib=$(( total * 3 / 4 / 1024 ))
  say "$a: idle $IDLE s: ${rows:-no ledger rows}; the card: ${used%%,*} of $total MiB used"
  step "$a-hold" run "$HERE/suites/tensord/teardown.py" gib:="$gib" host_gib:=0 seconds:=3 --gpus=1 --on "$NAME"
  held=$(status "$a-hold"); why=$(jq -r '.reason.message // empty' "$OUT/$a-hold.json" | head -c 300)
  echo "$(ledgers)" > "$OUT/$a-ledger-after.json"
  say "$a: an idle process kept at most $kept MiB in use; a new process asking $gib GiB: $held${why:+ ($why)}"
  { [ "$kept" -le 512 ] && [ "$held" = completed ]; } || bad+=("$a: kept $kept MiB in use while idle, the $gib GiB hold $held")
done
[ ${#bad[@]} = 0 ] || fail "$(printf '%s; ' "${bad[@]}")"
say "PASS: in every arm the idle executors offered what they held and a new process held three quarters of the card"
