# idle-neighbour (var-030 §9): a second model used after a first on the same GPU. Gate 44384060967 (rtx-4090): P-H3-Image's
# executor sat idle holding 23 GiB of the card's 24, and SDXL then ran in what was left, paging its own weights (20 to 30 GB
# refilled a call, no graph, 10.2 s against 3.1 s alone). One rental, its builds pinned by one `machine update`, the
# packages from a checkout with memoization out (suites/graphs `gcopy`'s edit: a repeat computes). B and A first run once
# each alone (weights on disk, kernels built), a restart after each. Then three arrangements, each from a machine restart,
# B run `calls` times with one seed:
#   fresh      B alone
#   neighbour  A once, left idle `idle_s`, then B; then A again. Once for each of `variants` (A's request shapes: one
#              whose graph records in its call leaves another executor behind than one that runs as written)
#   ended      A once, the machine restarted (no `cozy` verb ends one executor), then B
# Read, not judged: calls.tsv has each call's time, reserved and resident bytes, evictions, fills, graphs and digest (the
# weight counters are the process's, cumulative). <call>-before/after.snap is each process's ledger row and the card's use
# around the call, and ledger.log every change of them, sampled each 100 ms on the pod: who asked, who answered, what each
# gave. Fails only when a call does not complete.
# params:
#   tree       the packages_refs name the functions run from
#   a, b       {"target": "<package>/<function>", "args": [...]}: the model left idle, and the one run after it
#   tensord    true: this session's cozy as the pod's TensorD
#   runtime, tensorfs  a version, or a wheel's https URL (`#sha256=<hex>` checked): fetched here and given as a file, since
#              `machine update --runtime` takes a version or a path
#   expect     {"version", "runtime", "tensorfs"}: what `machine show` must say after the update; else the session is void
#   variants   {"<name>": [args], ...}: A's requests, a neighbour arrangement each (default: {"neighbour": a's args});
#              the first also warms A up, so it names every model A can need
#   ended      false: no `ended` arrangement
#   calls      B's calls in each arrangement (3)
#   idle_s     seconds A is left idle before B (30)
#   need_s     seconds the cap must buy (3000)
#   probe      {"file": "<package-relative .py of A's function>", "after": "<regex of the line its start goes after>",
#              "before": "<regex of the line its end goes before>", "field": "<an input field its warnings name>"}: proof
#              only, `pinprobe.py` in A's function: the residency blocks still bound, varena's pins and what holds each CUDA
#              storage alive, at its start (and again after a garbage collection) and at its end; said after each A call
. "$(dirname "$0")/lib.sh"
TREE=$(param tree) A=$(param a '{}') B=$(param b '{}') CALLS=$(param calls 3) IDLE=$(param idle_s 30) EXPECT=$(param expect '{}')
VARIANTS=$(param variants '{}') ENDED=$(param ended true)
[ "$(jq length <<<"$VARIANTS")" != 0 ] || VARIANTS=$(jq -c '{neighbour: .args}' <<<"$A")
jq -e 'to_entries | all(.[]; (.key | test("^[a-z0-9]+$")) and (.value | type == "array"))' <<<"$VARIANTS" > /dev/null || fail "variants $VARIANTS"
mapfile -t VNAMES < <(jq -r 'keys_unsorted[]' <<<"$VARIANTS")
# with <a or b> <variant>: it with that variant's args
with() { jq -c --argjson v "$VARIANTS" --arg n "$2" '.args = $v[$n]' <<<"$1"; }
[ -n "$TREE" ] && [ -d "$PACKAGES/$TREE" ] || fail "no packages checkout for tree '$TREE'"
[[ $CALLS =~ ^[1-9]$ && $IDLE =~ ^[0-9]+$ ]] || fail "calls $CALLS, idle_s $IDLE"
for m in "$A" "$B"; do
  p=$(jq -r '.target // "" | split("/") | first // ""' <<<"$m")
  [ -n "$p" ] && [ -f "$PACKAGES/$TREE/$p/pyproject.toml" ] || fail "no package $TREE/$p"
  # Memoization out, so a repeated request computes.
  { grep -rlE --include='*.py' 'memoize=' "$PACKAGES/$TREE/$p" || true; } |
    xargs -r sed -i -E -e '/^ *memoize=(True|Memoize\(requires=\["seed"\]\)),$/d' -e 's/, *memoize=True//; s/memoize=True, *//'
  say "$(jq -r .target <<<"$m"): $(git -C "$PACKAGES/$TREE" rev-parse --short HEAD) $(grep -m1 '^version' "$PACKAGES/$TREE/$p/pyproject.toml"), memoize left in $(grep -rE --include='*.py' -c 'memoize=' "$PACKAGES/$TREE/$p" | awk -F: '{n += $2} END {print n + 0}') places"
