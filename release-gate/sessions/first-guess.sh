# first-guess (cl-1123): how long a box's first guess takes once Qwen Image 2.1 goes on a picture, stage by stage, on this runner's own
# machine (`local`, its CPU): no rental. Two fresh projects, the second on a Calcifer that has read the package for the first; in each,
# an edit is typed while the first guess still reads (44199938129: a typed edit waited behind a 2.75 s first guess). Evidence is
# Calcifer's prediction lines and TensorD's plans (`cozy machine logs local …`). It judges nothing; its numbers go in results.txt.
# With `kind` orbit, an Orbit box of the picture is added instead, cold, as 44195761797's (its first guess took 83 s there). On a pod,
# a third project then renders a weightless box (e2e/cli/project/boxed) reading a new picture, shown by no page so no guess carried
# anything: the render lands its inputs itself, in one request (`PUT /v1/objects`, TensorD's "landed in one request").
# params:
#   tree  the packages_refs checkout whose packages run (`-f packages_refs=pk=<packages ref>`, tree pk)
#   kind  qwen (default) or orbit
#   pod   a SKU to rent one pod of and guess on, in place of this runner's machine (an Orbit box needs a GPU to be guessed for, and a
#         pod is where its code is carried over the network); the session's TensorD runs there (cozy=<dev-build run id>)
. "$(dirname "$0")/lib.sh"
fail() {
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1
  cozy machine logs "${ON:-local}" tensord --since 2h -n 0 > "$OUT/tensord-${ON:-local}.log" 2>&1
  say "FAIL: $*"; RC=1; exit 1
}
TREE=$(param tree '') KIND=$(param kind qwen) POD=$(param pod '') ON=local
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen-image-2/pyproject.toml" ] || fail "dispatch with -f packages_refs=<name>=<packages ref> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
V=$(cozy --version | awk '{print $2}')
for tool in jq curl ffmpeg python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
case $KIND in
  qwen) kernels "$SOURCE/qwen-image-2"; BOX=photo ;;
  orbit)
    kernels "$SOURCE/minimax-h3"; BOX=turned
    # What a guess of an Orbit box carries to its machine: orbit and the checkout's packages it calls, the Triton sets built in them too.
    for p in orbit minimax-h3; do
      say "$p as carried: $(du -sb --exclude=.venv --exclude=.git "$SOURCE/$p" | cut -f1) bytes in $(find "$SOURCE/$p" -path '*/.venv' -prune -o -type f -print | wc -l) files; its Triton sets $(du -sb "$SOURCE/$p"/*/_triton 2> /dev/null | cut -f1 | paste -sd+ | bc 2> /dev/null || echo 0) bytes"
    done ;;
  *) fail "kind $KIND" ;;
esac
nice -n 19 ffmpeg -v error -y -f lavfi -i 'testsrc2=size=1024x1024:rate=1' -frames:v 1 "$OUT/photo.png" || fail "no picture"
say "cozy $V on this runner's own machine; qwen-image-2 from $TREE $(git -C "$SOURCE" rev-parse --short=8 HEAD 2> /dev/null)"

