# ltx-comfy (se-172): LTX-2.5 against ComfyUI on one rental, one engine at a time, each request its own prompt and seed,
# the same for both. ComfyUI goes first, on the fresh pod (comfy-levels' start: no restart before it, xs-224): suites/bench/
# comfy.py `run` with the benchmarks runner's ltx25* cells, matched to a Cozy record's kernels. Then Cozy: `ab` on this
# pod (lib.sh on_this), its first cell updating the pod to the candidate and installing the model, every cell's run
# record kept. ComfyUI's numbers stay off this public run (Paul 11-10): its files are kept outside $OUT and published only as
# comfyui.sealed (seal.sh's scheme, to comfyui-seal.pub). A pod whose GPUs are power-capped by their host (a limit under
# its default) ends before anything downloads: its times would be void. params:
#   bench_ref  the benchmarks commit (pod-session.yaml checks it out at $BENCH)
#   tree       the packages_refs checkout whose ltx-2.5 runs (pk)
#   gpus       the pod's GPU count (1)
#   record     a Cozy run record ComfyUI is matched to, in this checkout (fixtures/ltx-comfy/rc-ltx-4090.json)
#   update     the candidate: ab's `update` ({"runtime", "tensorfs", "tensord", "gpu_memory_limit"})
#   cells      [{"name", "key" (ltx25, ltx25_i2v, ltx25_ms), "duration_s", "gpus", "picture" (true: first frame
#              release-gate/ref/seed.jpg), "prompt", "seed"}]: each a cold request and a warm one (its prompt with
#              ", at dawn" and seed + 100000), ComfyUI's and Cozy's the same; "comfy": false runs Cozy's alone (ComfyUI's
#              4-GPU run would repeat its 2-GPU one: it splits nothing more); "lane" Cozy's lane (int8; bf16 beside ltx25_bf16)
#   stall_s    no ComfyUI progress for this long ends the session (1800)
#   need_s     seconds the cap must buy (7200)
. "$(dirname "$0")/lib.sh"
TREE=$(param tree pk) GPUS=$(param gpus 1) CELLS=$(param cells '[]') UPDATE=$(param update '{}')
REC=$(param record release-gate/sessions/fixtures/ltx-comfy/rc-ltx-4090.json) STALL=$(param stall_s 1800)
RG=$(cd "$(dirname "$0")/.." && pwd)  # release-gate
PUB=$RG/suites/bench/comfyui-seal.pub
[ -f "${BENCH:-}/bench.py" ] || fail "no benchmarks checkout at ${BENCH:-}"
for f in suites/bench/comfy.py suites/bench/pod.sh suites/bench/vram.py suites/bench/comfyui-seal.pub; do
  [ -f "$RG/$f" ] || fail "no $f (pod-session.yaml's sparse checkout)"
done
[[ $GPUS =~ ^[1-8]$ && $TREE =~ ^[A-Za-z0-9_-]+$ ]] || fail "gpus or tree"
[ -f "$PACKAGES/$TREE/ltx-2.5/pyproject.toml" ] || fail "no ltx-2.5 in packages_refs $TREE"
REC=$RG/../$REC
jq -e '.measurements and .request.input.prompt' "$REC" > /dev/null 2>&1 || fail "record $REC: no measurements or prompt"
jq -e 'type == "array" and length > 0 and all(.[]; (.name | test("^[a-z0-9-]+$")) and (.prompt | type == "string")
  and (.seed | type == "number") and (.duration_s | type == "number") and (.gpus | type == "number"))' <<<"$CELLS" > /dev/null || fail "cells"
while read -r k; do grep -q "^\[models\.$k\]" "$BENCH/cells.toml" || fail "cells.toml has no model $k"; done < <(jq -r '.[].key' <<<"$CELLS")
# The requests: cold, then warm, each cell's own.
REQS=$(jq -c 'map({name, key, duration_s, gpus, picture: (.picture // false), comfy: (.comfy // true), lane: (.lane // "int8"),
  requests: [{prompt, seed}, {prompt: (.prompt + ", at dawn"), seed: (.seed + 100000)}]})' <<<"$CELLS")
