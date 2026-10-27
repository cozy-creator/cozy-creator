# h3-turbo-diag: why GPU 1's process aborts in a 2-GPU H3 Turbo call (release-gate 40459633948 run 1, Runtime 0.34.2,
# minimax-h3 0.1.2): the production image as it ships, the same 15 s request shape, one arm after another on one rental,
# and after each arm every log the machine keeps, read before the pod ends. Proof-only. params:
#   gpus     GPUs in the rental and per call (2)
#   arms     [{"label", "target" (a published function, e.g. minimax-h3/ref2va_turbo@0.1.2), "attention" ("" or an
#            --attention-kernel pin), "seed", "when" ("always" | "all_failed": only if every earlier arm failed)}]
#   traffic_gb GB the provider bills (120); need_s seconds the cap must buy (1500)
. "$(dirname "$0")/lib.sh"
ARMS=$(param arms '[]')
mapfile -t labels < <(jq -r '.[].label' <<<"$ARMS")
[ "${#labels[@]}" -gt 0 ] || fail "no arms"
for i in "${!labels[@]}"; do
  k=${labels[i]}
  [[ $k =~ ^[A-Za-z0-9_-]+$ ]] || fail "label $k"
  jq -e ".[$i] | (.target | test(\"^minimax-h3/[a-z0-9_]+@[0-9.]+$\")) and (.seed | type == \"number\")" <<<"$ARMS" >/dev/null || fail "arm $k: target/seed"
  say "arm $k: $(jq -c ".[$i]" <<<"$ARMS")"
done
[ -f "$REF/seed.jpg" ] || fail "no reference image"
H3REF="[{\"name\": \"Lead\", \"kind\": \"character\", \"image\": \"$REF/seed.jpg\"}]"
# number <file>: the run a --json follower's events name first.
number() { grep '^{' "$1" 2>/dev/null | jq -r 'select(.run.number != null) | .run.number' 2>/dev/null | head -1; }
printf '{"type":"x"}\n{"run":{"number":7,"status":"queued"}}\n' > "$OUT/probe.txt"
[ "$(number "$OUT/probe.txt")" = 7 ] || fail "parser"
quote "$(param traffic_gb 120)" "$(param need_s 1500)"
rent
failed=0 ran=0
for i in "${!labels[@]}"; do
  k=${labels[i]} t=$(jq -r ".[$i].target" <<<"$ARMS") pin=$(jq -r ".[$i].attention // \"\"" <<<"$ARMS")
  seed=$(jq -r ".[$i].seed" <<<"$ARMS") when=$(jq -r ".[$i].when // \"always\"" <<<"$ARMS")
  if [ "$when" = all_failed ] && [ "$failed" != "$ran" ]; then say "arm $k skipped: an earlier arm passed"; continue; fi
  args=(--json run "$t" --on "$NAME" --gpus="$GPUS" references:="$H3REF" seed:="$seed" duration_s:=15 resolution=768p
    prompt="<Lead> walks along the snowy cliff path to the lighthouse door, stops, and looks up at the beam. Wind, crunching snow, a distant foghorn. Take $k."
    --out "$OUT/out/$k")
  [ -z "$pin" ] || args+=(--attention-kernel "$pin")
  step "$k" "${args[@]}"
  n=$(number "$OUT/$k.txt") ran=$((ran + 1))
  [ -n "$n" ] && cozy --json run show "$n" --once > "$OUT/$k.json" 2>&1
  for log in tensord memory tensorfs; do cozy machine logs "$RID" "$log" -n 0 > "$OUT/$k-machine-$log.log" 2>&1; done
  cozy --json machine show "$RID" > "$OUT/$k-machine-show.json" 2>&1
  st=$(jq -r '.status // empty' "$OUT/$k.json" 2>/dev/null)
  [ "$st" = completed ] || failed=$((failed + 1))
  say "arm $k: run ${n:-none} ${st:-unknown} $(jq -r '.reason.message // empty' "$OUT/$k.json" 2>/dev/null | head -c 300)"
  # The abort's own words, wherever the machine kept them.
  grep -hiE 'terminate called|what\(\)|abort|SIGABRT|signal 6|CUDA error|illegal|assert|NCCL (WARN|error)|core dumped|Fatal Python|exit status -6' \
    "$OUT/$k.txt" "$OUT/$k-machine-"*.log 2>/dev/null | grep -v '"progress"' | cut -c1-400 | sort -u | head -40 > "$OUT/$k-abort.txt"
  say "arm $k: $(wc -l < "$OUT/$k-abort.txt") abort-like lines kept"
  grep '^{' "$OUT/$k.txt" | jq -r 'select(.text and (.level == "warning" or .level == "error")) | "\(.at_ms) \(.level) \(.text)"' 2>/dev/null |
    grep -vE '^\S+ error +(File|\^|return|raise)' | cut -c1-400 | tail -30 > "$OUT/$k-warnings.txt"
done
say "arms: $ran ran, $failed failed"
