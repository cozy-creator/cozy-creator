# disk-valentino (cl-774): rental valentino (11-05, a 100 GB H200) ran out of disk with one H3 + Qwen-Image project and
# its H3 renders failed DiskFullError mid-denoise. One pod with the same disk, its TensorD the session's cozy:
#   1. Qwen-Image 2.1, SDXL and Qwen-Image 2.1 turbo render (int8); their executors stay idle and warm, holding them open.
#   2. H3 int8 renders: it fits only once a model it doesn't read goes, so its room check ends idle executors and evicts
#      before any of H3 is fetched, and its warnings name what went.
# A step whose request the package refuses (its interface) fails the session before any model is judged.
#   3. While H3 renders, Qwen turbo is asked again: H3 is never touched (it completes, nothing of it evicted), and turbo
#      either fits beside it or is refused machine_disk_full with the sizes.
#   4. H3 bf16 (130 GiB, more than the disk) is refused machine_disk_full while it prepares, with nothing downloaded.
#   5. Every byte of the store is a model `model ls` names.
# Each package is the release its interface was read at (`cozy run <fn>@<release> --describe`, 11-05).
# params:
#   need_s      seconds the guard must buy (default 2700)
. "$(dirname "$0")/lib.sh"
# A step's run: its follower's first line is `run <n> · <target>`, and only a completed run's ends with a bare `run <n>`.
rn() { awk '$1 == "run" && $2 ~ /^[0-9]+$/ {print $2; exit}' "$OUT/$1.txt" 2>/dev/null; }
status() { cozy run show "$(rn "$1")" --once --json --full > "$OUT/$1.json" 2>&1; jq -r '.status // empty' "$OUT/$1.json"; }
reason() { jq -r '[.reason.code // "", .reason.message // ""] | join(": ")' "$OUT/$1.json" | cut -c1-500; }
# What a run warned of, its calls' too (a call's lines are on its root's log): any string of its record, or what its
# follower printed.
warned() { { jq -r '.. | strings' "$OUT/$1.json" 2>/dev/null; cat "$OUT/$1.txt" 2>/dev/null; } | grep -E 'the disk was short|evicted' | sort -u; }
disk() { cozy machine show "$NAME" --json | jq -r '.status | "\(.disk_free_bytes // 0) \(.disk_models_bytes // 0) \(.models_bytes // 0) \(.disk_total_bytes // 0)"'; }
gib() { awk -v b="$1" 'BEGIN { printf "%.1f GiB", b / 1073741824 }'; }
quote "$(param need_s 2700)"
rent