# show: a page shows this project alone, as the page's own fetch asks it (cl-985), on one stream for the session.
STREAM=''
show() {
  local addr id headers
  addr=$(jq -r .address "$HOME/.cozy/calcifer/calcifer.lock.holder")
  headers=(-H "Host: $addr" -H 'Sec-Fetch-Site: same-origin' -H 'Sec-Fetch-Mode: cors' -H 'Sec-Fetch-Dest: empty' -H 'cozy-project-storage: 3')
  if [ -z "$STREAM" ]; then
    curl -sSN "${headers[@]}" -H 'Accept: text/event-stream' "http://$addr/v1/events" > "$OUT/page.sse" 2>&1 & BG="$BG $!"
    for _ in $(seq 100); do
      STREAM=$(sed -n 's/^data: //p' "$OUT/page.sse" | jq -r 'select(.type == "hello") | .stream' 2> /dev/null | head -1); [ -n "$STREAM" ] && break; sleep 0.1
    done
    [ -n "$STREAM" ] || fail "the page's stream said no hello: $(head -c 300 "$OUT/page.sse")"
  fi
  id=$(python3 -c 'import sys; sys.path.insert(0, sys.argv[1]); import page; print(page.read(sys.argv[2])["id"])' "$(dirname "$RUN_PAGE")" "$RUN_PROJECT" 2> "$OUT/id.err")
  [ -n "$id" ] || fail "the project's id: $(tail -c 300 "$OUT/id.err")"
  curl -fsS -X PUT "${headers[@]}" -H "Origin: http://$addr" -H 'Content-Type: application/json' -d "$(jq -nc --arg id "$id" '{projects: [$id]}')" \
    "http://$addr/v1/events/$STREAM" > "$OUT/show.json" 2>&1 || fail "the page couldn't show the project: $(tail -c 300 "$OUT/show.json")"
}
# timeline <label> <asked ms> <answered ms> <edit ms>: from Calcifer's and TensorD's logs, what happened to this project's photo from
# the tool's going on: each guess of it and its stages, each round, each list put to local, and each item TensorD admitted and planned.
timeline() {
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
  cozy machine logs "$ON" tensord --since 2h -n 0 > "$OUT/tensord-$ON.log" 2>&1 || true
  python3 -I - "$OUT/calcifer.log" "$OUT/tensord-$ON.log" "$RUN_PROJECT" "$2" "$3" "$4" "$BOX" "$ON" <<'PY' | tee "$OUT/timeline-$1.txt"
import datetime, sys
calcifer, tensord, folder, asked, answered, edit = sys.argv[1], sys.argv[2], sys.argv[3], *map(int, sys.argv[4:7])
def ms(stamp):
    return int(datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp() * 1000)
def lines(path, keep):
    for line in open(path, errors="replace"):
        stamp = line.split(" ", 1)[0]
        if stamp[:4].isdigit() and ms(stamp) >= asked and keep(line):
            yield ms(stamp), line.split("  INFO ", 1)[-1].strip()
box, on = sys.argv[7], sys.argv[8]
said = sorted(list(lines(calcifer, lambda l: folder in l or f"lists to {on}" in l or f"lists put on {on}" in l or f"guess {box} " in l))
              + [(at, "tensord: " + text) for at, text in lines(tensord, lambda l: "cozy:prepare" in l and ("admitted" in l or "its request" in l))])
print(f"asked {asked}, answered +{answered - asked} ms, edit typed +{edit - asked} ms (Unix ms, this runner's clock)")
for at, text in said[:40]:
    print(f"+{at - asked:>6} ms (edit {at - edit:+} ms)  {text[:400]}")
PY
}
# measure <label>: a fresh project (RUN_PROJECT) on local, shown, with photo; Qwen goes on it, then an edit 500 ms after its answer.
measure() {
  run_project "$RUN_PROJECT" > "$OUT/project-$1.json" 2>&1 || fail "project $1: $(tail -c 300 "$OUT/project-$1.json")"
  (cd "$RUN_PROJECT" && cozy --json machine use local) > "$OUT/use-$1.json" 2>&1 || fail "machine use local: $(tail -c 300 "$OUT/use-$1.json")"
  run_page import "$OUT/photo.png" --name photo > "$OUT/photo-$1.json" 2>&1 || fail "import photo: $(tail -c 300 "$OUT/photo-$1.json")"
  show
  sleep 3
  local asked answered edit written
  asked=$(date +%s%3N)
  run_page set photo --tool qwen-image-2/image --fields '{}' > "$OUT/attach-$1.json" 2>&1 || fail "Qwen on photo: $(tail -c 300 "$OUT/attach-$1.json")"
  answered=$(date +%s%3N)
  written=$(run_page asset photo | jq -r '.asset.fields.instructions // empty')
  sleep 0.5
  edit=$(date +%s%3N)
  run_page set photo --fields "$(jq -nc --arg w "$written The same scene at night, under a full moon." '{instructions: $w, megapixels: 1}')" > "$OUT/edit-$1.json" 2>&1 ||
    fail "photo's edit: $(tail -c 300 "$OUT/edit-$1.json")"
  sleep 30
  timeline "$1" "$asked" "$answered" "$edit" > /dev/null
  say "project $1: $(head -1 "$OUT/timeline-$1.txt")"
  tail -n +2 "$OUT/timeline-$1.txt" | sed 's/^/  /' | tee -a "$R"
}

