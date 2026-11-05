# project-takeover (cl-746): `cozy project render … --on X` makes X the render's holder. X's own Calcifer walks it, and the computer that
# asked only hands it over and collects. One rental with SSH: a CPU SKU is enough for a to e (fixtures/client-gone/art.py renders on CPU);
# h asks a GPU of its own. The computer that asks is a second user on the pod, with its own home and Calcifer, reaching the pod's TensorD
# over fixtures/client-gone/link.py, a line this session throttles and cuts. X is the pod's own Calcifer, root's, which its TensorD keeps.
#   a. Handed over, then that computer gone for real: the line down and its CLI and Calcifer `kill -9`'d once X walks. X makes a still, a
#      shot reading it, a clip, a cut of it and a frame of it, each once. The line back at 1 MB/s and Calcifer started: `status` says
#      what isn't here yet and until when, then everything X made comes down, its bytes its hash.
#   k. Kills during the hand-off: a 20 MB plate imported there, the line down for the first ask, then at 300 KB/s both ways while that computer's
#      Calcifer is killed every few seconds wherever the hand-off is (said from its walk file each time) and started again. X makes the shot
#      that reads the plate once: one run, one version, the plate's bytes its hash on X.
#   e. Nothing owed and nothing connected, the pod ends itself within 15 minutes of its last job.
#   h. A real H3 shot on X reading a picture the hand-off uploads (a GPU SKU, and `h3`, the function: asked by name).
#   y. `--on X,Y` on a second rental: Y admits X's key through Y's door, X adds Y under this computer's name for it and sends it work
#      directly; four stills, this computer gone meanwhile, each made once across both, collected; Y no longer admits X once it's over.
#   l. X loses the project mid-render (its Calcifer killed and its copy wiped, as a fresh disk would): this computer's hold ends at once
#      with `holder_lost`, its render says so, and nothing is asked again.
# params:
#   cozy        the cozy under test, for `machine update --tensord`: a version, a file or URL#sha256= (default: the session's)
#   runtime     a Runtime for the pod with `cozy`: a wheel or a version (default: the pod's)
#   parts       which of a, k, e, h, y, l run (default a,k,e); y and l rent a second pod
#   h3          the function h renders its shot with (default minimax-h3/generate, Turbo)
#   rent_lock   a file to hold while ordering, where several sessions rent from one computer
#   traffic_gb  GB the provider bills (default 12)
#   seconds     the least time the cap must buy (default 3600)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures/client-gone" && pwd)
[ -f "$FIX/art.py" ] && [ -f "$FIX/link.py" ] || fail "no fixture checkout"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
UNDER=$(param cozy "$(command -v cozy)") LOCK=$(param rent_lock) RUNTIME=$(param runtime) H3=$(param h3 minimax-h3/generate) LINK=7777
PARTS=,$(param parts a,k,e),
has() { [[ $PARTS == *",$1,"* ]]; }
RENT_ARGS=--ssh
TWO=''; has y && TWO=1
quote "$(param traffic_gb 12)" "$(param seconds 3600)" "$SKU" ${TWO:+"$SKU"}
[ -z "$LOCK" ] || { exec 9>"$LOCK"; flock 9; }
order; [ -z "$TWO" ] || order
[ -z "$LOCK" ] || { flock -u 9; exec 9>&-; }
[ -z "$TWO" ] || { ready 1; Y=$NAME YRID=$RID; }
ready 0

