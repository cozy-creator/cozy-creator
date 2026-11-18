# comfy-levels: ComfyUI alone on one rental, uncapped and at VRAM levels, with no Cozy run on the pod: the gate's ComfyUI
# pass (suites/bench/comfy.py `run`, the benchmarks runner's setup and matched arm, its --lowvram rerun after an
# out-of-memory) for one benchmarks cell, a job a level beside suites/bench/vram.py (pod.sh vram), matched to a Cozy run
# record's kernels. On the pod, nvidia-smi (each arm's start, and every second) and host memory are logged throughout.
# ComfyUI's numbers stay off this public run (Paul 11-10): everything from the pod is kept outside $OUT and published only
# as comfyui.sealed (seal.sh's scheme, to comfyui-seal.pub); this log and results.txt carry no ComfyUI figure. params:
#   bench_ref  the benchmarks commit (pod-session.yaml checks it out at $BENCH; comfyui.sh's BENCHMARKS_AT)
#   key        the benchmarks cell (h3_turbo)
#   levels     GiB left free, one job each, in order; 0 is the whole card ([6, 8, 0])
#   record     the Cozy run record ComfyUI is matched to, a file in this checkout (served kernels and Sol schedule;
#              fixtures/comfy-levels/rc-h3-6a.json: ab 44287551754's rc-h3-6a, trimmed to what bench.py `served` reads)
#   requests   [{prompt, seed}, ...]: one per tier (cold, w1, ...), each level all of them on a fresh server ([] : the
#              record's request, then its prompt with a phrase added (cells.sh `vary`) and seed + 100)
#   length     a video's seconds (15)
#   stall_s    no progress on the pod for this long ends the session (1800)
#   need_s     seconds the cap must buy (9000)
. "$(dirname "$0")/lib.sh"
KEY=$(param key h3_turbo) LEVELS=$(param levels '[6, 8, 0]') REC=$(param record release-gate/sessions/fixtures/comfy-levels/rc-h3-6a.json) REQS=$(param requests '[]') LEN=$(param length 15)
STALL=$(param stall_s 1800)
RG=$(cd "$(dirname "$0")/.." && pwd)  # release-gate
PUB=$RG/suites/bench/comfyui-seal.pub
[ -f "${BENCH:-}/bench.py" ] || fail "no benchmarks checkout at ${BENCH:-}"
grep -q "^\[models\.$KEY\]" "$BENCH/cells.toml" || fail "cells.toml has no model $KEY"
for f in suites/bench/comfy.py suites/bench/pod.sh suites/bench/vram.py suites/bench/comfyui-seal.pub; do
  [ -f "$RG/$f" ] || fail "no $f (pod-session.yaml's sparse checkout)"
done
REC=$RG/../$REC
jq -e '.measurements and .request.input.beats' "$REC" > /dev/null 2>&1 || fail "record $REC: no measurements or beats"
if [ "$(jq length <<<"$REQS")" = 0 ]; then
  REQS=$(jq -c '.request | (.input.beats[0].description | if type == "object" then .text else . end) as $p |
    [{prompt: $p, seed: .seed}, {prompt: ($p + " The scene is set in light rain."), seed: (.seed + 100)}]' "$REC")
fi
jq -e 'type == "array" and length > 0 and all(.[]; (.prompt | type == "string") and (.seed | type == "number"))' <<<"$REQS" > /dev/null ||
  fail "requests"
jq -e 'type == "array" and length > 0 and all(.[]; type == "number" and . == floor and . >= 0)' <<<"$LEVELS" > /dev/null || fail "levels"
[[ $LEN =~ ^[0-9]+$ ]] || fail "length"
# The plaintext: outside $OUT, which the workflow publishes.
PLAIN=${RUNNER_TEMP:-$OUT/..}/comfy-plain; mkdir -p "$PLAIN"
# comfy.py's jobs: one a level, the same requests, the record's kernels; 0 is the whole card.
jq -n --arg key "$KEY" --argjson levels "$LEVELS" --argjson reqs "$REQS" --argjson len "$LEN" --slurpfile rec "$REC" \
  '[$levels[] | {cell: "\($key)-\(if . == 0 then "card" else "\(.)gib" end)", key: $key,
    tiers: (["cold"] + [range(1; $reqs | length) | "w\(.)"]), requests: $reqs, length: $len,
    cap_gib: (if . == 0 then null else . end), scout_s: null, gpus: 1, exchange: "host", record: $rec[0]}]' > "$PLAIN/jobs.json" ||
  fail "jobs"
say "benchmarks $(git -C "$BENCH" rev-parse --short HEAD): $KEY at $(jq -c . <<<"$LEVELS") GiB free (0: the card), $(jq length <<<"$REQS") requests a level, matched to run $(jq -r .id "$REC")"
seal() {
  local key; key=$(openssl rand -hex 32)
  tar -C "$PLAIN" -czf - . | openssl enc -aes-256-cbc -pbkdf2 -pass "pass:$key" -out "$OUT/comfyui.sealed.new" &&
    printf %s "$key" | openssl pkeyutl -encrypt -pubin -inkey "$PUB" -pkeyopt rsa_padding_mode:oaep -out "$OUT/comfyui.sealed.key.new" &&
    mv "$OUT/comfyui.sealed.new" "$OUT/comfyui.sealed" && mv "$OUT/comfyui.sealed.key.new" "$OUT/comfyui.sealed.key"
}
seal || fail "seal"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param need_s 9000)"
rent

pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
T=900 pod "command -v ffprobe >/dev/null || { apt-get update -qq && apt-get install -y -qq ffmpeg; } >/dev/null 2>&1; ffprobe -version | head -1;
  python3 -c 'import sys; assert sys.version_info >= (3, 11), sys.version; print(\"python\", sys.version.split()[0])'; git --version" \
  < /dev/null >> "$R" 2>&1 || fail "the pod lacks ffprobe, python3 >= 3.11 or git"