# turned <label>: a fresh project (RUN_PROJECT) on local, shown, with photo; an Orbit box of it is added, as 44195761797's was, and
# its first guess waited for (4 min at most).
turned() {
  run_project "$RUN_PROJECT" > "$OUT/project-$1.json" 2>&1 || fail "project $1: $(tail -c 300 "$OUT/project-$1.json")"
  (cd "$RUN_PROJECT" && cozy --json machine use "$ON") > "$OUT/use-$1.json" 2>&1 || fail "machine use $ON: $(tail -c 300 "$OUT/use-$1.json")"
  run_page import "$OUT/photo.png" --name photo > "$OUT/photo-$1.json" 2>&1 || fail "import photo: $(tail -c 300 "$OUT/photo-$1.json")"
  show
  sleep 3
  local asked answered began=$SECONDS
  jq -n '[{name: "turned", tool: "orbit/orbit", kind: "video", fields: {picture: "<photo>", subject: "lighthouse", duration: 5}}]' > "$OUT/boxes-$1.json"
  asked=$(date +%s%3N)
  run_page boxes "$OUT/boxes-$1.json" > "$OUT/boxes-$1.txt" 2>&1 || fail "turned: $(tail -c 300 "$OUT/boxes-$1.txt")"
  answered=$(date +%s%3N)
  until cozy machine logs local calcifer --lines 3000 2> /dev/null | grep -q "$RUN_PROJECT: turned guessed on"; do
    [ $((SECONDS - began)) -lt 180 ] || { say "project $1: turned not guessed in 3 min"; break; }
    sleep 2
  done
  timeline "$1" "$asked" "$answered" "$answered" > /dev/null
  say "project $1: $(head -1 "$OUT/timeline-$1.txt")"
  tail -n +2 "$OUT/timeline-$1.txt" | sed 's/^/  /' | tee -a "$R"
}
if [ -n "$POD" ]; then
  # The pod on the session's TensorD: the open RC's builds once, then this cozy's (edit-ahead.sh `ours`).
  quote "$(param need_s 900)" "$POD"
  rent
  ON=$NAME
  step rc machine update "$ON" -y || fail "$ON didn't move to the open RC's builds"
  for _ in 1 2 3; do
    [ "$(cozy --json machine show "$ON" 2> /dev/null | jq -r '.status.version // empty')" = "$V" ] && break
    step tensord machine update "$ON" --tensord "$(command -v cozy)" -y || fail "$ON refused the session's TensorD"
    sleep 30
  done
  say "$ON runs TensorD $(cozy --json machine show "$ON" 2> /dev/null | jq -r '.status.version // empty')"
fi
# uploaded <label>: a fresh project on $ON that no page shows renders `edit`, a weightless box reading `pic`, a new picture of noise
# (no guess carried it, and too large to go with its admission): what its machine landed, and in how many requests.
uploaded() {
  RUN_PROJECT=$OUT/project-$1
  run_project "$RUN_PROJECT" > "$OUT/project-$1.json" 2>&1 || fail "project $1: $(tail -c 300 "$OUT/project-$1.json")"
  (cd "$RUN_PROJECT" && cozy --json machine use "$ON") > "$OUT/use-$1.json" 2>&1 || fail "machine use $ON: $(tail -c 300 "$OUT/use-$1.json")"
  nice -n 19 ffmpeg -v error -y -f lavfi -i 'nullsrc=s=1024x1024,geq=random(1)*255:random(2)*255:random(3)*255' -frames:v 1 "$OUT/pic.png" || fail "no noise"
  run_page import "$OUT/pic.png" --name pic > "$OUT/pic-$1.json" 2>&1 || fail "import pic: $(tail -c 300 "$OUT/pic-$1.json")"
  local boxed asked
  boxed=$(cd "$(dirname "$0")/../../e2e/cli/project/boxed" && pwd)
  run_page add edit --tool "$boxed/plain" --fields '{"instructions": "<pic> a shade lighter"}' > "$OUT/edit-$1.json" 2>&1 || fail "edit: $(tail -c 300 "$OUT/edit-$1.json")"
  asked=$(date +%s%3N)
  timeout 900 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$ON" edit > "$OUT/render-$1.txt" 2>&1 || fail "edit's render: $(tail -c 400 "$OUT/render-$1.txt")"
  say "project $1: edit rendered on $ON in $(( $(date +%s%3N) - asked )) ms; pic is $(stat -c %s "$OUT/pic.png") bytes"
  cozy machine logs "$ON" tensord --since 2h -n 0 > "$OUT/tensord-$ON.log" 2>&1 || true
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
  python3 -I - "$OUT/tensord-$ON.log" "$asked" <<'PY' | tee -a "$R"
import datetime, sys
since = int(sys.argv[2])
for line in open(sys.argv[1], errors="replace"):
    stamp = line.split(" ", 1)[0]
    if stamp[:4].isdigit() and datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp() * 1000 >= since and ("landed in one request" in line or "PUT /v1/objects/" in line):
        print("  " + stamp[11:23] + " " + line.split("  INFO ", 1)[-1].strip()[:200])
PY
  grep -o "[0-9]* objects ([^)]*) landed in one request in [0-9]* ms" "$OUT/calcifer.log" | sed 's/^/  calcifer: /' | tee -a "$R"
}
if [ "$KIND" = orbit ]; then
  turned a
  RUN_PROJECT=$OUT/project-b
  turned b
  [ -z "$POD" ] || uploaded c
else
  began=$SECONDS
  (cd "$RUN_PROJECT" && cozy --json package install "$SOURCE/qwen-image-2" --on local) > "$OUT/install.json" 2>&1 ||
    fail "qwen-image-2 not installed on local: $(tail -c 300 "$OUT/install.json")"
  say "qwen-image-2 installed on local in $((SECONDS - began)) s"
  measure a
  RUN_PROJECT=$OUT/project-b
  measure b
fi
say "DONE: no judgement; the timelines are above and in timeline-a.txt, timeline-b.txt"