# A shell there, asked again when it goes quiet or can't connect (this runner's own network drops for minutes at a time: 02:47Z on
# 11-05): everything asked this way can be asked twice, and the pod goes on without it.
sh() {
  local tries=0 code
  while :; do
    timeout 120 cozy machine ssh "$NAME" -- "$*" < /dev/null 2>> "$OUT/ssh.err"; code=$?
    { [ "$code" = 124 ] || [ "$code" = 255 ]; } && [ "$tries" -lt 20 ] || return "$code"
    tries=$((tries + 1)); say "ssh went quiet or found no route ($code): asked again: ${*:0:80}"
    [ "$code" = 124 ] || sleep 30
  done
}
# The line back up while the pod is there (e ends it), and the session's own exit status kept for `finish`.
trap 'st=$?; [ "$(state "$RID")" != ready ] || sh "echo up > /tmp/link"; [ "$RC" != 0 ] || RC=$st; finish' EXIT
# What a failure leaves to read once the pod is gone: X's runs, and both Calcifers' logs.
fail() {
  say "FAIL: $*"
  if [ -n "${TAG:-}" ]; then
    x "--json --full run ls --limit 200" > "$OUT/fail-x-runs.json" 2>&1
    x "--json --full project status" > "$OUT/fail-x-status.json" 2>&1
    sh 'tail -300 /root/.cozy/calcifer/calcifer.log' > "$OUT/fail-x-calcifer.log" 2>&1
    sh 'tail -300 /home/client/.cozy/calcifer/calcifer.log' > "$OUT/fail-client-calcifer.log" 2>&1
  fi
  RC=1; exit 1
}
# c <cozy args>: as the computer that asks, in the part's project. x <cozy args>: as X, in its copy of it.
c() { sh "su client -s /bin/sh -c 'cd \$HOME/$TAG 2>/dev/null; /opt/client/cozy $*'"; }
x() { sh "cd \$(ls -d /root/.cozy/projects/$TAG-* | head -1) && cozy $*"; }
line() { sh "echo $* > /tmp/link"; }
client_calcifer() { sh 'pgrep -u client -x calcifer' | tr -dc '0-9\n' | head -1; }
# gone <pid…>: each `kill -9`'d there, and checked dead.
killed() {
  local pid
  for pid in "$@"; do
    [ -n "$pid" ] || fail "$TAG: nothing to kill"
    sh "kill -9 $pid; sleep 1; ! kill -0 $pid 2>/dev/null" || fail "$TAG: $pid lives after kill -9"
  done
}
# write <path>: stdin, as the file <path> of the computer that asks, whole or not at all, and checked by its length: a connection that
# stalls (seen here: 6 min on one cat) is asked again.
write() {
  local text tries=0; text=$(cat)
  until [ "$(printf '%s\n' "$text" | timeout 60 cozy machine ssh "$NAME" -- "cat > $1.new && chown client $1.new && mv $1.new $1 && wc -c < $1" 2>> "$OUT/ssh.err" | tr -dc 0-9)" = "$(printf '%s\n' "$text" | wc -c)" ]; do
    tries=$((tries + 1)); [ $tries -lt 4 ] || return 1
    say "writing $1 again"
  done
}
walk_file() { sh "cat /home/client/$TAG/.runs/walk.json 2>/dev/null"; }
# Where the hand-off stands, from that computer's walk file.
handing() { jq -r 'if .held == null then "nothing held" elif (.held.id // "") == "" then "not yet taken" elif (.held.asking // []) != [] then "taken, the render not yet asked" else "asked" end' <<<"${1:-{\}}" 2>/dev/null; }
# The files X's copy has, `sha256 path`, and the computer's.
x_files() { x "--json --full project status" | jq -r '[.assets[].prefixes[]?.main | select(. != null) | "\(.sha256) \(.path)"] | unique | .[]'; }

sh 'useradd -m client && install -d /opt/client && command -v python3 && command -v ffmpeg' > "$OUT/tools.txt" || fail "no second user or tools: $(tail -3 "$OUT/ssh.err")"
cozy machine ssh "$NAME" -- 'cat > /root/link.py' < "$FIX/link.py" 2>> "$OUT/ssh.err" || fail "copy link"
write /home/client/art.py < "$FIX/art.py" || fail "copy art"