PLAIN=${RUNNER_TEMP:-$OUT/..}/ltx-comfy-plain; rm -rf "$PLAIN"; mkdir -p "$PLAIN"
jq -n --argjson cells "$REQS" --slurpfile rec "$REC" '[$cells[] | select(.comfy) | {cell: .name, key, tiers: ["cold", "w1"], requests,
  length: .duration_s, cap_gib: null, scout_s: null, gpus, exchange: "host", record: $rec[0]}]' > "$PLAIN/jobs.json" || fail "jobs"
# Cozy's cells for ab: the same requests; the first updates the pod and installs the model, so no run fetches weights.
AB=$(jq -c --arg tree "$TREE" --argjson update "$UPDATE" --argjson gpus "$GPUS" '{gpus: $gpus, cells: ([.[] | . as $c | range(0; 2) as $i
  | {name: "\($c.name)-\($i)", tree: $tree, target: "ltx-2.5/generate",
     args: (["prompt=\($c.requests[$i].prompt)", "seed:=\($c.requests[$i].seed)", "duration_s:=\($c.duration_s)", "resolution=1080p",
             "aspect_ratio=16:9", "audio:=true", "--lane", $c.lane, "--gpus=\($c.gpus)"]
            + (if $c.picture then ["--file", "first_frame={ref}/seed.jpg"] else [] end))}]
  | .[0] += {update: $update, download: ([$cells[].lane] | unique | map(["lightricks/ltx-2.5@0.2.0", "--lane", .]))})}' --argjson cells "$REQS" <<<"$REQS") || fail "ab cells"
echo "$AB" > "$OUT/ab-params.json"
# ab fetches its update's wheels itself, after ComfyUI's pass on a billing pod: each is fetched here first, at $0.
for kind in runtime tensorfs; do
  src=$(jq -r --arg k "$kind" '.[$k] // empty' <<<"$UPDATE"); [ -n "$src" ] || continue
  f=$(wheel_from "$OUT/wheels-check/$kind" "$src") && [ -n "$f" ] || fail "update $kind: no wheel from $src"
  say "update $kind: ${f##*/}"
done
say "benchmarks $(git -C "$BENCH" rev-parse --short HEAD), packages $TREE $(git -C "$PACKAGES/$TREE" rev-parse --short HEAD): $(jq -r 'map(.name) | join(", ")' <<<"$CELLS")"
seal() {
  local key; key=$(openssl rand -hex 32)
  tar -C "$PLAIN" -czf - . | openssl enc -aes-256-cbc -pbkdf2 -pass "pass:$key" -out "$OUT/comfyui.sealed.new" &&
    printf %s "$key" | openssl pkeyutl -encrypt -pubin -inkey "$PUB" -pkeyopt rsa_padding_mode:oaep -out "$OUT/comfyui.sealed.key.new" &&
    mv "$OUT/comfyui.sealed.new" "$OUT/comfyui.sealed" && mv "$OUT/comfyui.sealed.key.new" "$OUT/comfyui.sealed.key"
}
seal || fail "seal"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh; [ "$GPUS" = 1 ] || RENT_ARGS="--ssh --gpus $GPUS"
quote "$(param need_s 7200)"
rent

pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
T=60 pod "nvidia-smi --query-gpu=index,name,power.limit,power.default_limit,clocks.max.sm,temperature.gpu --format=csv,noheader,nounits" \
  < /dev/null > "$OUT/gpus.csv" 2>&1 || fail "nvidia-smi on $NAME"
say "GPUs: $(paste -sd';' "$OUT/gpus.csv")"
awk -F', *' 'NF >= 4 && $3 + 0 < 0.97 * $4 { bad = bad " GPU " $1 " " $3 " of " $4 " W" } END { if (bad) { print bad; exit 1 } }' "$OUT/gpus.csv" > "$OUT/capped.txt" ||
  fail "power-capped by its host:$(cat "$OUT/capped.txt"): its times would be void"
T=900 pod "command -v ffprobe >/dev/null || { apt-get update -qq && apt-get install -y -qq ffmpeg; } >/dev/null 2>&1; ffprobe -version | head -1;
  python3 -c 'import sys; assert sys.version_info >= (3, 11), sys.version; print(\"python\", sys.version.split()[0])'; git --version" \
  < /dev/null >> "$R" 2>&1 || fail "the pod lacks ffprobe, python3 >= 3.11 or git"