tar -C "$BENCH" -cz --exclude .git --exclude results . | T=600 pod "rm -rf /var/tmp/bm && mkdir -p /var/tmp/bm && tar -xz -C /var/tmp/bm" ||
  fail "copy the runner"
{ T=60 pod "cat > /var/tmp/bm/bench_comfy.py" < "$RG/suites/bench/comfy.py" && T=60 pod "cat > /var/tmp/bench-pod.sh" < "$RG/suites/bench/pod.sh" &&
  T=60 pod "cat > /var/tmp/bench-vram.py" < "$RG/suites/bench/vram.py" && T=60 pod "cat > /var/tmp/bm/jobs.json" < "$PLAIN/jobs.json"; } ||
  fail "copy comfy.py, pod.sh, vram.py and the jobs"
# comfy.py run, with GPU 0 as nvidia-smi reads it at the start of each arm (after its level's holder stood).
T=60 pod "cat > /var/tmp/bm/levels.py" <<'EOF' || fail "copy levels.py"
import os, subprocess, sys, time
sys.path.insert(0, '/var/tmp/bm')
import bench
import bench_comfy

def q(*a):
    return subprocess.run(['nvidia-smi', '-i', '0', *a, '--format=csv,noheader,nounits'], capture_output=True, text=True).stdout.strip().replace('\n', '; ')

def smi(what):
    ready = '/var/tmp/bench-vram.ready'
    held = open(ready).read().strip() if os.path.exists(ready) else 'none'
    with open('/var/tmp/bm/levels.log', 'a') as f:
        f.write(f"{time.strftime('%H:%M:%S', time.gmtime())} {what}: nvidia-smi total, used, free MiB {q('--query-gpu=memory.total,memory.used,memory.free')}; "
                f"apps {q('--query-compute-apps=pid,process_name,used_memory') or 'none listed'}; holder (pid held free total others, bytes) {held}\n")

arm, match = bench.comfy_arm, bench.match
def comfy_arm(s, m, key, a, tiers, reqs, length, got, sampler, roots, base, go_on=None):
    flags = 'its own flags' if bench.match is match else '--lowvram (comfy.py rerun)'  # comfy.py swaps bench.match for its rerun
    smi(f"arm {base.get('cell')} (cap {base.get('cap_gib')} GiB) on {flags} starts")
    return arm(s, m, key, a, tiers, reqs, length, got, sampler, roots, base, go_on)
bench.comfy_arm = comfy_arm
smi('start')
bench_comfy.run(sys.argv[1])
smi('end')
EOF
T=60 pod "cat > /var/tmp/bm/mon.sh" <<'EOF' || fail "copy mon.sh"
# Every second: GPU 0 (nvidia-smi, its fields on the first line) and host memory (kB: MemTotal, MemAvailable, SwapTotal,
# SwapFree; ComfyUI's RSS). The slowdown reasons' field has two names across drivers.
B=timestamp,memory.total,memory.used,memory.free,utilization.gpu,power.draw,power.limit,clocks.sm,temperature.gpu
for Q in $B,clocks_event_reasons.active,pcie.link.gen.current,pcie.link.width.current \
    $B,clocks_throttle_reasons.active,pcie.link.gen.current,pcie.link.width.current $B; do
  nvidia-smi -i 0 --query-gpu=$Q --format=csv,noheader,nounits > /dev/null 2>&1 && break
done
echo "# $Q" > /var/tmp/bm/smi.csv
nvidia-smi -i 0 --query-gpu=$Q --format=csv,noheader,nounits -lms 1000 >> /var/tmp/bm/smi.csv 2>&1 &
while :; do
  p=$(pgrep -f 'main.py --listen' | head -1)
  echo "$(date +%s) $(awk '/^(MemTotal|MemAvailable|SwapTotal|SwapFree):/ {printf "%s ", $2}' /proc/meminfo)$( [ -n "$p" ] && awk '/^VmRSS:/ {print $2}' /proc/$p/status 2>/dev/null)" >> /var/tmp/bm/mem.log
  sleep 1
done
EOF
T=60 pod "cd /var/tmp/bm && { nvidia-smi; nvidia-smi -q -d PERFORMANCE,CLOCK,POWER,TEMPERATURE; free -b; nproc; cat /proc/cpuinfo | grep -m1 'model name'; df -h /var/tmp; } > pod-facts.txt 2>&1;
  setsid nohup sh mon.sh > /dev/null 2>&1 < /dev/null & setsid nohup python3 levels.py jobs.json > comfy.log 2>&1 < /dev/null & echo \$! > comfy.pid; sleep 2; test -s comfy.pid" \
  < /dev/null || fail "start the runner on $NAME"
say "runner started on $NAME"
pull() {  # the pod's logs, rows and (with `all`) outputs, into PLAIN, sealed again
  T=900 pod "cd /var/tmp/bm && tar -cf - $([ "${1:-}" = all ] || echo "--exclude=*.mp4") pod-facts.txt comfy.log levels.log smi.csv mem.log results 2>/dev/null" \
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
  [ $((SECONDS - moved)) -lt "$STALL" ] || { pull all; fail "the runner on $NAME made no progress for ${STALL}s"; }
done
pull all
rows=$(cat "$PLAIN"/results/bench-comfy.jsonl 2> /dev/null | wc -l)
[ "$rows" -gt 0 ] || fail "no ComfyUI rows came back (sealed: comfy.log)"
say "PASS: $rows ComfyUI rows, sealed in comfyui.sealed (seal.sh's header says how to open it)"