step update machine update "$NAME" --tensord "$UNDER" ${RUNTIME:+--runtime "$RUNTIME"} -y || fail "update to $UNDER"
# A command in a shell there restarts the pod's Calcifer on the new binary.
was=$(sh 'grep -o "\"pid\":[0-9]*" /root/.cozy/calcifer/calcifer.lock.holder | cut -d: -f2')
sh 'cozy --full machine show local --json' > "$OUT/local.json" || fail "cozy machine show local"
now=$(sh 'grep -o "\"pid\":[0-9]*" /root/.cozy/calcifer/calcifer.lock.holder | cut -d: -f2')
say "the pod runs $(sh 'cozy --version'); its Calcifer was pid $was, is $now, running $(sh "readlink /proc/$now/exe")"
ADDRESS=$(jq -r .address "$OUT/local.json") FINGERPRINT=$(jq -r .fingerprint "$OUT/local.json")
[ -n "$ADDRESS" ] && [ "$ADDRESS" != null ] || fail "local has no address: $(head -c 300 "$OUT/local.json")"
sh "echo up > /tmp/link; nohup python3 /root/link.py $LINK ${ADDRESS%:*} ${ADDRESS##*:} /tmp/link > /tmp/link.log 2>&1 < /dev/null & echo started" > /dev/null
sh 'cp "$(command -v cozy)" /opt/client/cozy && chmod 755 /opt/client/cozy'
TAG=setup
c "machine add box 127.0.0.1:$LINK --fingerprint $FINGERPRINT" > "$OUT/add.txt" 2>&1
KEY=$(grep -oE 'cozy machine authorize [A-Za-z0-9_-]{43}' "$OUT/add.txt" | awk '{print $4}' | head -1)
[ -n "$KEY" ] || fail "no key to authorize: $(tail -3 "$OUT/add.txt")"
sh "cozy machine authorize $KEY" > "$OUT/authorize.txt" 2>&1 || fail "authorize"
c "machine add box 127.0.0.1:$LINK --fingerprint $FINGERPRINT" > "$OUT/add.txt" 2>&1 || fail "add: $(tail -3 "$OUT/add.txt")"

# new_project <tag>: the part's own project there.
new_project() {
  TAG=$1
  c "project new /home/client/$TAG" > "$OUT/new-$TAG.txt" 2>&1 || fail "project new: $(tail -3 "$OUT/new-$TAG.txt")"
  sh "cp /home/client/art.py /home/client/$TAG/art.py; chown client /home/client/$TAG/art.py"
}

# x_made <assets…>: whether X's copy has a version of each, and owes nothing.
x_made() {
  local state; state=$(x "--json --full project status") || return 1
  jq -e --args '.busy == false and ([$ARGS.positional[] as $a | [.assets[] | select(.name == $a and .versions > 0)] | length > 0] | all)' "$@" <<<"$state" > /dev/null 2>&1
}

# collected: whether every file X's copy names is here, whole.
collected() {
  local sha path ok=0
  while read -r sha path; do
    [ -n "$sha" ] || continue
    [ "sha256:$(sh "sha256sum /home/client/$TAG/$path 2>/dev/null" | cut -c1-64)" = "$sha" ] || ok=1
  done < <(x_files)
  return $ok
}

# once <assets…>: X's copy holds one version of each, and every call X ran for the project is one version's run: none ran twice.
once() {
  local state runs made
  state=$(x "--json --full project status") || fail "$TAG: X's status: $state"
  jq -e --args '[$ARGS.positional[] as $a | [.assets[] | select(.name == $a and .versions == 1)] | length == 1] | all' "$@" <<<"$state" > /dev/null ||
    fail "$TAG: not one version each: $(jq -c '[.assets[] | {name, versions}]' <<<"$state")"
  # Its calls: what a machine fetches ahead for it is a warm run, which renders nothing.
  runs=$(x "--json --full run ls --limit 200" | jq -c --arg t "/projects/$TAG-" '[.runs[] | select(((.project // "") | contains($t)) and ((.kind // "call") | ascii_downcase) == "call" and (.target | startswith("ahead ") | not)) | .number] | sort')
  made=$(jq -c '[.assets[].prefixes[]?.run | select(. != null)] | sort' <<<"$state")
  [ "$runs" = "$made" ] || fail "$TAG: X ran $runs, and its versions came of $made"
  say "$TAG: one version each of $*; X's calls $runs are its versions' runs, none twice"
}

