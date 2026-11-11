# Suite bench's ComfyUI pass (bench.sh --comfyui; gate.sh sources it after cells.sh): ComfyUI at the benchmarks runner's pins
# on this pod, under the same host RAM, for the cells whose `comfyui` column names a benchmarks cell, with their prompts and
# seeds. Its rows go to comfyui-rows.json, which bench.py judges beside the cells and puts in the next baseline.
BENCHMARKS=https://github.com/cozy-creator/benchmarks.git BENCHMARKS_AT=72a02800719b963f1fa96ab9d0fbdab9acc7310f
COMFY_PY=(nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/suites/bench/comfy.py")
"${COMFY_PY[@]}" jobs "$OUT" > comfy-jobs.json 2> comfy-jobs.err || { expect comfyui-jobs FAIL "$(tail -c 300 comfy-jobs.err)"; return 0; }
if [ "$(jq length comfy-jobs.json)" = 0 ]; then log "comfyui: no completed cell of this group names a ComfyUI cell"; return 0; fi
{ [ -d benchmarks/.git ] || git clone -q --filter=blob:none --no-checkout "$BENCHMARKS" benchmarks; } && git -C benchmarks fetch -q origin "$BENCHMARKS_AT" &&
  git -C benchmarks archive "$BENCHMARKS_AT" | "$CLI" machine ssh "$M" "rm -rf /var/tmp/bm && mkdir -p /var/tmp/bm && tar -xf - -C /var/tmp/bm" &&
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
"$CLI" machine update "$M" --gpu-memory-limit off -y > comfy-free.log 2>&1 || log "comfyui: freeing the GPU: $(tail -c 300 comfy-free.log)"
podsh "cd /var/tmp/bm && setsid nohup python3 bench_comfy.py run jobs.json > comfy.log 2>&1 < /dev/null & echo \$! > /var/tmp/bm/comfy.pid" > /dev/null
log "comfyui: $(jq -r 'map(.cell + " as " + .key) | join(", ")' comfy-jobs.json) on $M"
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
mkdir -p comfyui
"$CLI" machine ssh "$M" "tar -cf - -C /var/tmp/bm comfy.log -C results bench-comfy.jsonl bench-comfy" | tar -xf - -C comfyui
"${COMFY_PY[@]}" rows "$OUT" > comfyui-rows.json
log "comfyui: $(jq -c 'map_values({total_s, cold_s, status})' comfyui-rows.json)"
podsh "cat /var/tmp/memavail.log" > memavail.log 2>/dev/null