done
PROBE=$(param probe '{}')
if [ "$(jq length <<<"$PROBE")" != 0 ]; then
  FILE=$PACKAGES/$TREE/$(jq -r '.target | split("/") | first' <<<"$A")/$(jq -r '.file // ""' <<<"$PROBE")
  AFTER=$(jq -r '.after // ""' <<<"$PROBE") BEFORE=$(jq -r '.before // ""' <<<"$PROBE") FIELD=$(jq -r '.field // ""' <<<"$PROBE")
  [ -f "$FILE" ] && [ -n "$AFTER" ] && [ -n "$BEFORE" ] && [[ $FIELD =~ ^[a-z_]+$ ]] || fail "probe: file, after, before and field"
  for re in "$AFTER" "$BEFORE"; do
    [ "$(grep -cE "$re" "$FILE")" = 1 ] || fail "probe: '$re' matches $(grep -cE "$re" "$FILE") lines of $FILE, not one"
  done
  sed -i -E -e "/$BEFORE/i\\    for _line in _pinprobe(\"end\"): adj.warn(\"$FIELD\", _line)" \
    -e "/$AFTER/a\\    for _line in _pinprobe(\"start\"): adj.warn(\"$FIELD\", _line)" "$FILE"
  cat "$(dirname "$0")/pinprobe.py" >> "$FILE"
  python3 -I -c 'import ast, sys; ast.parse(open(sys.argv[1]).read())' "$FILE" || fail "$FILE does not parse with the probe"
  say "probe in ${FILE#"$PACKAGES/"}: $(grep -nE '_pinprobe\("(start|end)"\)' "$FILE" | cut -c1-90 | tr '\n' ' ')"
