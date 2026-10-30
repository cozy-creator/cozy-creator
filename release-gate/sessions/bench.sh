# bench: the Cozy-vs-ComfyUI benchmark (cozy-creator/benchmarks, checked out at $BENCH by pod-session at
# params.bench_ref) on one rental. The runner times both engines on the pod itself; this session only rents with SSH,
# copies the runner over (`cozy machine ssh`), starts it detached there, keeps the rental's idle clock reset while the
# runner's progress log grows, brings the results back and ends the rental. Nothing here measures. params:
#   bench_ref  the benchmarks commit or branch (master)
#   models     cells.toml models, in order (["sdxl", "anima", "moss", "qwen", "h3_turbo", "h3_full"])
#   tiers      scout (one warm-up, one timed request per engine) or full (cold, warm x3)
#   arms       ComfyUI arms, comma-separated ("matched")
#   free_mib   leave only this much VRAM free, the rest held by a separate process (0: the whole card); an object
#              {"<model>": MiB} sets it per model (a model it omits gets the whole card)
#   ram_free_gib  leave only this much host RAM available (0: all of it)
#   traffic_gb GB the provider bills: the image, both engines' weights, the results (250)
#   need_s     seconds the guard must buy (10800); a hosted job ends at 6 h, so size a session below that
#   stall_s    no runner progress for this long ends the session (1800)
#   update     `cozy machine update` flags run on the pod before the runner: a candidate's Cozy side (["--runtime",
#              "url:https://…#sha256=<hex>", "--tensorfs", "artifact:<cozy-creator run id>:<artifact>"], or a
#              dev-build cozy as "--tensord", "artifact:<run id>:cozy-<sha>"); each url:
#              or artifact: value is fetched here before renting and sent from this runner ([]: the pod as rented)
#   candidates {"<package>": "<packages_refs name>"}: each package runs from that packages checkout (local/<package>),
#              not its Hub release: a package fix's proof ({}: the Hub's)
. "$(dirname "$0")/lib.sh"
MODELS=$(param models '["sdxl", "anima", "moss", "qwen", "h3_turbo", "h3_full"]') TIERS=$(param tiers scout) ARMS=$(param arms matched)
FREE=$(param free_mib 0) RAM=$(param ram_free_gib 0) STALL=$(param stall_s 1800)
[ -f "${BENCH:-}/bench.py" ] || fail "no benchmarks checkout at ${BENCH:-}"
mapfile -t models < <(jq -r '.[]' <<<"$MODELS")
free_of() { jq -r --arg m "$1" 'if type == "object" then (.[$m] // 0) else . end' <<<"$FREE"; }
for m in "${models[@]}"; do [[ $(free_of "$m") =~ ^[0-9]+$ ]] || fail "free_mib for $m"; done
[[ $TIERS == scout || $TIERS == full ]] && [[ $RAM =~ ^[0-9.]+$ ]] && [[ $ARMS =~ ^[a-z0-9,]+$ ]] || fail "tiers, arms or ram_free_gib"
# The candidate's wheels, fetched before renting, so a missing one costs nothing.
mapfile -t update < <(param update '[]' | jq -r '.[]')
for i in "${!update[@]}"; do
  v=${update[$i]}; [[ $v == url:* || $v == artifact:* ]] || continue
  d=$OUT/update/$i; mkdir -p "$d"
  if [[ $v == artifact:* ]]; then
    IFS=: read -r _ rid name <<<"$v"
    gh run download "$rid" -R cozy-creator/cozy-creator -n "$name" -D "$d" >/dev/null 2>&1 || fail "no artifact $v"
  else
    u=${v#url:} want=${v##*#sha256=}; u=${u%%#*}; f=${u##*/}; f=${f//%2B/+}  # a local version's +
    curl -fsSL --retry 5 -o "$d/$f" "$u" || fail "cannot fetch $u"
    [ "$(sha256sum "$d/$f" | cut -d' ' -f1)" = "$want" ] || fail "$u is not sha256 $want"
  fi
  # A Linux x86_64 wheel, else a dev-build cozy binary (`--tensord`), made executable.
  w=$(ls "$d"/*.whl 2>/dev/null | grep -m1 'manylinux.*x86_64') || w=$(find "$d" -type f -name cozy-linux-x86_64 | head -1)
  [ -n "$w" ] || fail "$v has no Linux x86_64 wheel or cozy binary"
  chmod +x "$w"; update[$i]=$w
done
for m in "${models[@]}"; do grep -q "^\[models\.$m\]" "$BENCH/cells.toml" || fail "cells.toml has no model $m"; done
# Each candidate package: its checkout here, copied to /root/candidates/<package> on the pod.
mapfile -t candidates < <(param candidates '{}' | jq -r 'to_entries[] | "\(.key)=\(.value)"')
here=() there=()
for c in "${candidates[@]}"; do
  pkg=${c%%=*} tree=${c#*=}; d=$PACKAGES/$tree/$pkg
  [[ $pkg =~ ^[a-z0-9.-]+$ && $tree =~ ^[A-Za-z0-9_-]+$ && -f $d/pyproject.toml ]] || fail "candidate $c: no packages_refs checkout $tree/$pkg"
  here+=(--candidate "$pkg=$d") there+=(--candidate "$pkg=/root/candidates/$pkg")
  say "candidate $pkg: packages $(git -C "$d" rev-parse --short HEAD) ($tree)"
done
python3 "$BENCH/bench.py" check --describe "${here[@]}" > "$OUT/check.json" 2>&1 || fail "the runner's dry run: $(head -c 600 "$OUT/check.json")"
say "benchmarks $(git -C "$BENCH" rev-parse --short HEAD): models ${models[*]}; tiers $TIERS; arms $ARMS; free_mib $(jq -c . <<<"$FREE"); ram_free_gib $RAM; update ${update[*]##*/}"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param traffic_gb 250)" "$(param need_s 10800)"
rent

if [ "${#update[@]}" -gt 0 ]; then
  step update machine update "$NAME" "${update[@]}" -y || fail "the candidate's update"
  cozy machine show "$NAME" > "$OUT/show-update.txt" 2>&1; grep -E '^(version|runtime|tensorfs) ' "$OUT/show-update.txt" | tee -a "$R"
fi
S=$(date -u +%Y%m%dT%H%MZ)-$SKU; [[ $FREE == 0 || $FREE == {* ]] || S=$S-$FREE; [ "$RAM" = 0 ] || S=$S-ram$RAM
[ "${#update[@]}" = 0 ] && [ "${#candidates[@]}" = 0 ] || S=$S-candidate
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
tar -C "$BENCH" -cz --exclude .git --exclude results . | T=600 pod "mkdir -p /root/bench && tar -xz -C /root/bench" || fail "copy the runner"
for c in "${candidates[@]}"; do
  pkg=${c%%=*}
  tar -C "$PACKAGES/${c#*=}/$pkg" -cz --exclude .venv . | T=600 pod "mkdir -p /root/candidates/$pkg && tar -xz -C /root/candidates/$pkg" ||
    fail "copy the candidate $pkg"
done
T=900 pod "command -v ffprobe >/dev/null || { apt-get update -qq && apt-get install -y -qq ffmpeg; } >/dev/null 2>&1; ffprobe -version | head -1" \
  < /dev/null >> "$R" 2>&1 || fail "no ffmpeg on the pod"
# ComfyUI, its environment and its weight files come from this runner (the runner's `ship`), so the pod's setup
# downloads only Cozy's models: five 3060 hosts in a row could not reach PyPI, pytorch.org or Hugging Face. The
# environment (about 10 GB) is built here; the weights stream through. A keepalive holds the rental meanwhile.
( while sleep 240; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done ) & KA=$!
if ! python3 "$BENCH/bench.py" ship --models "$(IFS=,; echo "${models[*]}")" --arms "$ARMS" --pod "timeout 3600 cozy machine ssh $NAME" \
    < /dev/null > "$OUT/ship.log" 2>&1; then
  kill "$KA"; fail "ship: $(tail -3 "$OUT/ship.log")"
fi
kill "$KA"; say "$(tail -1 "$OUT/ship.log")"
# One detached plan on the pod: a dropped SSH connection or a lost runner leaves it running, and the rental ends itself
# 15 min after the plan stops making progress.
plan="cd /root/bench && { python3 bench.py setup --models $(IFS=,; echo "${models[*]}") --arms $ARMS --sku $SKU --session $S && {"
for m in "${models[@]}"; do
  plan="$plan python3 bench.py cell --model $m --tiers $TIERS --arms $ARMS --free-mib $(free_of "$m") --ram-free-gib $RAM --sku $SKU --session $S ${there[*]};"
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
