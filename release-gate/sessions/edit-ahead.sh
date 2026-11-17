# edit-ahead (cl-1123; Paul, 11-17: "I just attached qwen image 2.1, to an image, and typed up a prompt, but it didn't pre-fetch the
# iTi prompt-enhancer until I pushed render"): a picture box, approved as an import is, with Qwen Image 2.1 put on it and an edit
# typed, is a draft its machine gets ready for; so is a text-to-image box. Qwen Image 2 from a packages checkout (a dev home, as Paul's
# laptop is), every step through this runner's cozy and its Calcifer, on the session's TensorD; evidence is the machine's own log
# and records.
# 1. A page shows the project. `photo`, an imported picture, gets Qwen Image 2.1, then an edit typed (`<self.1> …`, as the page
#    writes it): its machine plans the item with expander_i2i needed within 3 s of each, and lands it whole; expander_t2i never comes.
# 2. photo rendered: its run is the request the item predicted, its quote downloads nothing and it waits for nothing.
# 3. `draw`, a text-to-image box, added: planned with expander_t2i needed (expander_i2i not) within 3 s; expander_t2i lands whole;
#    rendered, it is adopted and waits for nothing.
# params:
#   tree    the packages_refs checkout whose qwen-image-2 runs (`-f packages_refs=pk=<packages ref with its rules>`, tree pk)
#   need_s  the seconds the cap must buy (default 2400)
#   ahead_min how long a box may take to be ready ahead (default 30; a red run's 3 ends sooner)
. "$(dirname "$0")/lib.sh"
fail() {
  [ -z "${NAME:-}" ] || cozy machine logs "$NAME" --since 2h -n 0 > "$OUT/tensord-$NAME.log" 2>&1
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1
  say "FAIL: $*"; RC=1; exit 1
}
TREE=$(param tree '') AHEAD_S=$(($(param ahead_min 30) * 60))
T2I=pottokao/qwen-image-2.1-pe-t2i-abliterated I2I=darrellbest/qwen-image-2.1-pe-i2i-abliterated
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen-image-2/pyproject.toml" ] || fail "dispatch with -f packages_refs=<name>=<packages ref> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V: the change runs from a dev build (pod-session cozy=<dev-build run id>)"
for tool in jq curl ffmpeg python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
# A dev home, its first-party packages from the checkout; Calcifer started again on it.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
kernels "$SOURCE/qwen-image-2"
nice -n 19 ffmpeg -v error -y -f lavfi -i 'testsrc2=size=1024x1024:rate=1' -frames:v 1 "$OUT/photo.png" || fail "no picture"
run_page import "$OUT/photo.png" --name photo > "$OUT/photo.json" 2>&1 || fail "import photo: $(tail -c 300 "$OUT/photo.json")"
say "cozy $V, a dev home; qwen-image-2 $(sed -n 's/^version = "\(.*\)"/\1/p' "$SOURCE/qwen-image-2/pyproject.toml" | head -1) from $TREE $(git -C "$SOURCE" rev-parse --short=8 HEAD 2> /dev/null)"
quote "$(param need_s 2400)"