fi
flags=()
[ "$(param tensord false)" != true ] || flags+=(--tensord "$(command -v cozy)")
for kind in runtime tensorfs; do
  src=$(param "$kind"); [ -n "$src" ] || continue
  if [[ $src == https://* ]]; then
    url=${src%%#*} want=''; [[ $src != *'#sha256='* ]] || want=${src##*#sha256=}
    f=$OUT/wheels/$kind/$(basename "$url"); f=${f//%2B/+}
    mkdir -p "$OUT/wheels/$kind" && curl -fsSL --retry 5 -o "$f" "$url" || fail "no $kind wheel from $url"
    got=$(sha256sum "$f" | cut -d' ' -f1)
    [ -z "$want" ] || [ "$got" = "$want" ] || fail "$kind wheel ${f##*/} is sha256 $got, not $want"
    say "$kind: ${f##*/} sha256 $got"; flags+=("--$kind" "$f")
  else
    flags+=("--$kind" "$src")
  fi
done
[ "${#flags[@]}" -gt 0 ] || fail "no builds named (tensord, runtime, tensorfs)"
for m in "$A" "$B"; do kernels "$PACKAGES/$TREE/$(jq -r '.target | split("/") | first' <<<"$m")"; done
[ -f ~/.ssh/id_ed25519.pub ] || { mkdir -p ~/.ssh && ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519; } || fail "no SSH key"
RENT_ARGS=--ssh
quote "$(param need_s 3000)"
rent

# The pod's sampler: every GPU ledger page (v7: generation, answers, released, then rows of pid, held, idle, want, answered,
# last use, running, short, donated, bell) each 100 ms, and the card by nvidia-smi each 2 s; a line whenever either moved.
SAMPLER=$(base64 -w0 <<'PY'
import glob, os, struct, subprocess, sys, threading, time

HEADER, ROWS, WIDTH = 3, 32, 10
card = {"line": "card ?"}


def smi():
    while True:
        try:
            ask = lambda q: subprocess.run(["nvidia-smi", q, "--format=csv,noheader,nounits"], capture_output=True, text=True, timeout=20).stdout
            used = ask("--query-gpu=memory.used,memory.free").split("\n")[0].replace(" ", "")
            apps = ";".join(line.replace(" ", "") for line in ask("--query-compute-apps=pid,used_memory").split("\n") if line.strip())
            card["line"] = f"card used,free {used} apps {apps or '-'}"
        except Exception as error:
            card["line"] = f"card ? {type(error).__name__}"
        time.sleep(2)


def read():
    rows = []
    for path in sorted(glob.glob("/dev/shm/cozy-varena-*/v7-*")):
        if path.endswith("-host"):
            continue
        try:
            with open(path, "rb") as page:
                words = struct.unpack("<512Q", page.read(4096))
        except Exception:
            continue
        rows.append("page gen %d answers %d released %d" % words[:3])
        for i in range(ROWS):
            r = words[HEADER + i * WIDTH : HEADER + (i + 1) * WIDTH]
            if r[0] and os.path.exists(f"/proc/{r[0]}"):
                rows.append("pid %d held %d idle %d want %d answered %d running %d short %d donated %d" % (r[0], r[1], r[2], r[3], r[4], r[6], r[7], r[8]))
    return " | ".join(rows) or "no ledger"


threading.Thread(target=smi, daemon=True).start()
last, beat = None, 0.0
with open(sys.argv[1], "a") as log:
    while True:
        now, line = time.time(), read() + " | " + card["line"]
        if line != last or now - beat >= 5:
            log.write(f"{now:.3f} | {line}\n")
            log.flush()
            last, beat = line, now
        time.sleep(0.1)
PY
)
START="[ -s /var/tmp/idle-neighbour.pid ] && kill -0 \"\$(cat /var/tmp/idle-neighbour.pid)\" 2> /dev/null && { echo running; exit 0; }
echo $SAMPLER | base64 -d > /var/tmp/idle-neighbour.py
setsid nohup python3 /var/tmp/idle-neighbour.py /var/tmp/idle-neighbour.log > /var/tmp/idle-neighbour.err 2>&1 < /dev/null &
echo \$! > /var/tmp/idle-neighbour.pid; echo started"
# sampler: started unless it runs (after the rental and each restart).
sampler() {
  local try
  for try in 1 2 3 4 5 6; do
    timeout 60 cozy machine ssh "$NAME" "echo $(base64 -w0 <<<"$START") | base64 -d | sh" 2>&1 | grep -qE '^(started|running)$' && return 0
    sleep 10
  done
  fail "the pod's ledger sampler did not start"
}
# samples: the pod's whole sampler log into ledger.log, kept when the fetch returned it.
samples() {
  timeout 180 cozy machine ssh "$NAME" "cat /var/tmp/idle-neighbour.log" 2> /dev/null | grep -E '^[0-9.]+ [|] ' > "$OUT/ledger.tmp"
  [ -s "$OUT/ledger.tmp" ] && mv "$OUT/ledger.tmp" "$OUT/ledger.log"
}
before_end() { samples || say "the pod's ledger samples were not fetched before it ended"; cozy machine logs "$NAME" memory -n 2000 > "$OUT/machine-memory.log" 2>&1; }
# snap <label>: the pod's last sample, kept (<label>.snap) and said in MiB.
snap() {
  local line
  line=$(timeout 30 cozy machine ssh "$NAME" 'tail -n 1 /var/tmp/idle-neighbour.log' 2> /dev/null | grep -E '^[0-9.]+ [|] ' | tail -1)
  echo "$line" > "$OUT/$1.snap"
  say "$1: $(awk -F' [|] ' '{ s = ""; for (i = 2; i <= NF; i++) { split($i, w, " ")
      if (w[1] == "pid") s = s sprintf("pid %s held %d idle %d MiB running %s donated %d MiB; ", w[2], w[4] / 1048576, w[6] / 1048576, w[12], w[16] / 1048576)
      else if (w[1] == "card") s = s $i " MiB" }
    print (s == "" ? "no sample" : s) }' <<<"$line")"
}
printf 'call\tstatus\texec_s\tpeak_reserved_mib\tdriver_before_mib\tdriver_peak_mib\tweights_resident_mib\tweights_reserved_mib\tevictions\tfills\treclaimed_mib\treclaimed_room_mib\tfilled_mib\tfilled_n\tcapacity_mib\tgraphs\tas_written\tmemoized\tsha256\tpinned_mib\tactive_graphs\n' > "$OUT/calls.tsv"
failed=0
# call <name> <a or b>: one run of it, its record kept (<name>.json), its row in calls.tsv.
call() {
  local n=$1 m=$2 t N; t=$(jq -r .target <<<"$m"); local args; mapfile -t args < <(jq -r '.args[]' <<<"$m")
  snap "$n-before"
  cd "$PACKAGES/$TREE" || fail "no $PACKAGES/$TREE"
  step "$n" run "./$t" "${args[@]}" --on "$NAME"
  N=$(runno "$n")
  cd "$RUN_PROJECT" || fail "no $RUN_PROJECT"
  [ -z "$N" ] || cozy run watch "$N" --once --json --full > "$OUT/$n.json" 2> /dev/null
  cd "$OUT" || fail "no $OUT"
  snap "$n-after"
  jq -r --arg n "$n" '([.. | objects | select(has("weight_residency")) | .weight_residency] | first // {}) as $w
    | ([.. | objects | select(has("moved") and has("weight_residency")) | .moved | to_entries[] | select(.key != "bake") | .value] ) as $f
    | (.measurements.memory // {}) as $mem
    | [$n, (.status // "none"), ((.execution_ms // 0) / 1000), (($mem.peak_reserved_bytes // 0) / 1048576 | floor),
       (($mem.driver[0].before_bytes // 0) / 1048576 | floor), (($mem.driver[0].peak_bytes // 0) / 1048576 | floor),
       (($w.resident_bytes // 0) / 1048576 | floor), (($w.reserved_bytes // 0) / 1048576 | floor), ($w.evictions // 0), ($w.fills // 0),
       (($w.reclaimed_bytes // 0) / 1048576 | floor), (($w.reclaimed_by_reason.room // 0) / 1048576 | floor),
       (($f | map(.bytes // 0) | add // 0) / 1048576 | floor), ($f | map(.fills // 0) | add // 0),
       (([.. | objects | .chosen? // empty | .[]? | .capacity? // empty] | max // 0) / 1048576 | floor),
       ([.measurements.residency.graphs // {} | to_entries[] | "\(.key) \(.value.captures // 0)c/\(.value.replays // 0)r"] | join(", ")),
       ([.measurements.residency.graphs // {} | to_entries[] | .value.graph.eager // [] | .[]] | unique | join("; ")),
       ((.memo.memoized // false) or ([.calls[]?.memoized] | any)), ([.output[]?.sha256] | join(",")),
       (($w.pinned_bytes // 0) / 1048576 | floor), ([.measurements.residency.graphs // {} | to_entries[] | .value.active_graphs // 0] | add // 0)] | @tsv' "$OUT/$n.json" >> "$OUT/calls.tsv" 2> /dev/null
  local st; st=$(jq -r '.status // "none"' "$OUT/$n.json" 2> /dev/null)
  say "$n run ${N:-none} ${st:-none}: $(tail -n 1 "$OUT/calls.tsv" | cut -f3- | tr '\t' ' ')"
  jq -r '.. | strings | select(contains("PINPROBE"))' "$OUT/$n.json" 2> /dev/null | awk '!seen[$0]++' | cut -c1-900 |
    sed "s/^/$n: /" | tee -a "$R"
  [ "$st" = completed ] || { failed=$((failed + 1)); say "$n reason: $(jq -r '.reason.message // .reason // empty' "$OUT/$n.json" 2> /dev/null | head -c 600)"; }
}
# restart <label>: the machine's service restarts, ending every executor; the sampler again, and the card as it then is.
restart() {
  step "restart-$1" machine update "$NAME" --gpu-memory-limit off -y || fail "$1: the restart"
  sampler; sleep 5; snap "$1-restarted"
  say "$1: derived state: $(pod_derived cozy "$NAME" census 2> /dev/null || true)"
}

step update machine update "$NAME" "${flags[@]}" --gpu-memory-limit off -y || fail "the update"
cozy machine show "$NAME" --json > "$OUT/show-updated.json" 2> /dev/null
is=$(jq -c '.status | {version, runtime, tensorfs}' "$OUT/show-updated.json")
say "under test: $(jq -r '"cozy \(.version) · Runtime \(.runtime) · TensorFS \(.tensorfs)"' <<<"$is"), $(jq -r '.status | "mode \(.mode // "?"), GPU limit \([.gpus[]? | .memory_limit_bytes // 0] | join(","))"' "$OUT/show-updated.json")"
jq -e --argjson is "$is" 'to_entries | all(.[]; $is[.key] == .value)' <<<"$EXPECT" > /dev/null || fail "void: the pod runs $is, not $EXPECT"
sampler
call warm-b "$B"
restart warm-a
call warm-a "$(with "$A" "${VNAMES[0]}")"
[ "$failed" = 0 ] || fail "a warm-up call did not complete"

restart fresh
for i in $(seq "$CALLS"); do call "fresh-b$i" "$B"; done
samples

for v in "${VNAMES[@]}"; do
  restart "$v"
  call "$v-a" "$(with "$A" "$v")"
  say "$v: A left idle ${IDLE}s"; sleep "$IDLE"
  for i in $(seq "$CALLS"); do call "$v-b$i" "$B"; done
  call "$v-a2" "$(with "$A" "$v")"
  samples
done

if [ "$ENDED" = true ]; then
  restart ended-a
  call ended-a "$(with "$A" "${VNAMES[0]}")"
  restart ended
  for i in $(seq "$CALLS"); do call "ended-b$i" "$B"; done
  samples
fi
cozy machine logs "$NAME" memory -n 2000 > "$OUT/machine-memory.log" 2>&1

column -t -s $'\t' "$OUT/calls.tsv" | cut -c1-400 | tee -a "$R"
[ "$failed" = 0 ] || fail "$failed calls did not complete"
say "PASS: every call completed; calls.tsv and ledger.log hold what each arrangement did"