part_a() {
  new_project a
  say "a: handed over, then this computer gone"
  write "/home/client/a/project.yaml" <<YAML || fail "a: its project.yaml wasn't written"
version: 3
assets:
  still:
    tool: ./art.py/make
    seconds: 25
    side: 1024
    instructions: a still $RANDOM
  shot:
    tool: ./art.py/make
    instructions: reads <still> $RANDOM
  clip:
    tool: ./art.py/film
    seconds: 3
    instructions: noise $RANDOM
  edit:
    tool: cozy/edit
    tracks:
      - clips:
          - <clip>
  aframe:
    tool: cozy/frame
    video: <clip>
    at: 1
YAML
  sh "su client -s /bin/sh -c 'cd \$HOME/a; nohup /opt/client/cozy project render all --on box --plain > \$HOME/a.out 2>&1 < /dev/null & echo \$! > /tmp/a.pid'" > /dev/null
  local t=0 walk=''
  while [ $t -lt 300 ]; do
    walk=$(walk_file)
    [ "$(handing "$walk")" = asked ] && x "--json --full project status" 2>/dev/null | jq -e .busy > /dev/null && break
    t=$((t + 3)); sleep 3
  done
  echo "$walk" > "$OUT/a-walk.json"
  [ "$(handing "$walk")" = asked ] || fail "a: X never took the render: $(handing "$walk"); $(sh 'tail -5 /home/client/a.out' | paste -sd'|' | cut -c1-300)"
  say "a: X walks it ($t s after asking); the line down, this computer's CLI and Calcifer killed"
  line down
  killed "$(sh 'cat /tmp/a.pid' | tr -dc 0-9)" "$(client_calcifer)"
  local least=999999999 deadline left at=$SECONDS moved='' was='' began=$SECONDS
  until x_made still shot clip edit aframe; do
    deadline=$(cozy machine show "$NAME" --json | jq -r '.status.idle_deadline_ms // 0')
    if [ "$deadline" != 0 ]; then left=$(( (deadline - $(date +%s%3N)) / 1000 )); [ "$left" -ge "$least" ] || least=$left; fi
    moved=$(x "--json --full project status" | jq -c '[.board.jobs[]? | [.asset, .state, .stage]]')
    [ "$moved" != "$was" ] && { was=$moved; at=$SECONDS; }
    [ $((SECONDS - at)) -lt 300 ] || fail "a: X's render stopped moving for 5 min: $moved"
    sleep 5
  done
  t=$((SECONDS - began))
  x "--json --full project status" > "$OUT/a-x-status.json"
  x "--json --full run ls --limit 200" > "$OUT/a-x-runs.json"
  say "a: X made all five in $t s with nobody here; its idle clock at its least had $([ "$least" = 999999999 ] && echo 'never run' || echo "${least} s left")"
  once still shot clip edit aframe
  # Back at 200 KB/s: what status says while what X made comes down.
  line "up 200000"
  t=0
  until collected; do
    [ $t -lt 600 ] || fail "a: not all collected in $t s: $(c 'project status' | tail -4 | paste -sd'|' | cut -c1-300)"
    c "project status 2>&1" > "$OUT/a-back.txt"
    grep -E 'on box until|rendering on box|all here' "$OUT/a-back.txt" >> "$OUT/a-held.txt"
    t=$((t + 5)); sleep 3
  done
  say "a: this computer back, its status said: $(sort -u "$OUT/a-held.txt" | head -3 | paste -sd'|')"
  grep -qE 'on box until [0-9]{2}:[0-9]{2}: [0-9]+ files' "$OUT/a-held.txt" || fail "a: status never said what isn't here yet and until when"
  line up
  sleep 6
  c "project status" > "$OUT/a-collected.txt" 2>&1
  [ "$(handing "$(walk_file)")" = "nothing held" ] || fail "a: everything here, and the hold isn't over: $(walk_file | head -c 300)"
  local sha path
  while read -r sha path; do
    cozy machine ssh "$NAME" -- "cat /home/client/a/$path" < /dev/null > "$OUT/a-${path##*/}" 2>> "$OUT/ssh.err"
  done < <(x_files)
  cozy machine ssh "$NAME" -- "cat /home/client/a/edit.mp4" < /dev/null > "$OUT/a-edit.mp4" 2>> "$OUT/ssh.err"
  say "a: all $(x_files | wc -l) files collected $t s after it came back, each its hash; the hold is over; the film here: $(sh 'ffprobe -v error -show_entries stream=codec_type,nb_frames -of csv=p=0 /home/client/a/edit.mp4' | paste -sd' ')"
}