shown() { cozy --json machine show "$NAME" > "$1" 2> /dev/null || true; }
held() { jq -r --arg m "$2" '[.status.models[]? | select(.repository == $m) | .unique_bytes // 0] | add // 0' "$1" 2> /dev/null || echo 0; }
whole() { jq -e --arg m "$2" '[.status.models[]? | select(.repository == $m) | .checkpoints[]?.complete] | any' "$1" > /dev/null 2>&1; }
models() { jq -c '[.status.models[]? | {repository, total_bytes, unique_bytes, complete: [.checkpoints[]?.complete]}]' "$1" 2> /dev/null; }
HEADER=$((16 << 20))
record() {
  local n
  n=$(run_page asset "$1" | jq -r '.versions[-1].runs[-1] // empty')
  [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$2" 2>&1
}
# ours: $NAME on the open RC's builds once, then on the session's TensorD (`V`), which carries the change (needs-ahead.sh).
version() { cozy --json machine show "$NAME" 2> /dev/null | jq -r '.status.version // empty'; }
ours() {
  local tries=0 runs
  if [[ " ${RC_DONE:-} " != *" $NAME "* ]]; then
    RC_DONE="${RC_DONE:-} $NAME"
    step rc machine update "$NAME" -y || fail "$NAME didn't move to the open RC's builds"
  fi
  while runs=$(version); [ "$runs" != "$V" ]; do
    [ "$tries" -lt 3 ] || fail "$NAME doesn't keep the session's TensorD $V: it runs ${runs:-nothing it says}"
    step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "$NAME refused the session's TensorD"
    sleep 30; tries=$((tries + 1))
  done
  say "$NAME runs the session's TensorD $V"
}
# page: a page shows this project, as the page's own fetch asks it (cl-985): its boxes are guessed while it does.
page() {
  local addr id stream='' headers
  addr=$(jq -r .address "$HOME/.cozy/calcifer/calcifer.lock.holder")
  headers=(-H "Host: $addr" -H 'Sec-Fetch-Site: same-origin' -H 'Sec-Fetch-Mode: cors' -H 'Sec-Fetch-Dest: empty' -H 'cozy-project-storage: 3')
  curl -sSN "${headers[@]}" -H 'Accept: text/event-stream' "http://$addr/v1/events" > "$OUT/page.sse" 2>&1 & BG="$BG $!"
  for _ in $(seq 100); do
    stream=$(sed -n 's/^data: //p' "$OUT/page.sse" | jq -r 'select(.type == "hello") | .stream' 2> /dev/null | head -1); [ -n "$stream" ] && break; sleep 0.1
  done
  [ -n "$stream" ] || fail "the page's stream said no hello: $(head -c 300 "$OUT/page.sse")"
  id=$(python3 -c 'import sys; sys.path.insert(0, sys.argv[1]); import page; print(page.read(sys.argv[2])["id"])' "$(dirname "$RUN_PAGE")" "$RUN_PROJECT" 2> "$OUT/id.err")
  [ -n "$id" ] || fail "the project's id: $(tail -c 300 "$OUT/id.err")"
  curl -fsS -X PUT "${headers[@]}" -H "Origin: http://$addr" -H 'Content-Type: application/json' -d "$(jq -nc --arg id "$id" '{projects: [$id]}')" \
    "http://$addr/v1/events/$stream" > "$OUT/show.json" 2>&1 || fail "the page couldn't show the project: $(tail -c 300 "$OUT/show.json")"
}
# planned <since ms> <rule>: from TensorD's log, when the first prepare item after <since> whose rules hold for <rule> was planned,
# the rules it logged, and when the machine first fetched at a prepare item's order after it (Unix ms each; the pod's clock).
planned() {
  python3 -I - "$OUT/tensord-$NAME.log" "$1" "$2" <<'PY'
import datetime, re, sys
log, since, rule = sys.argv[1], int(sys.argv[2]), sys.argv[3]
def ms(stamp):
    return int(datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp() * 1000)
planned = fetched = 0
rules = "-"
for line in open(log, errors="replace"):
    parts = line.split(" ", 1)
    if not parts[0][:4].isdigit():
        continue
    at = ms(parts[0])
    if at < since:
        continue
    if not planned and "cozy:prepare:" in line and "its rules hold for" in line and f'"{rule}"' in line.split("its rules hold for", 1)[1].split("}", 1)[0]:
        planned, rules = at, "hold for " + line.split("its rules hold for ", 1)[1].split("; it needs", 1)[0].strip()
    elif planned and not fetched:
        found = re.search(r"fetching object=\S+ bytes=\d+ priority=(\d+)", line)
        if found and int(found.group(1)) >= 1 << 63:
            fetched = at
print(planned, fetched, rules.replace(" ", "_"))
PY
}
row() { run_page board | jq -c --arg n "$1" '[.ahead[]? | select((.names // []) | index($n))] | max_by(.run) // empty' 2> /dev/null; }
# ready_ahead <box> <repository> <file>: <box>'s guess is ready on $NAME and holds <repository> whole (`ahead_min` at most).
ready_ahead() {
  local began=$SECONDS got
  while [ $((SECONDS - began)) -lt "$AHEAD_S" ]; do
    shown "$3"; got=$(row "$1")
    whole "$3" "$2" && jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${got:-null}" > /dev/null 2>&1 && { say "$1 ready on $NAME: $got"; return 0; }
    sleep 2
  done
  fail "$1 isn't ready on $NAME with $2 whole after $((AHEAD_S / 60)) min: $(models "$3"); row: ${got:-none}"
}
MISMATCH=()
# judged <box> <since ms> <rule> <what>: <box>'s item planned with <rule> needed within 3 s of <what> at <since>.
judged() {
  cozy machine logs "$NAME" --since 2h -n 0 > "$OUT/tensord-$NAME.log" 2>&1 || true
  local at fetched rules
  read -r at fetched rules <<< "$(planned "$2" "$3")"
  say "$1, $4 at $2 (Unix ms): planned on $NAME $((at - $2)) ms later ($rules); first fetched ahead $((fetched - $2)) ms after"
  [ "$at" -gt 0 ] && [ $((at - $2)) -le 3000 ] || MISMATCH+=("$1's item with $3 needed was planned $((at - $2)) ms after $4")
}
# rendered <box>: rendered on $NAME, it adopts its item and its quote and its wait are 0.
rendered() {
  local before after adopted
  shown "$OUT/before-$1.json"; before=$(models "$OUT/before-$1.json")
  timeout 3000 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" "$1" > "$OUT/$1.txt" 2>&1 || fail "$1: $(tail -c 400 "$OUT/$1.txt")"
  record "$1" "$OUT/$1-run.json" || fail "$1 has no run record: $(tail -c 300 "$OUT/$1.txt")"
  shown "$OUT/after-$1.json"; after=$(models "$OUT/after-$1.json")
  say "$1: placed $(jq -c .placed "$OUT/$1-run.json"); weights $(jq -c .measurements.weights "$OUT/$1-run.json")"
  jq -e '.placed.fetch_bytes == 0' "$OUT/$1-run.json" > /dev/null || MISMATCH+=("$1's quote counts $(jq -r .placed.fetch_bytes "$OUT/$1-run.json") bytes to download")
  jq -e '.measurements.weights.needs == 0' "$OUT/$1-run.json" > /dev/null || MISMATCH+=("$1 waited for $(jq -r .measurements.weights.needs "$OUT/$1-run.json") bytes")
  [ "$before" = "$after" ] || MISMATCH+=("$NAME's models moved during $1's render: $before -> $after")
  cozy machine logs "$NAME" --since 2h -n 0 > "$OUT/tensord-$NAME.log" 2>&1 || true
  adopted=$(grep -E "run run-[0-9a-f]+ (is the request|names)" "$OUT/tensord-$NAME.log" | tail -1 | sed 's/.*INFO tensord::ahead: //')
  say "$1's run on $NAME: ${adopted:-named no predicted request}"
  [[ $adopted == *"is the request"*"takes what it made ready"* && $adopted != "${ADOPTED:-}" ]] || MISMATCH+=("$1's render didn't adopt its item: ${adopted:-it named none}")
  ADOPTED=$adopted
}

rent
ours
(cd "$RUN_PROJECT" && cozy --json machine use "$NAME") > "$OUT/use.json" 2>&1 || fail "machine use $NAME: $(tail -c 300 "$OUT/use.json")"
say "step 1: a page shows the project; photo, an imported picture, gets Qwen Image 2.1, then an edit"
page
sleep 2
ATTACH_MS=$(date +%s%3N)
run_page set photo --tool qwen-image-2/image --fields '{}' > "$OUT/attach.json" 2>&1 || fail "Qwen on photo: $(tail -c 300 "$OUT/attach.json")"
WRITTEN=$(run_page asset photo | jq -r '.asset.fields.instructions // empty')
[[ $WRITTEN == "<self."* ]] || fail "photo's instructions don't start from its picture: '$WRITTEN'"
sleep 2
EDIT_MS=$(date +%s%3N)
run_page set photo --fields "$(jq -nc --arg w "$WRITTEN The same scene at night, under a full moon." '{instructions: $w, megapixels: 1}')" > "$OUT/edit.json" 2>&1 ||
  fail "photo's edit: $(tail -c 300 "$OUT/edit.json")"
ready_ahead photo "$I2I" "$OUT/photo-ready.json"
[ "$(held "$OUT/photo-ready.json" "$T2I")" -le "$HEADER" ] || MISMATCH+=("expander_t2i came for photo's edit")
judged photo "$ATTACH_MS" expander_i2i "Qwen put on it"
judged photo "$EDIT_MS" expander_i2i "its edit typed"
say "step 2: photo rendered on $NAME"
rendered photo
say "step 3: draw, a text-to-image box, added"
ADD_MS=$(date +%s%3N)
run_page add draw --tool qwen-image-2/image --fields '{"instructions": "A red bicycle against a white wall.", "megapixels": 1}' > "$OUT/draw-add.json" 2>&1 ||
  fail "draw: $(tail -c 300 "$OUT/draw-add.json")"
ready_ahead draw "$T2I" "$OUT/draw-ready.json"
judged draw "$ADD_MS" expander_t2i "added"
grep -E "cozy:prepare:.*its rules hold for [^;]*expander_t2i" "$OUT/tensord-$NAME.log" | grep -q 'not for {[^}]*"expander_i2i"' || MISMATCH+=("draw's item didn't rule out expander_i2i")
rendered draw
cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
say "Calcifer on why a box isn't guessed: $(grep -o "[^/ ]* isn't guessed: .*" "$OUT/calcifer.log" | sort | uniq -c | sed 's/^ *//' | paste -sd ';' -)"
say "Calcifer's guesses: $(grep -o 'guess [a-z_]* on [^:]*: [^;]*' "$OUT/calcifer.log" | sort -u | paste -sd ';' -)"
[ ${#MISMATCH[@]} = 0 ] || fail "what didn't match: ${MISMATCH[*]}"
say "PASS: a picture box with Qwen put on it and an edit typed, and a text-to-image box, were each made ready ahead with their own enhancer and adopted by their renders"