tar -C "$BENCH" -cz --exclude .git --exclude results . | T=600 pod "rm -rf /var/tmp/bm && mkdir -p /var/tmp/bm && tar -xz -C /var/tmp/bm" ||
  fail "copy the runner"
{ T=60 pod "cat > /var/tmp/bm/bench_comfy.py" < "$RG/suites/bench/comfy.py" && T=60 pod "cat > /var/tmp/bench-pod.sh" < "$RG/suites/bench/pod.sh" &&
  T=60 pod "cat > /var/tmp/bench-vram.py" < "$RG/suites/bench/vram.py" && T=60 pod "cat > /var/tmp/bm/jobs.json" < "$PLAIN/jobs.json"; } ||
  fail "copy comfy.py, pod.sh, vram.py and the jobs"
# Every GPU each second (nvidia-smi: clocks, power and slowdown reasons, for the heat check) and host memory.
T=60 pod "cat > /var/tmp/bm/mon.sh" <<'EOF' || fail "copy mon.sh"
B=timestamp,index,memory.used,utilization.gpu,power.draw,power.limit,clocks.sm,clocks.max.sm,temperature.gpu
for Q in $B,clocks_event_reasons.active $B,clocks_throttle_reasons.active $B; do
  nvidia-smi --query-gpu=$Q --format=csv,noheader,nounits > /dev/null 2>&1 && break
done
echo "# $Q" > /var/tmp/bm/smi.csv
nvidia-smi --query-gpu=$Q --format=csv,noheader,nounits -lms 1000 >> /var/tmp/bm/smi.csv 2>&1 &
while :; do
  echo "$(date +%s) $(awk '/^(MemTotal|MemAvailable):/ {printf "%s ", $2}' /proc/meminfo)" >> /var/tmp/bm/mem.log
  sleep 1
done
EOF
T=60 pod "cd /var/tmp/bm && { nvidia-smi; nvidia-smi -q -d PERFORMANCE,CLOCK,POWER,TEMPERATURE; free -b; nproc; grep -m1 'model name' /proc/cpuinfo; df -h /var/tmp; } > pod-facts.txt 2>&1;
  setsid nohup sh mon.sh > /dev/null 2>&1 < /dev/null & setsid nohup python3 bench_comfy.py run jobs.json > comfy.log 2>&1 < /dev/null & echo \$! > comfy.pid; sleep 2; test -s comfy.pid" \
  < /dev/null || fail "start the runner on $NAME"
say "ComfyUI's runner started on $NAME"
pull() {  # the pod's logs and rows (with `all`, its videos), into PLAIN, sealed again
  T=900 pod "cd /var/tmp/bm && tar -cf - $([ "${1:-}" = all ] || echo "--exclude=*.mp4") pod-facts.txt comfy.log smi.csv mem.log results 2>/dev/null" \
    < /dev/null 2> /dev/null | tar -xf - -C "$PLAIN" 2> /dev/null
  seal || say "seal failed: nothing new published"
}
seen='' moved=$SECONDS kept=$SECONDS
while sleep 30; do
  read -r alive bytes disk < <(pod "cd /var/tmp/bm; kill -0 \$(cat comfy.pid) 2>/dev/null && echo alive || echo gone; cat comfy.log results/bench-comfy/progress.log 2>/dev/null | wc -c; du -sb /var/tmp/bench 2>/dev/null | cut -f1" \
    < /dev/null 2> /dev/null | tr '\n' ' '; echo)
  [ "${alive:-}" != gone ] || break
  [ "${bytes:-} ${disk:-}" = "$seen" ] || { seen="${bytes:-} ${disk:-}"; moved=$SECONDS; }
  [ $((SECONDS - kept)) -lt 240 ] || { cozy machine keepalive "$NAME" > /dev/null 2>&1; kept=$SECONDS; pull; }
  [ $((SECONDS - moved)) -lt "$STALL" ] || { pull all; fail "ComfyUI's runner on $NAME made no progress for ${STALL}s"; }
done
pull all
rows=$(cat "$PLAIN"/results/bench-comfy.jsonl 2> /dev/null | wc -l)
say "ComfyUI: $rows rows, sealed in comfyui.sealed"
# ComfyUI's processes are gone (each job stops its server); the GPU logger keeps running for Cozy's turn.
on_this ab "$AB"
pull
say "$([ "$RC" = 0 ] && echo PASS || echo FAIL): ComfyUI $rows rows (sealed), Cozy's records in ab/"
