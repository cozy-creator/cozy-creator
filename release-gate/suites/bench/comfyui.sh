# Suite bench's ComfyUI pass (bench.sh --comfyui; gate.sh sources it after cells.sh): ComfyUI at the benchmarks runner's pins
# on this pod, under the same host RAM, for the cells whose `comfyui` column names a benchmarks cell, with their prompts and
# seeds. Its rows go to comfyui-rows.json, which bench.py judges beside the cells and puts in the next baseline.
BENCHMARKS=https://github.com/cozy-creator/benchmarks.git BENCHMARKS_AT=72a02800719b963f1fa96ab9d0fbdab9acc7310f
COMFY_PY=(nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/suites/bench/comfy.py")
# cells.sh's retried, for a gate that sourced an older cells.sh.
declare -F retried > /dev/null || retried() { local w=2 t=$SECONDS; until "$@"; do [ $((SECONDS - t)) -lt 300 ] || return 1; sleep $((w + RANDOM % (w + 1))); w=$((w * 2 > 60 ? 60 : w * 2)); done; }
"${COMFY_PY[@]}" jobs "$OUT" > comfy-jobs.json 2> comfy-jobs.err || { expect comfyui-jobs FAIL "$(tail -c 300 comfy-jobs.err)"; return 0; }
if [ "$(jq length comfy-jobs.json)" = 0 ]; then log "comfyui: no completed cell of this group names a ComfyUI cell"; return 0; fi
# The runner is private: CI checks it out with its deploy key (release-gate.yml, BENCHMARKS_DIR); elsewhere this computer's
# git credentials clone it.
BM=${BENCHMARKS_DIR:-benchmarks}
{ [ -d "$BM/.git" ] || git clone -q --filter=blob:none --no-checkout "$BENCHMARKS" "$BM"; } &&
  { git -C "$BM" cat-file -e "$BENCHMARKS_AT^{commit}" 2> /dev/null || git -C "$BM" fetch -q origin "$BENCHMARKS_AT"; } &&
  git -C "$BM" archive "$BENCHMARKS_AT" | "$CLI" machine ssh "$M" "rm -rf /var/tmp/bm && mkdir -p /var/tmp/bm && tar -xf - -C /var/tmp/bm" &&
  "$CLI" machine ssh "$M" "cat > /var/tmp/bm/bench_comfy.py" < "$HERE/suites/bench/comfy.py" &&
  "$CLI" machine ssh "$M" "cat > /var/tmp/bm/jobs.json" < comfy-jobs.json ||
  { expect comfyui-setup FAIL "the benchmarks runner at $BENCHMARKS_AT did not reach $M"; return 0; }
# Gated ComfyUI files (Klein): the laptop's Hugging Face token, over stdin (never an argument or a log line), which comfy.py
# hands to the weights step alone and deletes before ComfyUI starts.
HF_FILE=${HF_HOME:-$HOME/.cache/huggingface}/token
if [ -n "${HF_TOKEN:-}" ]; then printf %s "$HF_TOKEN" | "$CLI" machine ssh "$M" "umask 077 && cat > /var/tmp/bm/.hf-token" || log "comfyui: the HF token did not reach $M"
elif [ -s "$HF_FILE" ]; then "$CLI" machine ssh "$M" "umask 077 && cat > /var/tmp/bm/.hf-token" < "$HF_FILE" || log "comfyui: the HF token did not reach $M"
else log "comfyui: no HF token here: gated ComfyUI files (Klein) cannot download"; fi
# One restart: Cozy's executors leave the GPU to ComfyUI.
retried "$CLI" machine update "$M" --gpu-memory-limit off -y > comfy-free.log 2>&1 || log "comfyui: freeing the GPU: $(tail -c 300 comfy-free.log)"
# The machine's restart above can leave its SSH unreachable for a few minutes (a PRO 6000, 43039533939: "Connection timed
# out", and the pass never started): the runner's start is retried until its pid file is there.
started() { podsh "test -s /var/tmp/bm/comfy.pid || { cd /var/tmp/bm && setsid nohup python3 bench_comfy.py run jobs.json > comfy.log 2>&1 < /dev/null & echo \$! > /var/tmp/bm/comfy.pid; }; test -s /var/tmp/bm/comfy.pid" > /dev/null 2>&1; }
retried started || { expect comfyui-start FAIL "the benchmarks runner did not start on $M (its SSH did not answer for 5 minutes)"; return 0; }
log "comfyui: $(jq -r 'map(.cell + " as " + .key) | join(", ")' comfy-jobs.json) on $M"
date +%s%3N > comfy-window  # its start and end: bench.py sets the pass aside when another controller's run overlapped it
# No Cozy run moves meanwhile, so gate.sh's hold loop renews nothing: this loop keeps the rental, and stops ComfyUI if its log
# and its files stand still for 20 minutes.
seen= moved=$SECONDS
while sleep 30; do
  "$CLI" machine keepalive "$M" > /dev/null 2>&1
  read -r alive bytes disk < <(podsh "cd /var/tmp/bm; kill -0 \$(cat comfy.pid) 2>/dev/null && echo alive || echo gone; wc -c < comfy.log; du -sb /var/tmp/bench 2>/dev/null | cut -f1" | tr '\n' ' '; echo)
  [ "${alive:-}" != gone ] || break
  [ "${bytes:-} ${disk:-}" = "$seen" ] || { seen="${bytes:-} ${disk:-}"; moved=$SECONDS; }
  if (( SECONDS - moved >= 1200 )); then
    expect comfyui-progress FAIL "ComfyUI stood still 20 min: $(podsh 'tail -c 300 /var/tmp/bm/comfy.log' | tr '\n' ' ')"
    podsh "kill \$(cat /var/tmp/bm/comfy.pid)" > /dev/null 2>&1
    break
  fi
done
date +%s%3N >> comfy-window
mkdir -p comfyui
"$CLI" machine ssh "$M" "tar -cf - -C /var/tmp/bm comfy.log -C results bench-comfy.jsonl bench-comfy" | tar -xf - -C comfyui
"${COMFY_PY[@]}" rows "$OUT" > comfyui-rows.json
# In CI (a public log) ComfyUI's numbers stay in comfyui-rows.json, which release-gate.yml seals (seal.sh).
if [ -n "${GITHUB_ACTIONS:-}" ]; then log "comfyui: rows for $(jq -r 'keys | join(", ")' comfyui-rows.json) in comfyui-rows.json"
else log "comfyui: $(jq -c 'map_values({total_s, cold_s, status})' comfyui-rows.json)"; fi
podsh "cat /var/tmp/memavail.log" > memavail.log 2>/dev/null
