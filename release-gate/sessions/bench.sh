# bench: the Cozy-vs-ComfyUI benchmark (cozy-creator/benchmarks, checked out at $BENCH by pod-session at
# params.bench_ref) on one rental. The runner times both engines on the pod itself; this session only rents with SSH,
# copies the runner over (`cozy machine ssh`), starts it detached there, keeps the rental's idle clock reset while the
# runner's progress log grows, brings the results back and ends the rental. Nothing here measures. params:
#   bench_ref  the benchmarks commit or branch (master)
#   models     cells.toml models, in order (["sdxl", "anima", "moss", "qwen", "h3_turbo", "h3_full"])
#   tiers      scout (one warm-up, one timed request per engine) or full (cold, warm x3)
#   arms       ComfyUI arms, comma-separated ("matched")
#   free_mib   leave only this much VRAM free, the rest held by a separate process (0: the whole card)
#   ram_free_gib  leave only this much host RAM available (0: all of it)
#   traffic_gb GB the provider bills: the image, both engines' weights, the results (250)
#   need_s     seconds the guard must buy (10800); a hosted job ends at 6 h, so size a session below that
#   stall_s    no runner progress for this long ends the session (1800)
. "$(dirname "$0")/lib.sh"
MODELS=$(param models '["sdxl", "anima", "moss", "qwen", "h3_turbo", "h3_full"]') TIERS=$(param tiers scout) ARMS=$(param arms matched)
FREE=$(param free_mib 0) RAM=$(param ram_free_gib 0) STALL=$(param stall_s 1800)
[ -f "${BENCH:-}/bench.py" ] || fail "no benchmarks checkout at ${BENCH:-}"
mapfile -t models < <(jq -r '.[]' <<<"$MODELS")
[[ $TIERS == scout || $TIERS == full ]] && [[ $FREE =~ ^[0-9]+$ ]] && [[ $RAM =~ ^[0-9.]+$ ]] && [[ $ARMS =~ ^[a-z0-9,]+$ ]] || fail "tiers, arms, free_mib or ram_free_gib"
for m in "${models[@]}"; do grep -q "^\[models\.$m\]" "$BENCH/cells.toml" || fail "cells.toml has no model $m"; done
python3 "$BENCH/bench.py" check --describe > "$OUT/check.json" 2>&1 || fail "the runner's dry run: $(head -c 600 "$OUT/check.json")"
say "benchmarks $(git -C "$BENCH" rev-parse --short HEAD): models ${models[*]}; tiers $TIERS; arms $ARMS; free_mib $FREE; ram_free_gib $RAM"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param traffic_gb 250)" "$(param need_s 10800)"
rent

S=$(date -u +%Y%m%dT%H%MZ)-$SKU; [ "$FREE" = 0 ] || S=$S-$FREE; [ "$RAM" = 0 ] || S=$S-ram$RAM
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
tar -C "$BENCH" -cz --exclude .git --exclude results . | T=600 pod "mkdir -p /root/bench && tar -xz -C /root/bench" || fail "copy the runner"
T=900 pod "command -v ffprobe >/dev/null || { apt-get update -qq && apt-get install -y -qq ffmpeg; } >/dev/null 2>&1; ffprobe -version | head -1" \
  < /dev/null >> "$R" 2>&1 || fail "no ffmpeg on the pod"
# One detached plan on the pod: a dropped SSH connection or a lost runner leaves it running, and the rental ends itself
# 15 min after the plan stops making progress.
plan="cd /root/bench && { python3 bench.py setup --models $(IFS=,; echo "${models[*]}") --sku $SKU --session $S && {"
for m in "${models[@]}"; do
  plan="$plan python3 bench.py cell --model $m --tiers $TIERS --arms $ARMS --free-mib $FREE --ram-free-gib $RAM --sku $SKU --session $S;"
done
plan="$plan }; echo \$? > done; } > run.log 2>&1"
pod "nohup bash -c '$plan' > /dev/null 2>&1 &" < /dev/null || fail "start the runner"
say "runner started on $NAME, session $S"
pull() {  # the rows and logs so far; the outputs too with `all`
  mkdir -p "$OUT/results"
  T=900 pod "cd /root/bench && tar -cz run.log results $([ "${1:-}" = all ] || echo "--exclude=*.png --exclude=*.webp --exclude=*.mp4")" \
    < /dev/null 2>/dev/null | tar -xz -C "$OUT/results" 2>/dev/null
}
last=-1 still=0 kept=0
while :; do
  sleep 30
  size=$(pod "test -f /root/bench/done && echo done; cat /root/bench/results/$S/progress.log 2>/dev/null | wc -c" < /dev/null 2>/dev/null)
  grep -q done <<<"$size" && break
  size=$(tail -1 <<<"$size")
  if [[ $size =~ ^[0-9]+$ ]] && [ "$size" != "$last" ]; then
    still=0 last=$size
    [ $(( $(date +%s) - kept )) -lt 300 ] || { cozy machine keepalive "$NAME" > /dev/null 2>&1; kept=$(date +%s); pull; }
  else
    still=$((still + 30))
    [ "$still" -lt "$STALL" ] || { pull all; fail "the runner made no progress for ${STALL}s: $(tail -3 "$OUT/results/run.log" 2>/dev/null)"; }
  fi
done
pull all
say "runner exit $(pod 'cat /root/bench/done' < /dev/null 2>/dev/null); $(tail -2 "$OUT/results/run.log")"
cp -r "$OUT/results/results/." "$BENCH/results/" 2>/dev/null && (cd "$BENCH" && python3 chart.py results/$S.jsonl) >> "$R" 2>&1
cp "$BENCH/results/cells.csv" "$BENCH/results/profile.md" "$BENCH/chart.svg" "$OUT/" 2>/dev/null
[ -s "$OUT/results/results/$S.jsonl" ] || fail "no results"
say "PASS: $(wc -l < "$OUT/results/results/$S.jsonl") requests in results/$S.jsonl"
