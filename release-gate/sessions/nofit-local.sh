# nofit-local (cl-1123): a Qwen Image 2.1 box (the Hub package) put on a project's canvas on this runner's own machine, which has no
# GPU, as the walkthrough did (44299692970: `local` took about 20 GiB nobody asked for). A page shows the project, which renders on
# `local` alone (a computer with no rental). The box is added with no prompt, then a prompt mentioning a picture is typed, taken back
# and typed again (the walkthrough's undo and redo). Evidence is the machine's own: `cozy model ls --on local`, its disk, TensorD's list
# items and what landed, and Calcifer's prediction lines, each with its time after the box was added. No rental.
# Passes (want=nothing): local lists no item for the box, holds no model it didn't hold before, and landed no weights; Calcifer says
# why the box isn't guessed. want=any judges nothing (the RC's build, before the fix).
# params:
#   want    nothing (default) or any
#   wait_s  seconds read after the last edit (default 120)
. "$(dirname "$0")/lib.sh"
logs() {
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1
  cozy machine logs local tensord --since 2h -n 0 > "$OUT/tensord-local.log" 2>&1
  cozy machine logs local tensorfs --since 2h -n 0 > "$OUT/tensorfs-local.log" 2>&1
}
fail() { logs; say "FAIL: $*"; RC=1; exit 1; }
WANT=$(param want nothing) WAIT=$(param wait_s 120) TOOL=qwen-image-2/image
[[ $WANT =~ ^(nothing|any)$ && $WAIT =~ ^[0-9]+$ ]] || fail "want $WANT, wait_s $WAIT"
for tool in jq ffmpeg python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
nice -n 19 ffmpeg -v error -y -f lavfi -i 'testsrc2=size=1024x1024:rate=1' -frames:v 1 "$OUT/photo.png" || fail "no picture"

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
say "cozy $(cozy --version | awk '{print $2}') on this runner; local: no GPU; $(grep -E '^disk (free|use)' "$OUT/local-before.txt" | tr -s ' ' | paste -sd ';')"

run_project "$OUT/project" > "$OUT/project.json" 2>&1 || fail "the project: $(tail -c 300 "$OUT/project.json")"
(cd "$RUN_PROJECT" && cozy --json machine use local) > "$OUT/use.json" 2>&1 || fail "machine use local: $(tail -c 300 "$OUT/use.json")"
run_page import "$OUT/photo.png" --name photo > "$OUT/photo.json" 2>&1 || fail "import photo: $(tail -c 300 "$OUT/photo.json")"
python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/show.json" 2> "$OUT/show.err" & BG="$BG $!"
for _ in $(seq 100); do grep -q '"held"' "$OUT/show.json" 2> /dev/null && break; sleep 0.1; done
grep -q '"held"' "$OUT/show.json" || fail "no page shows the project: $(tail -c 300 "$OUT/show.err")"
sleep 3

mark() { echo "$(date +%s%3N) $1" >> "$OUT/marks.txt"; }
words() { run_page set q --fields "$(jq -nc --arg w "$1" '{instructions: $w}')" > "$OUT/set-$2.json" 2>&1 || fail "$2: $(tail -c 300 "$OUT/set-$2.json")"; }
PICTURED='The red bicycle of <photo>, against a white wall at night.'
mark added; run_page add q --tool "$TOOL" --fields '{}' > "$OUT/add.json" 2>&1 || fail "the Qwen box: $(tail -c 300 "$OUT/add.json")"
sleep 5
mark typed; words "$PICTURED" typed
sleep 10
mark undone; words '' undone
sleep 10
mark redone; words "$PICTURED" redone
sleep "$WAIT"
mark read

run_page fit q > "$OUT/fit.json" 2>&1 || say "fit: $(tail -c 300 "$OUT/fit.json")"
(cd "$RUN_PROJECT" && cozy --json status) > "$OUT/status.json" 2>&1 || say "status: $(tail -c 300 "$OUT/status.json")"
cozy --json machine show local > "$OUT/local-after.json" 2>&1
cozy machine show local > "$OUT/local-after.txt" 2>&1
cozy model ls --on local > "$OUT/models-after.txt" 2>&1
logs
say "the box beside Render: $(jq -c '[.estimates[]? | select(.name == "q") | {machine, faster: .faster.line}]' "$OUT/fit.json" 2> /dev/null); its board rows ahead: $(jq -c '[.board.ahead[]? | select((.names // []) | index("q"))]' "$OUT/status.json" 2> /dev/null)"
say "local after: $(grep -E '^disk (free|use)' "$OUT/local-after.txt" | tr -s ' ' | paste -sd ';')"
say "cozy model ls --on local, before:"; sed 's/^/  /' "$OUT/models-before.txt" | tee -a "$R"
say "cozy model ls --on local, after:"; sed 's/^/  /' "$OUT/models-after.txt" | tee -a "$R"

# What happened, by time after the box was added: Calcifer's prediction lines of this project and its lists to local; TensorD's list
# items; and the weights local landed (TensorFS's lines in TensorD's log), summed.
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
said = sorted([(at, "mark: " + what) for at, what in marked]
              + list(lines(calcifer, lambda l: folder in l or "guess q on " in l or "lists put on local" in l or "lists to local" in l))
              + [(at, "tensord: " + text) for at, text in lines(tensord, lambda l: "cozy:prepare" in l)])
for at, text in said[:120]:
    print(f"  +{at - added:>7} ms  {text[:420]}")
landed = {m.group(1): int(m.group(2)) for at, line in lines(tensord, lambda l: "fetch::download: landed" in l)
          for m in [re.search(r"object=(\w+) bytes=(\d+)", line)] if m}
items = sum(1 for _ in lines(tensord, lambda l: "cozy:prepare" in l))
print(f"  local landed {len(landed)} objects, {sum(landed.values())} bytes, from the box's adding on; TensorD said {items} lines of list items")
PY

[ "$WANT" = nothing ] || { say "DONE: want=any, nothing judged"; exit 0; }
new=$(comm -13 <(awk 'NR > 1 {print $2}' "$OUT/models-before.txt" | sort) <(awk 'NR > 1 && $1 == "local" {print $2}' "$OUT/models-after.txt" | sort) | paste -sd ' ')
[ -z "$new" ] || fail "local holds models it didn't before: $new"
grep -q "local landed 0 objects" "$OUT/timeline.txt" || fail "local landed weights: $(grep 'local landed' "$OUT/timeline.txt")"
grep -q "TensorD said 0 lines of list items" "$OUT/timeline.txt" || fail "local was told of a list item: $(grep 'TensorD said' "$OUT/timeline.txt")"
grep -q "q isn't guessed: this computer has no GPU" "$OUT/calcifer.log" || fail "Calcifer didn't say why q isn't guessed"
say "PASS: local was told of nothing, landed nothing and holds no new model; Calcifer: $(grep -o "q isn't guessed: [^\"]*" "$OUT/calcifer.log" | head -1)"
