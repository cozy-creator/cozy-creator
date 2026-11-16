# prepare-ahead (cl-905, Paul 11-12: "this video node should be trying to download minimax h3 immediately, rather than sitting there
# waiting"; "everything should overlap as much as possible"): one cold rental, a project's picture and the video box that mentions it,
# rendered together through this computer's Calcifer (its walker) on the rental alone. The picture's run is sent at once. The video
# waits for its take, and its fetch ahead (a warm run) installs H3 and downloads its weights behind the picture's, in the machine's
# install and download lines. When the take lands, H3's run starts on weights already there. prepare-ahead.py reads the runs' records,
# the board and TensorD's log (each install turn, each object's start and landing with its order) into timeline.json: each job's spans
# in each line, seconds from the ask, end to end against the longest single chain, and the wait H3 saved. The session's cozy (a dev
# build of the RC) puts its TensorD on the pod; the pod keeps its image's released Runtime, whose kernels the times need (RUNTIME_WHEEL
# is a pure wheel built from source, with none: a pod on it runs slowly and says nothing). params:
#   picture   the picture's tool (default qwen-image-2/image), at `megapixels` (default 1)
#   video     the video's tool (default minimax-h3/generate, its default lane and turbo), `seconds` long (default 15)
#   update    `cozy machine update` flags for the pod after its TensorD: a candidate's Runtime and TensorFS, each a dev build's
#             wheel with its kernels (["--runtime", "artifact:<run id>:<artifact>:cozy-runtime", "--tensorfs",
#             "artifact:<run id>:<artifact>"]), fetched here before renting ([]: its image's)
#   arms      [{"name": "base", "tensord": "artifact:<run id>:cozy-<sha>", "update": […]}, …]: each in turn on the same rental, in a
#             project of its own, so each starts cold (a project's weights and environments are its own, #2858): its TensorD from
#             that dev build's cozy (absent: the session's), then its `update`; its files under arm-<name>/ and compare.json
#             side by side ([]: one arm, the session's cozy and `update`)
#   reset     true: before each arm, the pod's TensorFS store is emptied and its executors ended, over `cozy machine ssh` (no
#             verb empties a rented machine's store: `cozy model uninstall` is this computer's only), so each arm starts cold;
#             projects share the machine's store of public weights; and the picture's and video's packages uninstalled
#             (`cozy package uninstall`), so each arm installs them afresh: a later arm would otherwise update the first's
#             environments in a second (43683143359)
#   sample    true: the pod's disk (each device's bytes read and written), I/O and memory pressure and page cache every 0.5 s
#             for the session, over SSH (samples.jsonl in each arm): what a running call's loads met while downloads wrote
#   link_mbit the pod's download link shaped to this many Mbit/s for the whole session (tc police on its ingress, over SSH);
#             a pod that refuses it is said and runs unshaped ("": as rented)
. "$(dirname "$0")/lib.sh"
PICTURE=$(param picture qwen-image-2/image) MP=$(param megapixels 1) VIDEO=$(param video minimax-h3/generate) SECONDS_LONG=$(param seconds 15)
ARMS=$(param arms '[]')
[ "$(jq length <<<"$ARMS")" -gt 0 ] || ARMS=$(jq -nc --argjson update "$(param update '[]')" '[{name: "", update: $update}]')
mapfile -t names < <(jq -r '.[].name' <<<"$ARMS")
# artifact:<run id>:<artifact>[:<repo>] fetched into <dir>: the file in it matching <pattern>.
fetched() {
  IFS=: read -r _ rid name repo <<<"$1"; mkdir -p "$2"
  gh run download "$rid" -R "cozy-creator/${repo:-cozy-creator}" -n "$name" -D "$2" > /dev/null 2>&1 || return 1
  find "$2" -type f | grep -m1 -E "$3"
}
# Each arm's TensorD and update, fetched before renting, so a missing one costs nothing.
declare -A TENSORD=() UPDATE=()
for i in "${!names[@]}"; do
  a=${names[$i]}; [[ -z $a && ${#names[@]} = 1 || $a =~ ^[a-z0-9-]+$ ]] || fail "arm name '$a'"
  t=$(jq -r --argjson i "$i" '.[$i].tensord // empty' <<<"$ARMS")
  if [ -n "$t" ]; then
    b=$(fetched "$t" "$OUT/arms/$a/tensord" 'cozy-linux-x86_64$') || fail "arm $a: no cozy-linux-x86_64 in $t"
    chmod +x "$b"; TENSORD[$a]=$b
  else
    TENSORD[$a]=$(command -v cozy)
  fi
  mapfile -t update < <(jq -r --argjson i "$i" '.[$i].update // [] | .[]' <<<"$ARMS")
  for j in "${!update[@]}"; do
    v=${update[$j]}; [[ $v == artifact:* ]] || continue
    update[$j]=$(fetched "$v" "$OUT/arms/$a/update/$j" 'manylinux.*x86_64.*\.whl$') || fail "arm $a: $v has no Linux x86_64 wheel"
  done
  UPDATE[$a]=${update[*]:-}
  say "arm ${a:-(one)}: TensorD $("${TENSORD[$a]}" --version 2> /dev/null | awk '{print $2}')${UPDATE[$a]:+, then ${UPDATE[$a]//$OUT\//}}"
done
jq -n --arg picture "$PICTURE" --argjson mp "$MP" --arg video "$VIDEO" --argjson seconds "$SECONDS_LONG" '[
  {name: "pier", tool: $picture, kind: "image", fields: {megapixels: $mp, aspect_ratio: "16:9",
    instructions: "A weathered wooden pier at dusk, warm lamps along its rail, gulls overhead and a calm grey sea. Photo.",
    description: "A weathered wooden pier at dusk, warm lamps along its rail."}},
  {name: "walk", tool: $video, kind: "video", fields: {aspect_ratio: "16:9",
    summary: "A woman in a yellow raincoat walks the length of <pier>.",
    soundscape: "Waves lapping at the posts, gulls, footsteps on wet wood.",
    beats: [{duration: $seconds, description: "A tracking shot follows a woman in a yellow raincoat as she walks along <pier> toward the lamps at its far end, gulls circling overhead."}]}}]' > "$OUT/boxes.json"
# Each arm's project and its boxes, before renting: a project of its own starts cold, its weights and environments its own (#2858).
for ARM in "${names[@]}"; do
  D=$OUT${ARM:+/arm-$ARM}; mkdir -p "$D"
  [ -z "$ARM" ] || run_project "$OUT/project-$ARM" > "$D/project.json" 2>&1 || fail "arm $ARM: its project: $(tail -c 300 "$D/project.json")"
  run_page boxes "$OUT/boxes.json" > "$D/boxes.txt" 2>&1 || fail "${ARM:+arm $ARM: }the project's boxes: $(tail -3 "$D/boxes.txt")"
done
say "the session's cozy: $(cozy --version)"
RESET=$(param reset false) LINK=$(param link_mbit '') SAMPLE=$(param sample false)
[[ -z $LINK || $LINK =~ ^[0-9]+$ ]] || fail "link_mbit $LINK"
if [ "$RESET" = true ] || [ -n "$LINK" ] || [ "$SAMPLE" = true ]; then
  [ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
  RENT_ARGS=--ssh
fi
quote "$(param need_s $((2700 * ${#names[@]})))"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
if [ "$RESET" = true ] || [ -n "$LINK" ] || [ "$SAMPLE" = true ]; then
  for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
fi
if [ -n "$LINK" ]; then
  # Policed ingress: what comes faster is dropped, and TCP settles under it, as on a slower link.
  dev=$(pod "ip route show default | awk '{print \$5; exit}'" < /dev/null 2> /dev/null | tr -d '\r')
  if pod "tc qdisc add dev $dev handle ffff: ingress && tc filter add dev $dev parent ffff: protocol ip u32 match u32 0 0 police rate ${LINK}mbit burst 4m drop flowid :1" \
      < /dev/null > "$OUT/link.txt" 2>&1; then
    say "the pod's link ($dev) is shaped to $LINK Mbit/s"
  else
    say "the pod refused the shaped link: $(tail -c 200 "$OUT/link.txt"); it runs as rented"; LINK=''
  fi
fi
SAMPLE_PY=$(cat <<'PY'
import json, os, time
out = open("/tmp/samples.jsonl", "a")
while not os.path.exists("/tmp/sample.stop"):
    row = {"t": round(time.time(), 3), "disk": {}, "psi": {}, "mem": {}}
    for line in open("/proc/diskstats"):
        f = line.split()
        if not f[2].startswith(("loop", "ram")) and int(f[3]) + int(f[7]):
            row["disk"][f[2]] = [int(f[5]) * 512, int(f[9]) * 512]
    for res in ("io", "memory"):
        try:
            row["psi"][res] = {l.split()[0]: float(l.split()[1].split("=")[1]) for l in open(f"/proc/pressure/{res}")}
        except (OSError, IndexError, ValueError):
            pass
    for line in open("/proc/meminfo"):
        k, v = line.split(":", 1)
        if k in ("MemAvailable", "Cached", "Dirty", "Writeback"):
            row["mem"][k] = int(v.split()[0]) * 1024
    out.write(json.dumps(row) + "\n"); out.flush()
    time.sleep(0.5)
PY
)
if [ "$SAMPLE" = true ]; then
  # Held by an SSH call of its own for the session, as cold-start's is (one started detached never wrote a row).
  T=60 pod "rm -f /tmp/sample.stop /tmp/samples.jsonl" < /dev/null || fail "the sampler's files"
  (printf '%s' "$SAMPLE_PY" | T=9000 pod "cat > /tmp/sample.py && python3 /tmp/sample.py 2> /tmp/sample.err" > /dev/null 2>&1) &
  say "sampling the pod's disk, pressure and page cache"
fi

for ARM in "${names[@]}"; do
  D=$OUT${ARM:+/arm-$ARM} TAG=${ARM:+-$ARM} BEGAN=$(date +%s)
  [ -z "$ARM" ] || RUN_PROJECT=$OUT/project-$ARM
  if [ "$RESET" = true ]; then
    T=600 pod "cat > /tmp/reset.py && python3 /tmp/reset.py" < "$(dirname "$0")/empty-store.py" > "$D/reset.json" 2>&1 || fail "${ARM:+arm $ARM: }the pod's store wasn't emptied: $(tail -c 300 "$D/reset.json")"
    say "${ARM:+arm $ARM: }the pod's store emptied: $(tail -c 400 "$D/reset.json")"
    step "uninstall$TAG" package uninstall "${PICTURE%%/*}" "${VIDEO%%/*}" --on "$NAME" -y || say "${ARM:+arm $ARM: }the packages weren't uninstalled"
  fi
  step "tensord$TAG" machine update "$NAME" --tensord "${TENSORD[$ARM]}" -y || fail "the pod refused ${ARM:-the session}'s TensorD"
  v=$(cozy machine show "$NAME" --json | jq -r .status.version)
  [ "$v" = "$("${TENSORD[$ARM]}" --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not ${ARM:-the session}'s"
  read -ra update <<<"${UPDATE[$ARM]}"
  [ ${#update[@]} -eq 0 ] || step "candidate$TAG" machine update "$NAME" "${update[@]}" -y || fail "the pod refused ${ARM:-the candidate}'s Runtime and TensorFS"
  say "pod TensorD $v, Runtime $(cozy machine show "$NAME" --json | jq -r '.status.runtime // empty')"

  # The board as the page sees it, every 5 s: when the video's fetch ahead began, and what each box said.
  ( while :; do printf '{"at_ms":%s,"board":%s}\n' "$(date +%s%3N)" "$(run_page board 2> /dev/null || echo null)" >> "$D/board.ndjson"; sleep 5; done ) & BG=$!
  ASK=$(date +%s%3N)
  say "step render: pier and walk asked together on $NAME"
  timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" pier walk > "$D/render.txt" 2>&1; rendered=$?
  kill "$BG" 2> /dev/null; BG=''
  tail -4 "$D/render.txt" | cut -c1-300 | sed 's/^/    /' >> "$R"

  (cd "$RUN_PROJECT" && cozy --json run ls --all --limit 40) > "$D/runs.json" 2>&1
  for n in $(jq -r --argjson a "$ASK" '.runs[]? | select(.created_at_ms >= $a - 60000) | .number' "$D/runs.json"); do
    (cd "$RUN_PROJECT" && cozy run watch "$n" --once --json --full) > "$D/run-$n.json" 2>&1
  done
  # The whole log since the rental began (the default is its last 200 lines), and the transfer log beside it.
  cozy machine logs "$NAME" --lines 0 --since "$(( ($(date +%s) - BEGAN) / 60 + 2 ))m" > "$D/tensord.log" 2>&1 || say "the pod's TensorD log wasn't read"
  cozy machine logs "$NAME" tensorfs --lines 0 --since "$(( ($(date +%s) - BEGAN) / 60 + 2 ))m" > "$D/tensorfs.log" 2>&1 || true
  cozy machine show "$NAME" --json > "$D/show-after.json" 2>&1
  cozy machine logs local calcifer --lines 20000 > "$D/calcifer.log" 2>&1 || true
  for b in pier walk; do run_page asset "$b" > "$D/asset-$b.json" 2>&1; done
  [ "$SAMPLE" != true ] || T=120 pod "cat /tmp/samples.jsonl" > "$D/samples.jsonl" 2> /dev/null < /dev/null || say "the samples weren't read"
  python3 "$(dirname "$0")/prepare-ahead.py" "$D" "$ASK" "${PICTURE%%/*}" "${VIDEO%%/*}" > "$D/timeline.json" 2> "$D/timeline.failed"
  cat "$D/timeline.json" >> "$R"
  # The next arm runs either way: its numbers are the comparison.
  [ "$rendered" = 0 ] || { say "FAIL: ${ARM:+arm $ARM: }the render didn't land both takes (exit $rendered): $(tail -c 400 "$D/render.txt")"; RC=1; }
  while read -r why; do say "FAIL: ${ARM:+arm $ARM: }$why"; RC=1; done < "$D/timeline.failed"
done
if [ "${#names[@]}" -gt 1 ]; then
  python3 "$(dirname "$0")/prepare-ahead.py" --compare "${names[@]/#/$OUT/arm-}" > "$OUT/compare.json" 2>> "$R"
  cat "$OUT/compare.json" >> "$R"
fi
[ "$RC" = 0 ] && say "PASS: the video's fetch ahead downloaded H3 behind the picture's weights, while the picture rendered, and H3 started on them"
