# gpuless-render (cl-1279; Paul, 11-18: "why are you trying to place jobs on a machine that cannot run them?"): what Render does on
# a computer with no GPU, this runner's own machine, for two of the Hub's tools that each build a model: `birefnet/remove_background`,
# rendered to its end, and `qwen-image-2/image`, watched until its work on the CPU shows, then canceled. A plain project that renders on
# `local` alone, shown by a page; each box rendered with `cozy render`. Then `cozy run` of the Qwen function naming `local`. No rental.
# It judges nothing: where each job was placed and on what quote, what its box and the CLI said, what landed, and what the run did are
# said in results.txt, from the runs' records, the board, `cozy machine show local` and the machine's logs.
# params:
#   watch_s            the most seconds the Qwen render is watched from its ask (default 3000)
#   work_s             it is canceled once its function has worked this long past its loads (default 1200)
#   prompt_upsampling  the Qwen box's prompt enhancer, 11 GB more (default false)
. "$(dirname "$0")/lib.sh"
logs() {
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1
  cozy machine logs local tensord --since 3h -n 0 > "$OUT/tensord-local.log" 2>&1
  cozy machine logs local tensorfs --since 3h -n 0 > "$OUT/tensorfs-local.log" 2>&1
}
fail() { logs; say "FAIL: $*"; RC=1; exit 1; }
WATCH=$(param watch_s 3000) WORK=$(param work_s 1200) UPSAMPLE=$(param prompt_upsampling false)
[[ $WATCH =~ ^[0-9]+$ && $WORK =~ ^[0-9]+$ && $UPSAMPLE =~ ^(true|false)$ ]] || fail "watch_s $WATCH, work_s $WORK, prompt_upsampling $UPSAMPLE"
for tool in jq ffmpeg python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
mkdir -p "$OUT/runs"
nice -n 19 ffmpeg -v error -y -f lavfi -i 'testsrc2=size=1024x1024:rate=1' -frames:v 1 "$OUT/photo.png" || fail "no picture"
WORDS='A red bicycle against a white wall at night.'
in_project() { (cd "$RUN_PROJECT" && cozy "$@"); }
ram() { awk '/MemAvailable/ { printf "%.1f", $2 / 1048576 }' /proc/meminfo; }
room() { df -BM --output=avail "$HOME" | tail -1 | tr -dc 0-9; }
mark() { echo "$(date +%s%3N) $1" >> "$OUT/marks.txt"; }

# This computer's machine, up and saying what it has.
for _ in $(seq 60); do
  cozy --json machine show local > "$OUT/local-before.json" 2> "$OUT/local-before.err"
  [ "$(jq -r '.status.phase // empty' "$OUT/local-before.json" 2> /dev/null)" = ready ] && break
  sleep 5
done
[ "$(jq -r '.status.phase // empty' "$OUT/local-before.json")" = ready ] || fail "local isn't ready: $(tail -c 300 "$OUT/local-before.err")"
[ "$(jq '.status.gpus // [] | length' "$OUT/local-before.json")" = 0 ] || fail "local has a GPU: this session is for a computer with none"
cozy machine show local > "$OUT/local-before.txt" 2>&1
cozy model ls --on local > "$OUT/models-before.txt" 2>&1
say "cozy $(cozy --version | awk '{print $2}') on this runner: $(nproc) CPUs, $(awk '/MemTotal/ { printf "%.1f", $2 / 1048576 }' /proc/meminfo) GiB RAM ($(ram) free), $(room) MB disk free; local: $(jq -c '.status | {gpus: (.gpus | length), serving_gpus, memory_bytes, platform, runtime, torch, times: (.times // [] | length)}' "$OUT/local-before.json")"

run_project "$OUT/project" > "$OUT/project.json" 2>&1 || fail "the project: $(tail -c 300 "$OUT/project.json")"
in_project --json machine use local > "$OUT/use.json" 2>&1 || fail "machine use local: $(tail -c 300 "$OUT/use.json")"
run_page import "$OUT/photo.png" --name photo > "$OUT/photo.json" 2>&1 || fail "import photo: $(tail -c 300 "$OUT/photo.json")"
python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/show.json" 2> "$OUT/show.err" & BG="$BG $!"
for _ in $(seq 100); do grep -q '"held"' "$OUT/show.json" 2> /dev/null && break; sleep 0.1; done
grep -q '"held"' "$OUT/show.json" || fail "no page shows the project: $(tail -c 300 "$OUT/show.err")"
sleep 3

