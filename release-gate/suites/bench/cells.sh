# Suite `bench` (README.md): this pod's group of cells.tsv, each a cold run on its fixed seed and its warm runs on fresh ones,
# between two runs of the reference workload, on a host held to the group's RAM (`skus`). bench.tsv names every run;
# bench.py judges them (check.py in analyze.py; bench.sh's table across groups).
BG=${GROUP:-${SKU}x${GPUS:-1}} FX=$HERE/suites/bench/fixtures
echo "$BG" > bench-group; : > bench.tsv
read -r _ HOST_GIB _ < <(awk -v g="$BG" '$1 == g' "$HERE/suites/bench/skus")
[ -n "${HOST_GIB:-}" ] || { expect bench-group FAIL "no group $BG in suites/bench/skus"; return 0; }
BENCH_PY=(nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/bench.py")
# Never the caller's stdin: inside the cell loop that is cells.tsv, which an ssh would swallow after the first cell (1a, 11-10).
podsh() { timeout 900 "$CLI" machine ssh "$M" "$1" < /dev/null; }
# The pod side's scripts, then the host's RAM held to the group's: the rest of the pod's limit is a hog no cozy process can
# take back. One attempt: a pod it cannot hold runs its cells anyway, labelled in host-ram and the table (coordinator 11-10).
podsh "echo $(base64 -w0 "$HERE/suites/bench/pod.sh") | base64 -d > /var/tmp/bench-pod.sh && echo $(base64 -w0 "$HERE/suites/bench/hog.py") |
  base64 -d > /var/tmp/bench-hog.py && echo $(base64 -w0 "$HERE/suites/bench/vram.py") | base64 -d > /var/tmp/bench-vram.py" > pod-copy.log 2>&1 ||
  log "copying the pod scripts: $(tr '\n\r' '  ' < pod-copy.log | tail -c 300)"
if timeout 360 "$CLI" machine ssh "$M" "sh /var/tmp/bench-pod.sh hold $(awk -v g="$HOST_GIB" 'BEGIN {printf "%d", g * 2^30}')" < /dev/null > host.txt 2>&1; then
  echo "held to $HOST_GIB GiB ($(grep -h '^hog ' host.txt))$(grep -h '^SHORT' host.txt | sed 's/^/; /')" > host-ram
  log "host held to $HOST_GIB GiB: $(tr '\n\r' '  ' < host.txt)"
else
  timeout 120 "$CLI" machine ssh "$M" "sh /var/tmp/bench-pod.sh release" < /dev/null > /dev/null 2>&1
  echo "NOT held to $HOST_GIB GiB: $(tr '\n\r' '  ' < host.txt | tail -c 300)" > host-ram
  expect bench-host FAIL "host RAM not held, the cells run on the pod's own: $(cat host-ram)"
fi
podsh "sh /var/tmp/bench-pod.sh gpu" > pod-facts.txt 2> /dev/null || log "no GPU facts: $(tail -c 200 pod-facts.txt)"
# The pictures cells.tsv names in prompts, as project assets.
for a in "$HERE/ref/seed.jpg still" "$HERE/ref/seed.jpg lead" "$FX/lighthouse.jpg photo" "$FX/teapot.jpg teapot"; do
  "$CLI" project -p "$RUN_PROJECT" import ${a% *} ${a#* } < /dev/null >> import.log 2>&1 || log "project import ${a#* }: $(tail -c 200 import.log)"
done
ln -sfn "$HERE/suites/bench/calibrate" "$OUT/bench-calibrate"
ONLY_ASKED=$ONLY ONLY=  # --only names bench cells, each run of which is a gate cell of its own name
cell calibrate-before ./bench-calibrate/measure seed:="$R"
if ! "${BENCH_PY[@]}" admit "$OUT" > admit.txt 2>&1; then
  if [ "$HOST" -lt 3 ] && [ -z "$ON" ]; then
    # Another class of pod: ended, and the gate starts again on the next host (gate.sh `again`).
    ONLY=$ONLY_ASKED; echo "pod out of range: $(paste -sd';' admit.txt)" > host-slow
    expect bench-pod FAIL "$(cat host-slow)"
    exit 1
  fi
  # The third host, or a pod a previous gate kept (--on), runs whatever it is: labelled, and what runs on the same pod
  # still compares.
  expect bench-pod FAIL "host $HOST${ON:+ (kept $ON)} runs anyway, out of range: $(paste -sd';' admit.txt)"
else
  expect bench-pod PASS "$(paste -sd';' admit.txt)"
fi
# A capped cell runs on a card of that size, as ComfyUI does: Cozy's executors leave the GPU (one restart), then a holder
# process takes all but `cap` GiB (vram.py). Each run's row carries the holder's MiB, which bench.py takes off its peak.
CAPNOW=off HELD=0 LEVEL_OK=1
# vary <args> <i>: a ComfyUI-matched cell's warm run i gets its own prompt, the cold run's and a fixed phrase, so neither
# engine reuses the last request's text encoding (Paul 09-30); comfy.py hands ComfyUI each run's prompt from its record.
VARY=("at dawn" "in light rain" "in deep winter" "under a full moon" "in late autumn")
vary() {
  local v=${VARY[$((($2 - 1) % ${#VARY[@]}))]}
  sed -E "s/prompt=\"([^\"]*)\"/prompt=\"\\1, $v\"/; s/\"description\":\"([^\"]*)\"/\"description\":\"\\1 The scene is set $v.\"/" <<< "$1"
}
while IFS=$'\t' read -r name group cap warm seed target args before _ _ _ comfy _; do
  [ "${name:0:1}" != '#' ] && [ "$group" = "$BG" ] || continue
  [ -z "$ONLY_ASKED" ] || [[ $ONLY_ASKED == *",$name,"* ]] || continue
  want=off; [ "$cap" = - ] || want=$cap
  if [ "$want" != "$CAPNOW" ]; then
    "$CLI" machine update "$M" --gpu-memory-limit off -y < /dev/null > "restart-$want.log" 2>&1 || log "restart for $want: $(tail -c 300 "restart-$want.log")"
    HELD=0 LEVEL_OK=1
    if [ "$want" = off ]; then
      podsh "sh /var/tmp/bench-pod.sh vram off" > /dev/null 2>&1
    elif line=$(podsh "sh /var/tmp/bench-pod.sh vram $((want * 1073741824))" 2>&1) && read -r _ held free _ <<< "$line" && [ "${held:-x}" -ge 0 ] 2>/dev/null; then
      HELD=$((held / 1048576))
      log "a ${want} GiB card: holder $HELD MiB, $((free / 1048576)) MiB free ($line)"
    else
      LEVEL_OK=0; log "no ${want} GiB holder: $line"
    fi
    CAPNOW=$want
  fi
  if [ "$LEVEL_OK" = 0 ]; then expect "$name" FAIL "no holder for a ${want} GiB card"; continue; fi
  t=$(pkg "${target%/*}" "${target##*/}")
  for ((i = 0; i <= warm; i++)); do
    s=; [ "$seed" = - ] || s="seed:=$((seed + 100000 * i))"
    if [ "$before" != - ]; then
      b=${before%% *}
      eval "cell \"\$name-before-\$i\" \"\$(pkg \${b%/*} \${b##*/})\" ${before#* } seed:=$((RANDOM + 1000000))"
    fi
    a=$args; [ "$comfy" = - ] || [ "$i" = 0 ] || a=$(vary "$args" "$i")
    eval "cell \"\$name-\$i\" \"\$t\" $a $s"
    printf '%s\t%s\t%s\t%s\n' "$name" "$([ "$i" = 0 ] && echo cold || echo warm)" "$name-$i" "$HELD" >> bench.tsv
  done
  read -r held want_bytes < <(podsh "sh /var/tmp/bench-pod.sh check" 2>/dev/null)
  [ "${held:-0}" -ge "${want_bytes:-0}" ] || expect "$name-host" FAIL "the host hog shrank to ${held:-?} of ${want_bytes:-?} bytes: this cell ran on more RAM"
done < "$HERE/suites/bench/cells.tsv"
[ "$CAPNOW" = off ] || podsh "sh /var/tmp/bench-pod.sh vram off" > /dev/null 2>&1
cell calibrate-after ./bench-calibrate/measure seed:="$R"
ONLY=$ONLY_ASKED
podsh "cat /var/tmp/memavail.log" > memavail.log 2>/dev/null  # the hog stays: comfyui.sh runs under it, and the pod ends after