part_k() {
  new_project k
  say "k: kills during the hand-off"
  sh "ffmpeg -v error -f lavfi -i 'nullsrc=s=2900x2900,geq=random(1)*255:random(1)*255:random(1)*255' -frames:v 1 -y /home/client/plate.png && chown client /home/client/plate.png && stat -c %s /home/client/plate.png" > "$OUT/k-plate.txt" || fail "k: no plate"
  write "/home/client/k/project.yaml" <<YAML || fail "k: its project.yaml wasn't written"
version: 3
assets:
  shotk:
    tool: ./art.py/make
    instructions: reads <plate> $RANDOM
YAML
  c "project import /home/client/plate.png" > "$OUT/k-import.txt" 2>&1 || fail "k: import: $(tail -3 "$OUT/k-import.txt")"
  line down
  sh "su client -s /bin/sh -c 'cd \$HOME/k; nohup /opt/client/cozy project render shotk --on box --plain > \$HOME/k.out 2>&1 < /dev/null &'" > /dev/null
  local kills=0 where walk
  sleep 3
  while :; do
    walk=$(walk_file); where=$(handing "$walk")
    [ "$where" = asked ] && break
    # Held before and nothing held now: the render ended between two looks, made or not, and what follows says which.
    [ "$where" = "nothing held" ] && [ $kills -gt 0 ] && { sh 'tail -6 /home/client/k.out' > "$OUT/k-out.txt"; break; }
    [ $kills -lt 60 ] || fail "k: the hand-off didn't end in $kills kills: $where"
    kills=$((kills + 1))
    say "k: kill $kills: $where; X holds $(sh 'du -c -B1 /root/.cozy/calcifer/files/parts/*.part 2>/dev/null | tail -1 | cut -f1') bytes of uploads under way"
    killed "$(client_calcifer)"
    [ $kills = 1 ] && line "up 300000 both"
    c "project status" > /dev/null 2>&1
    sleep 6
  done
  line up
  sh 'grep -E "files|PUT|upload" /root/.cozy/calcifer/calcifer.log | tail -20' > "$OUT/k-x-uploads.log" 2>&1
  say "k: asked after $kills kills"
  local t=0 state
  until x_made shotk; do
    state=$(x "--json --full project status")
    jq -e '.busy == false and ([.assets[] | select(.name == "shotk" and .versions > 0)] | length == 0)' <<<"$state" > /dev/null 2>&1 &&
      fail "k: X's walk ended without shotk: $(jq -c '[.board.jobs[]? | select(.asset == "shotk") | .error.message // .why] | first' <<<"$state" | cut -c1-400); this computer said: $(sh 'tail -4 /home/client/k.out' | paste -sd'|' | cut -c1-400)"
    [ $t -lt 300 ] || fail "k: X didn't make shotk in $t s"
    t=$((t + 5)); sleep 5
  done
  once shotk
  local plate; plate=$(x "--json --full project status" | jq -r '[.assets[] | select(.name == "plate") | .sha256] | first')
  [ "sha256:$(sh "sha256sum /root/.cozy/projects/k-*/files/${plate#sha256:}.png" | cut -c1-64)" = "$plate" ] || fail "k: the plate on X is not its hash"
  x "--json --full project status" > "$OUT/k-x-status.json"
  say "k: shotk made once on X, the plate whole there"
  t=0
  until collected && [ "$(handing "$(walk_file)")" = "nothing held" ]; do
    [ $t -lt 300 ] || fail "k: not collected in $t s"
    t=$((t + 5)); sleep 5
  done
}

part_e() {
  say "e: nothing owed, nothing connected: the pod ends itself"
  local t=0 s
  while [ $t -lt 1500 ]; do
    s=$(state "$RID")
    [ "$s" = ended ] || [ -z "$s" ] && break
    t=$((t + 20)); sleep 20
  done
  [ "$s" = ended ] || [ -z "$s" ] || fail "e: still $s after $t s"
  say "e: ended itself $t s after the last part"
}

part_h() {
  new_project h
  say "h: a real H3 shot on X, reading a picture the hand-off uploads"
  sh "ffmpeg -v error -f lavfi -i 'color=c=0x1d4e89:s=832x480,drawbox=x=300:y=140:w=232:h=200:color=0xf2c14e:t=fill' -frames:v 1 -y /home/client/sea.png && chown client /home/client/sea.png" || fail "h: no picture"
  write "/home/client/h/project.yaml" <<YAML || fail "h: its project.yaml wasn't written"
version: 3
assets:
  seashot:
    tool: $H3
    first_frame: <sea>
    duration_s: 5
    prompt: A golden lantern floats on a deep blue sea at night. The camera drifts slowly toward it as its warm light flickers on small waves lapping around it, and a soft wind hums over the water.
YAML
  c "project import /home/client/sea.png" > "$OUT/h-import.txt" 2>&1 || fail "h: import"
  sh "su client -s /bin/sh -c 'cd \$HOME/h; nohup /opt/client/cozy project render seashot --on box --plain > \$HOME/h.out 2>&1 < /dev/null & echo \$! > /tmp/h.pid'" > /dev/null
  local t=0
  until [ "$(handing "$(walk_file)")" = asked ] || sh 'grep -q "took the render over" /home/client/h.out'; do [ $t -lt 300 ] || fail "h: never handed"; t=$((t + 3)); sleep 3; done
  line down
  killed "$(sh 'cat /tmp/h.pid' | tr -dc 0-9)" "$(client_calcifer)"
  say "h: X walks it, this computer gone"
  local was='' at=$SECONDS moved
  t=0
  local state
  until x_made seashot; do
    state=$(x "--json --full project status")
    jq -e '.busy == false and ([.assets[] | select(.name == "seashot" and .versions > 0)] | length == 0)' <<<"$state" > /dev/null 2>&1 &&
      fail "h: X's walk ended without seashot: $(jq -c '[.board.jobs[]? | select(.asset == "seashot") | .error.message // .why] | first' <<<"$state" | cut -c1-400)"
    moved=$(jq -c '[.board.jobs[]? | [.state, .stage, .detail]]' <<<"$state")
    [ "$moved" != "$was" ] && { was=$moved; at=$SECONDS; say "h: $moved"; }
    [ $((SECONDS - at)) -lt 900 ] || fail "h: stopped moving for 15 min"
    t=$((t + 15)); sleep 15
  done
  line up
  t=0
  until collected; do
    [ $t -lt 900 ] || fail "h: not collected"
    c "project status 2>&1" | grep -E 'on box|rendering on' >> "$OUT/h-held.txt"
    t=$((t + 5)); sleep 5
  done
  say "h: this computer back, its status said: $(sort -u "$OUT/h-held.txt" | head -2 | paste -sd'|')"
  local sha path
  while read -r sha path; do
    case $path in *.mp4) cozy machine ssh "$NAME" -- "cat /home/client/h/$path" < /dev/null > "$OUT/h-seashot.mp4" 2>> "$OUT/ssh.err" ;; esac
  done < <(x_files)
  say "h: the shot came down whole: $(ffprobe -v error -show_entries stream=codec_type,nb_frames -of csv=p=0 "$OUT/h-seashot.mp4" | paste -sd' ')"
}

# On Y, the second rental: its update, and a shell there.
if [ -n "$TWO" ]; then
  step update-y machine update "$Y" --tensord "$UNDER" ${RUNTIME:+--runtime "$RUNTIME"} -y || fail "update $Y to $UNDER"
  cozy machine ssh "$Y" -- 'cozy --full machine show local --json' < /dev/null > "$OUT/y-local.json" 2>> "$OUT/ssh.err" || fail "cozy machine show local on $Y"
fi
ysh() { NAME=$Y sh "$@"; }

part_y() {
  new_project y
  say "y: X drives Y ($Y)"
  local address fingerprint
  cozy machine show "$Y" --json > "$OUT/y-show.json"
  address=$(jq -r .address "$OUT/y-show.json") fingerprint=$(jq -r .fingerprint "$OUT/y-show.json")
  ysh "cozy machine authorize $KEY" > "$OUT/y-authorize.txt" 2>&1 || fail "y: $Y won't admit this computer"
  c "machine add ybox $address --fingerprint $fingerprint" > "$OUT/y-add.txt" 2>&1 || fail "y: add ybox: $(tail -3 "$OUT/y-add.txt")"
  write "/home/client/y/project.yaml" <<YAML || fail "y: its project.yaml wasn't written"
version: 3
assets:
  north:
    tool: ./art.py/make
    seconds: 40
    instructions: north $RANDOM
  south:
    tool: ./art.py/make
    seconds: 40
    instructions: south $RANDOM
  east:
    tool: ./art.py/make
    seconds: 40
    instructions: east $RANDOM
  west:
    tool: ./art.py/make
    seconds: 40
    instructions: west $RANDOM
YAML
  sh "su client -s /bin/sh -c 'cd \$HOME/y; nohup /opt/client/cozy project render all --on box,ybox --plain > \$HOME/y.out 2>&1 < /dev/null & echo \$! > /tmp/y.pid'" > /dev/null
  local t=0
  until [ "$(handing "$(walk_file)")" = asked ]; do [ $t -lt 300 ] || fail "y: X never took it: $(sh 'tail -5 /home/client/y.out' | paste -sd'|')"; t=$((t + 3)); sleep 3; done
  line down
  killed "$(sh 'cat /tmp/y.pid' | tr -dc 0-9)" "$(client_calcifer)"
  say "y: X walks it, this computer gone"
  t=0
  until x_made north south east west; do [ $t -lt 900 ] || fail "y: X didn't make all four"; t=$((t + 5)); sleep 5; done
  x "--json --full run ls --limit 50" > "$OUT/y-x-runs.json"
  once north south east west
  local on_y; on_y=$(jq --arg t "/projects/y-" '[.runs[] | select(((.project // "") | contains($t)) and (.target | startswith("ahead ") | not) and .machine == "ybox")] | length' "$OUT/y-x-runs.json")
  [ "$on_y" -gt 0 ] || fail "y: X sent Y nothing: $(jq -c '[.runs[] | [.number, .machine, .target]]' "$OUT/y-x-runs.json")"
  local xkey; xkey=$(sh 'cozy --json --full machine show local' | jq -r .controller_key)
  line up
  t=0
  until collected && [ "$(handing "$(walk_file)")" = "nothing held" ]; do
    [ $t -lt 600 ] || fail "y: not collected"
    c "project status 2>&1" > /dev/null
    t=$((t + 5)); sleep 5
  done
  # Taking it back changes nothing when the hold's end already did.
  ysh "cozy --json machine authorize --remove $xkey" > "$OUT/y-key.json" 2>&1
  say "y: four made once, $on_y of their calls on $Y, sent there by X; all collected; X's key still on $Y after: $(jq -r .changed "$OUT/y-key.json")"
  [ "$(jq -r .changed "$OUT/y-key.json")" = false ] || fail "y: $Y still admitted X once the hold was over: $(head -c 300 "$OUT/y-key.json")"
}

part_l() {
  new_project l
  say "l: X loses the project mid-render"
  write "/home/client/l/project.yaml" <<YAML || fail "l: its project.yaml wasn't written"
version: 3
assets:
  slow:
    tool: ./art.py/make
    seconds: 120
    instructions: slow $RANDOM
YAML
  sh "su client -s /bin/sh -c 'cd \$HOME/l; nohup /opt/client/cozy project render all --on box --plain > \$HOME/l.out 2>&1 < /dev/null &'" > /dev/null
  local t=0
  until [ "$(handing "$(walk_file)")" = asked ] && x "--json --full project status" 2>/dev/null | jq -e .busy > /dev/null; do [ $t -lt 300 ] || fail "l: X never took it"; t=$((t + 3)); sleep 3; done
  sh 'p=$(pgrep -u root -x calcifer); python3 -c "import json; f=\"/root/.cozy/calcifer/projects.json\"; i=json.load(open(f)); i[\"projects\"]={k: v for k, v in i[\"projects\"].items() if \"/projects/l-\" not in v[\"path\"]}; json.dump(i, open(f, \"w\"))"; rm -rf /root/.cozy/projects/l-*; kill -9 $p' > /dev/null
  say "l: X walks it; X's Calcifer killed and its copy wiped"
  t=0
  until sh 'grep -q "gone before" /home/client/l.out' && [ "$(handing "$(walk_file)")" = "nothing held" ]; do
    [ $t -lt 300 ] || fail "l: the hold never ended: $(sh 'tail -4 /home/client/l.out' | paste -sd'|')"
    t=$((t + 5)); sleep 5
  done
  sh 'tail -4 /home/client/l.out' > "$OUT/l-out.txt"
  sh 'cozy machine show local' | grep -E '^(running|queued) ' | paste -sd' ' > "$OUT/l-x-machine.txt"
  say "l: this computer's render ended $t s later: $(grep -m1 -E 'gone before' "$OUT/l-out.txt" | cut -c1-200); X now: $(cat "$OUT/l-x-machine.txt")"
}

for part in a k h y l e; do
  ! has "$part" || "part_$part"
done
say "done: $PARTS"