mark added
run_page add cut --tool birefnet/remove_background --fields '{"image": "<photo>"}' > "$OUT/add-cut.json" 2>&1 || fail "the BiRefNet box: $(tail -c 300 "$OUT/add-cut.json")"
run_page add q --tool qwen-image-2/image --fields "$(jq -nc --arg w "$WORDS" --argjson u "$UPSAMPLE" '{instructions: $w, prompt_upsampling: $u}')" > "$OUT/add-q.json" 2>&1 \
  || fail "the Qwen box: $(tail -c 300 "$OUT/add-q.json")"
sleep 45
# What a page showing them had guessed for each, and what it says beside Render before a render here.
logs
for box in cut q; do
  say "$box, shown and not rendered: $(grep -oE "$box (isn't guessed: [^\"]*|guessed on [a-z0-9-]+)|$box's guess [^\"]*" "$OUT/calcifer.log" | sort -u | paste -sd '|' | cut -c1-600)"
done
run_page fit cut q > "$OUT/fit.json" 2> "$OUT/fit.err" || say "fit: $(tail -c 300 "$OUT/fit.err")"
say "fit, as the page asks before a render here: $(jq -c '{tools: [.tools[]? | {tool, names, line, bytes}], estimates: [.estimates[]? | {name, machine, card, run_ms, basis, faster: .faster.line}], rent: .rent}' "$OUT/fit.json" 2> /dev/null | cut -c1-1500)"

# newest <function>: the project's newest run of it, as `cozy run ls` lists it.
newest() { in_project --json run ls --limit 0 2> /dev/null | jq -c --arg f "$1" '[.runs[]? | select((.target // "") | test("/" + $f + "(@|$)"))][0] // empty'; }
# recorded <name> <run>: the run's record kept in runs/ (no ledger row: this judges no package), and said.
recorded() {
  in_project --json --full run watch "$2" --once > "$OUT/runs/$1.json" 2> "$OUT/runs/$1.err"
  in_project run watch "$2" --once > "$OUT/runs/$1.txt" 2>&1
  say "$1: its run: $(jq -c 'def s(a; b): if a != null and b != null then ((b - a) / 100 | round / 10) else null end;
    {number, target, machine, status, cancel_requested, gpu, waiting, reason: .reason.message,
     placed: (.placed | if . then {machine, finish_s: s(.quoted_at_ms; .finish_at_ms), basis, warm, prepare_s: (.prepare_ms / 1000 | round), fetch_bytes} else null end),
     queued_s: s(.created_at_ms; .accepted_at_ms), preparing_s: s(.accepted_at_ms; .started_at_ms), ran_s: s(.started_at_ms; .settled_at_ms),
     execution_ms, progress: (.progress | if . then {stage, completed, total, bytes_done, bytes_total} else null end),
     warnings: [.warnings[]?.message], output: [.output[]? | (.path // .sha256 // .)]}' "$OUT/runs/$1.json" 2> /dev/null | cut -c1-1800)"
}
# rendered <box> <function> <limit_s> <work_s>: `cozy render <box>` followed: each change of where its run stands said with this
# runner's free RAM and disk. It ends itself, or is canceled once <limit_s> are up, once its function has worked <work_s> past its
# loads (0: never), or when this runner's free RAM is nearly gone. RUN: its run's number.
WORKING='denois|generat|encoding|decoding|upsampl|refin|removing|reading'
rendered() {
  local box=$1 fn=$2 limit=$3 work=$4 began=$SECONDS p rec='' job coarse last='' fine steps told=0 worked=0 low=0 stage why='' code t
  RUN=''
  say "cozy render $box ($fn), the project's machines: $(jq -c '.machines // .' "$OUT/use.json" 2> /dev/null | cut -c1-200)"
  mark "$box"
  (cd "$RUN_PROJECT" && exec cozy render "$box") > "$OUT/render-$box.txt" 2>&1 & p=$!
  while kill -0 "$p" 2> /dev/null; do
    sleep 5; t=$((SECONDS - began))
    [ -n "$RUN" ] || RUN=$(newest "$fn" | jq -r '.number // empty')
    [ -z "$RUN" ] || rec=$(in_project --json --full run watch "$RUN" --once 2> /dev/null)
    job=$(run_page board 2> /dev/null | jq -c --arg b "$box" '[.jobs[]? | select(.asset == $b)][-1] // {} | {state, machine, why, stage}' 2> /dev/null)
    stage=$(jq -r '.progress.stage // empty' <<< "$rec" 2> /dev/null)
    steps=$(jq -r '.progress | if . and (.unit // "") == "" and (.total // 0) > 0 then "\(.completed)/\(.total)" else "-" end' <<< "${rec:-{\}}" 2> /dev/null)
    coarse="run ${RUN:-none} $(jq -r '"\(.status // "unsent") on \(.machine // "-"), waiting: \(.waiting // "-")"' <<< "${rec:-{\}}" 2> /dev/null); stage: ${stage:--}, step $steps; its job: $job"
    fine=$(jq -r '.progress | if . then "\(.completed)/\(.total) \(.unit // ""), \(.bytes_done)/\(.bytes_total) bytes" else "-" end' <<< "${rec:-{\}}" 2> /dev/null)
    if [ "$coarse" != "$last" ] || [ $((t - told)) -ge 60 ]; then
      say "$box +${t}s: $coarse; $fine; RAM free $(ram) GiB, disk free $(room) MB"; last=$coarse told=$t
    fi
    if [ "$work" != 0 ] && [[ $stage =~ $WORKING ]] && [[ $stage != *loading* ]]; then
      [ "$worked" != 0 ] || worked=$SECONDS
      [ $((SECONDS - worked)) -lt "$work" ] || { why="its function worked $work s on this computer's CPU"; break; }
    fi
    [ "$t" -lt "$limit" ] || { why="$limit s were up"; break; }
    if awk -v m="$(ram)" 'BEGIN { exit !(m < 0.6) }'; then low=$((low + 1)); else low=0; fi
    [ "$low" -lt 2 ] || { why="this runner's free RAM fell to $(ram) GiB"; break; }
  done
  if kill -0 "$p" 2> /dev/null; then
    say "$box: canceled after $((SECONDS - began)) s: $why"
    mark "$box-canceled"
    if [ -n "$RUN" ]; then in_project run cancel "$RUN" > "$OUT/cancel-$box.txt" 2>&1; else run_page cancel "$box" > "$OUT/cancel-$box.txt" 2>&1; fi
    say "$box: cancel said: $(tr '\n' '|' < "$OUT/cancel-$box.txt" | cut -c1-300)"
    for _ in $(seq 24); do kill -0 "$p" 2> /dev/null || break; sleep 5; done
    kill "$p" 2> /dev/null
  fi
  wait "$p"; code=$?
  mark "$box-ended"
  say "$box: cozy render exited $code after $((SECONDS - began)) s and said: $(tr '\n' '|' < "$OUT/render-$box.txt" | cut -c1-1500)"
  [ -z "$RUN" ] || recorded "$box" "$RUN"
  run_page board > "$OUT/board-$box.json" 2> /dev/null; run_page asset "$box" > "$OUT/asset-$box.json" 2> /dev/null
  say "$box: the board said: $(jq -c '[.said[]? | {level, message}]' "$OUT/board-$box.json" 2> /dev/null | cut -c1-1500)"
  say "$box: its job: $(jq -c --arg b "$box" '[.jobs[]? | select(.asset == $b)][-1] // {} | {state, machine, why, stage, detail, error: .error.message}' "$OUT/board-$box.json" 2> /dev/null | cut -c1-600)"
  say "$box: its box: $(jq -c '{standing: .asset.standing, failure: .asset.failure, versions: (.versions | length), warnings: [.versions[-1].warnings[]?]}' "$OUT/asset-$box.json" 2> /dev/null | cut -c1-600)"
  in_project status > "$OUT/status-$box.txt" 2>&1
}

rendered cut remove_background 2400 0
rendered q image "$WATCH" "$WORK"
QRUN=$RUN
# What the canceled render still brings: this runner's disk a moment after, and 45 s on.
r0=$(room); sleep 45; say "q: after its cancel, disk free $r0 MB, then $(room) MB 45 s on"

# A run naming this computer, of the same function: what the CLI says of it, then canceled.
say "cozy run qwen-image-2/image instructions=… prompt_upsampling=$UPSAMPLE --on local"
mark named
(cd "$RUN_PROJECT" && exec cozy run qwen-image-2/image "instructions=$WORDS" "prompt_upsampling=$UPSAMPLE" --on local) > "$OUT/named.txt" 2>&1 & np=$!
sleep 75
NRUN=$(newest image | jq -r '.number // empty')
if [ -n "$NRUN" ] && [ "$NRUN" != "$QRUN" ]; then
  recorded named "$NRUN"
  in_project run cancel "$NRUN" > "$OUT/cancel-named.txt" 2>&1
fi
for _ in $(seq 12); do kill -0 "$np" 2> /dev/null || break; sleep 5; done
kill "$np" 2> /dev/null; wait "$np"
mark named-ended
say "named: cozy run said in its first 75 s: $(tr '\n' '|' < "$OUT/named.txt" | cut -c1-1500)"

cozy --json machine show local > "$OUT/local-after.json" 2>&1
cozy machine show local > "$OUT/local-after.txt" 2>&1
cozy model ls --on local > "$OUT/models-after.txt" 2>&1
in_project --json run ls --limit 0 > "$OUT/runs/ls.json" 2>&1
logs
say "runs: $(jq -c '[.runs[]? | {number, kind, target, machine, status, reason: .reason.message}]' "$OUT/runs/ls.json" 2> /dev/null | cut -c1-1500)"
say "cozy model ls --on local, after:"; sed 's/^/  /' "$OUT/models-after.txt" | tee -a "$R"
say "local after: $(grep -E '^disk (free|use)' "$OUT/local-after.txt" | tr -s ' ' | paste -sd ';'); this runner: RAM free $(ram) GiB, disk free $(room) MB"
say "what local timed: $(jq -c '[.status.times[]? | {function, runs}]' "$OUT/local-after.json" 2> /dev/null | cut -c1-1200)"

# By the marks: what local landed in each span (TensorFS's lines in TensorD's log), and what Calcifer said of this project's placement.
python3 -I - "$OUT/calcifer.log" "$OUT/tensord-local.log" "$OUT/marks.txt" "$RUN_PROJECT" <<'PY' | tee "$OUT/timeline.txt" | tee -a "$R"
import datetime, re, sys
calcifer, tensord, marks, folder = sys.argv[1:5]
def ms(stamp):
    try:
        return int(datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp() * 1000)
    except ValueError:
        return 0
marked = [(int(at), what) for at, what in (line.split(" ", 1) for line in open(marks).read().split("\n") if line)]
added = marked[0][0]
def lines(path, keep):
    for line in open(path, errors="replace"):
        stamp = line.split(" ", 1)[0]
        if stamp[:4].isdigit() and ms(stamp) >= added - 5000 and keep(line):
            yield ms(stamp), re.sub(r"\x1b\[[0-9;]*m", "", line.split("  INFO ", 1)[-1].split("  WARN ", 1)[-1]).strip()
landed = sorted((at, int(m.group(2))) for at, line in lines(tensord, lambda l: "fetch::download: landed" in l)
                for m in [re.search(r"object=(\w+) bytes=(\d+)", line)] if m)
for (at, what), (until, _) in zip(marked, marked[1:] + [(1 << 62, "")]):
    span = [size for when, size in landed if at <= when < until]
    print(f"  from {what} (+{(at - added) / 1000:.0f} s): local landed {len(span)} objects, {sum(span)} bytes")
said = list(lines(calcifer, lambda l: folder in l and any(word in l for word in (" goes to ", "isn't planned", "no machine planned", "waits", "guess"))))
for at, text in said[:60]:
    print(f"  +{(at - added) / 1000:>7.1f} s  {text[:600]}")
PY
say "DONE: nothing judged"