step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
[ "$v" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not the session's"
read -r free stored listed total < <(disk); say "pod TensorD $v; disk $(gib "$total"), $(gib "$free") free"

# 1. Both Qwen models and SDXL, the turbo one last so its executor is the newest idle one. A 5 s H3 with no reference
# reads 48 GiB (no transformer_ref), so it must make room on a 70 GB disk (`-f disk=70`) and fits beside them on 100 GB.
step qwen run qwen-image-2/generate@0.3.0 --on "$NAME" --lane int8 prompt="a red lighthouse on a cliff at dawn, photo" seed:=$RANDOM || fail "qwen"
[ "$(status qwen)" = completed ] || fail "qwen $(status qwen): $(reason qwen)"
step sdxl run sdxl/generate@0.3.0 --on "$NAME" prompt="a copper kettle on a stove, morning light, photo" seed:=$RANDOM || fail "sdxl"
[ "$(status sdxl)" = completed ] || fail "sdxl $(status sdxl): $(reason sdxl)"
step turbo run qwen-image-2/generate_turbo@0.3.0 --on "$NAME" --lane int8 prompt="a green tram in the rain at night, photo" seed:=$RANDOM || fail "turbo"
[ "$(status turbo)" = completed ] || fail "turbo $(status turbo): $(reason turbo)"
read -r free stored listed total < <(disk); say "after Qwen: $(gib "$free") free, the store $(gib "$stored")"

# 2 and 3. H3 in the background; turbo asked again once H3 is rendering. What fails is kept (`bad`) so every case is read.
bad=()
SHOT="A woman in a yellow raincoat walks along a wet pier at dusk, gulls overhead, the camera tracking beside her."
before=$(cozy machine logs "$NAME" tensord -n 0 2>/dev/null | wc -l)
mem_before=$(cozy machine logs "$NAME" memory -n 0 2>/dev/null | wc -l)
step h3 run minimax-h3/generate@0.4.0 --on "$NAME" --lane int8 prompt="$SHOT" duration_s:=5.0 seed:=$RANDOM & H3P=$!
# Once H3 has made its room (its warning says what went), so turbo's ask can't race H3's room check.
until { [ -n "$(rn h3)" ] && status h3 > /dev/null && warned h3 | grep -q evicted; } || ! kill -0 "$H3P" 2>/dev/null; do sleep 5; done
t0=$(date +%s)
step turbo2 run qwen-image-2/generate_turbo@0.3.0 --on "$NAME" --lane int8 prompt="a blue kite over a beach, photo" seed:=$RANDOM
t2=$(status turbo2) t2_s=$(( $(date +%s) - t0 ))
wait "$H3P"; hs=$(status h3)
[ -n "$(rn h3)" ] || fail "H3 made no run: $(tail -4 "$OUT/h3.txt" | tr '\n' ' ')"
cozy machine logs "$NAME" tensord -n 0 > "$OUT/tensord.log" 2>&1
tail -n +"$((before + 1))" "$OUT/tensord.log" > "$OUT/tensord-h3.log"
cozy machine logs "$NAME" memory -n 0 > "$OUT/memory.log" 2>&1
tail -n +"$((mem_before + 1))" "$OUT/memory.log" > "$OUT/memory-h3.log"
h3_evicted=$(grep -c 'tensorfs: evicted minimaxai/minimax-h3' "$OUT/tensord-h3.log")
said=$(warned h3 | grep -m1 'evicted' || true)
ended=$(grep -c '"event":"end_idle"' "$OUT/memory-h3.log") for_vram=$(grep -c '"event":"ended_for_room"' "$OUT/memory-h3.log")
grep -E 'tensorfs: evicted|ended the idle executors|the disk was short|machine_disk_full' "$OUT/tensord-h3.log" | cut -c1-300 >> "$R"
grep -E '"event":"(end_idle|ended_for_room)"' "$OUT/memory-h3.log" | cut -c1-300 >> "$R"
say "H3 $hs, its warning: ${said:-none}"
say "turbo asked while H3 rendered: $t2 in ${t2_s}s $(reason turbo2); H3 evicted $h3_evicted times; executors ended since H3 began: $ended ($for_vram of them for GPU memory)"
[ "$hs" = completed ] || bad+=("H3 $hs: $(reason h3)")
[ -n "$said" ] || bad+=("H3 says nothing evicted: the disk held it without making room")
[ "$h3_evicted" = 0 ] || bad+=("H3 was evicted while it rendered")
case "$t2:$(jq -r '.reason.code // empty' "$OUT/turbo2.json")" in
  completed:* | failed:machine_disk_full) ;;
  *) bad+=("turbo beside H3: $t2 $(reason turbo2)") ;;
esac

# 4. More than the disk.
read -r free stored listed total < <(disk)
t0=$(date +%s)
step bf16 run minimax-h3/generate@0.4.0 --on "$NAME" --lane bf16 prompt="$SHOT" duration_s:=5.0 seed:=$RANDOM
bs=$(status bf16) bs_s=$(( $(date +%s) - t0 )); read -r free2 stored2 _ _ < <(disk)
grew=$(( stored2 > stored ? stored2 - stored : 0 ))
say "H3 bf16 $bs in ${bs_s}s: $(reason bf16); the store grew $(gib "$grew")"
[ "$bs" = failed ] && [ "$(jq -r .reason.code "$OUT/bf16.json")" = machine_disk_full ] || bad+=("H3 bf16 $bs, not refused machine_disk_full: $(reason bf16)")
grep -q -- '--disk-gb' <<<"$(reason bf16)" || bad+=("the refusal names no --disk-gb")
[ "$grew" -lt $((1 << 30)) ] || bad+=("H3 bf16 was refused after $(gib "$grew") landed")

# 5. Every byte a listed model's.
cozy model ls --on "$NAME" > "$OUT/model-ls.txt" 2>&1
read -r free stored listed total < <(disk); unlisted=$(( stored > listed ? stored - listed : 0 ))
say "the store $(gib "$stored"), listed models $(gib "$listed"): $(tail -1 "$OUT/model-ls.txt")"
[ $((unlisted * 100)) -le "$stored" ] || bad+=("$(gib "$unlisted") of the store no model names")
[ ${#bad[@]} = 0 ] || fail "$(printf '%s; ' "${bad[@]}")"
say "PASS: H3 rendered after evicting ($said); turbo beside it $t2; H3 bf16 refused while it prepared; the store $(gib "$stored"), $(gib "$unlisted") unlisted"
