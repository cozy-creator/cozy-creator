# project-takeover (cl-746): `cozy project render … --on X` makes X the render's holder. X's own Calcifer walks it, and the computer that
# asked only hands it over and collects. One rental with SSH: a CPU SKU is enough for a to e (fixtures/client-gone/art.py renders on CPU);
# h asks a GPU of its own. The computer that asks is a second user on the pod, with its own home and Calcifer, reaching the pod's TensorD
# over fixtures/client-gone/link.py, a line this session throttles and cuts. X is the pod's own Calcifer, root's, which its TensorD keeps.
#   a. Handed over, then that computer gone for real: the line down and its CLI and Calcifer `kill -9`'d once X walks. X makes a still, a
#      shot reading it, a clip, a cut of it and a frame of it, each once. The line back at 1 MB/s and Calcifer started: `status` says
#      what isn't here yet and until when, then everything X made comes down, its bytes its hash.
#   k. Kills during the hand-off: a 25 MB plate imported there, the line down for the first ask, then at 300 KB/s while that computer's
#      Calcifer is killed every few seconds wherever the hand-off is (said from its walk file each time) and started again. X makes the shot
#      that reads the plate once: one run, one version, the plate's bytes its hash on X.
#   e. Nothing owed and nothing connected, the pod ends itself within 15 minutes of its last job.
#   h. A real H3 shot on X reading a picture the hand-off uploads (a GPU SKU, and `h3`, the function: asked by name).
# params:
#   cozy        the cozy under test, for `machine update --tensord`: a version, a file or URL#sha256= (default: the session's)
#   runtime     a Runtime for the pod with `cozy`: a wheel or a version (default: the pod's)
#   parts       which of a, k, e, h run (default a,k,e)
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
quote "$(param traffic_gb 12)" "$(param seconds 3600)"
[ -z "$LOCK" ] || { exec 9>"$LOCK"; flock 9; }
order
[ -z "$LOCK" ] || { flock -u 9; exec 9>&-; }
ready

# A shell there, asked again when it goes quiet: everything asked this way can be asked twice.
sh() {
  local tries=0 code
  while :; do
    timeout 120 cozy machine ssh "$NAME" -- "$*" < /dev/null 2>> "$OUT/ssh.err"; code=$?
    [ "$code" = 124 ] && [ "$tries" -lt 3 ] || return "$code"
    tries=$((tries + 1)); say "ssh went quiet: asked again: ${*:0:80}"
  done
}
trap 'sh "echo up > /tmp/link"; finish' EXIT
# What a failure leaves to read once the pod is gone: X's runs, and both Calcifers' logs.
fail() {
  say "FAIL: $*"
  if [ -n "${TAG:-}" ]; then
    x "--json run ls --limit 200" > "$OUT/fail-x-runs.json" 2>&1
    x "--json project status" > "$OUT/fail-x-status.json" 2>&1
    sh 'tail -300 /root/.cozy/calcifer/calcifer.log' > "$OUT/fail-x-calcifer.log" 2>&1
    sh 'tail -300 /home/client/.cozy/calcifer/calcifer.log' > "$OUT/fail-client-calcifer.log" 2>&1
  fi
  RC=1; exit 1
}
# c <cozy args>: as the computer that asks, in the part's project. x <cozy args>: as X, in its copy of it.
c() { sh "su client -s /bin/sh -c 'cd \$HOME/$TAG 2>/dev/null; /opt/client/cozy $*'"; }
x() { sh "cd \$(ls -d /root/.cozy/projects/$TAG-* | head -1) && cozy $*"; }
line() { sh "echo $* > /tmp/link"; }
client_calcifer() { sh 'grep -o "\"pid\":[0-9]*" /home/client/.cozy/calcifer/calcifer.lock.holder | cut -d: -f2'; }
write() { cozy machine ssh "$NAME" -- "cat > $1; chown client $1" 2>> "$OUT/ssh.err"; }
walk_file() { sh "cat /home/client/$TAG/.runs/walk.json 2>/dev/null"; }
# Where the hand-off stands, from that computer's walk file.
handing() { jq -r 'if .held == null then "nothing held" elif (.held.id // "") == "" then "not yet taken" elif (.held.asking // []) != [] then "taken, the render not yet asked" else "asked" end' <<<"${1:-{\}}" 2>/dev/null; }
# The files X's copy has, `sha256 path`, and the computer's.
x_files() { x "--json project status" | jq -r '[.assets[].prefixes[]?.main | select(. != null) | "\(.sha256) \(.path)"] | unique | .[]'; }

sh 'useradd -m client && install -d /opt/client && command -v python3 && command -v ffmpeg' > "$OUT/tools.txt" || fail "no second user or tools: $(tail -3 "$OUT/ssh.err")"
cozy machine ssh "$NAME" -- 'cat > /root/link.py' < "$FIX/link.py" 2>> "$OUT/ssh.err" || fail "copy link"
write /home/client/art.py < "$FIX/art.py" || fail "copy art"

step update machine update "$NAME" --tensord "$UNDER" ${RUNTIME:+--runtime "$RUNTIME"} -y || fail "update to $UNDER"
# A command in a shell there restarts the pod's Calcifer on the new binary.
was=$(sh 'grep -o "\"pid\":[0-9]*" /root/.cozy/calcifer/calcifer.lock.holder | cut -d: -f2')
sh 'cozy machine show local --json' > "$OUT/local.json" || fail "cozy machine show local"
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
  local state; state=$(x "--json project status") || return 1
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

# once <asset> <function> <n>: X's copy holds n versions of it, and X ran its function n times, none twice.
once() {
  local versions runs
  versions=$(x "--json project status" | jq --arg a "$1" '[.assets[] | select(.name == $a) | .versions] | first // 0')
  # Its calls: what a machine fetches ahead for it is a warm run, which renders nothing.
  runs=$(x "--json run ls --limit 200" | jq --arg f "$2" --arg t "/projects/$TAG-" '[.runs[] | select(((.project // "") | contains($t)) and ((.kind // "call") | ascii_downcase) == "call" and (.target | startswith("ahead ") | not) and (.target | split("@")[0] | split("/")[-1] == $f))] | length')
  [ "$versions" = "$3" ] && [ "$runs" = "$3" ] || fail "$TAG: $1 has $versions versions, and X ran $2 $runs times, not $3"
}

part_a() {
  new_project a
  say "a: handed over, then this computer gone"
  write "/home/client/a/project.yaml" <<YAML
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
    [ "$(handing "$walk")" = asked ] && x "--json project status" 2>/dev/null | jq -e .busy > /dev/null && break
    t=$((t + 3)); sleep 3
  done
  echo "$walk" > "$OUT/a-walk.json"
  [ "$(handing "$walk")" = asked ] || fail "a: X never took the render: $(handing "$walk"); $(sh 'tail -5 /home/client/a.out' | paste -sd'|' | cut -c1-300)"
  say "a: X walks it ($t s after asking); the line down, this computer's CLI and Calcifer killed"
  line down
  sh "kill -9 \$(cat /tmp/a.pid) $(client_calcifer)"
  local least=999999999 deadline left at=$SECONDS moved='' was='' began=$SECONDS
  until x_made still shot clip edit aframe; do
    deadline=$(cozy machine show "$NAME" --json | jq -r '.status.idle_deadline_ms // 0')
    if [ "$deadline" != 0 ]; then left=$(( (deadline - $(date +%s%3N)) / 1000 )); [ "$left" -ge "$least" ] || least=$left; fi
    moved=$(x "--json project status" | jq -c '[.board.jobs[]? | [.asset, .state, .stage]]')
    [ "$moved" != "$was" ] && { was=$moved; at=$SECONDS; }
    [ $((SECONDS - at)) -lt 300 ] || fail "a: X's render stopped moving for 5 min: $moved"
    sleep 5
  done
  t=$((SECONDS - began))
  x "--json project status" > "$OUT/a-x-status.json"
  x "--json run ls --limit 200" > "$OUT/a-x-runs.json"
  say "a: X made all five in $t s with nobody here; its idle clock at its least had $([ "$least" = 999999999 ] && echo 'never run' || echo "${least} s left")"
  once still make 1; once shot read 1; once clip film 1
  line "up 1000000"
  c "project status" > /dev/null 2>&1
  sleep 8
  c "project status" > "$OUT/a-back.txt" 2>&1
  say "a: this computer back: $(grep -E 'on box|rendering on|all here' "$OUT/a-back.txt" | head -2 | paste -sd'|')"
  grep -qE 'on box until [0-9]{2}:[0-9]{2}|rendering on box' "$OUT/a-back.txt" || fail "a: status says nothing of where its render is"
  t=0
  until collected; do
    [ $t -lt 600 ] || fail "a: not all collected in $t s: $(c 'project status' | tail -4 | paste -sd'|' | cut -c1-300)"
    t=$((t + 5)); sleep 5
  done
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
  write "/home/client/k/project.yaml" <<YAML
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
    [ $kills -lt 60 ] || fail "k: the hand-off didn't end in $kills kills: $where"
    kills=$((kills + 1))
    say "k: kill $kills: $where; X holds $(sh 'cat /root/.cozy/calcifer/files/parts/*.held 2>/dev/null | head -c 80' | tr -d '\n') of the plate"
    sh "kill -9 $(client_calcifer)"
    [ $kills = 1 ] && line "up 300000"
    c "project status" > /dev/null 2>&1
    sleep 6
  done
  line up
  say "k: asked after $kills kills"
  local t=0
  until x_made shotk; do
    [ $t -lt 300 ] || fail "k: X didn't make shotk in $t s"
    t=$((t + 5)); sleep 5
  done
  once shotk read 1
  local plate; plate=$(x "--json project status" | jq -r '[.assets[] | select(.name == "plate") | .sha256] | first')
  [ "sha256:$(sh "sha256sum /root/.cozy/projects/k-*/files/${plate#sha256:}.png" | cut -c1-64)" = "$plate" ] || fail "k: the plate on X is not its hash"
  x "--json project status" > "$OUT/k-x-status.json"
  say "k: shotk made once on X, from one run; the plate whole there; it read $(x '--json project status' | jq -r '[.assets[] | select(.name == "shotk") | .prefixes[0].run] | first' | xargs -I{} sh "cd /root/.cozy/projects/k-* && cozy --json run show {} --once" | jq -r '.result.read_bytes // "?"') pixels"
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
  write "/home/client/h/project.yaml" <<YAML
version: 3
assets:
  seashot:
    tool: $H3
    summary: A slow push in on <sea>, a golden lantern floating on a deep blue sea at night, small waves lapping around it.
    beats:
      - duration: 5
        description: The camera drifts toward the lantern as its light flickers on the water.
YAML
  c "project import /home/client/sea.png" > "$OUT/h-import.txt" 2>&1 || fail "h: import"
  sh "su client -s /bin/sh -c 'cd \$HOME/h; nohup /opt/client/cozy project render seashot --on box --plain > \$HOME/h.out 2>&1 < /dev/null & echo \$! > /tmp/h.pid'" > /dev/null
  local t=0
  until [ "$(handing "$(walk_file)")" = asked ]; do [ $t -lt 300 ] || fail "h: never handed"; t=$((t + 3)); sleep 3; done
  line down
  sh "kill -9 \$(cat /tmp/h.pid) $(client_calcifer)"
  say "h: X walks it, this computer gone"
  local was='' at=$SECONDS moved
  t=0
  until x_made seashot; do
    moved=$(x "--json project status" | jq -c '[.board.jobs[]? | [.state, .stage, .detail]]')
    [ "$moved" != "$was" ] && { was=$moved; at=$SECONDS; say "h: $moved"; }
    [ $((SECONDS - at)) -lt 900 ] || fail "h: stopped moving for 15 min"
    t=$((t + 15)); sleep 15
  done
  line up
  t=0
  until collected; do [ $t -lt 900 ] || fail "h: not collected"; t=$((t + 5)); sleep 5; done
  local sha path
  while read -r sha path; do
    case $path in *.mp4) cozy machine ssh "$NAME" -- "cat /home/client/h/$path" < /dev/null > "$OUT/h-seashot.mp4" 2>> "$OUT/ssh.err" ;; esac
  done < <(x_files)
  say "h: the shot came down whole: $(ffprobe -v error -show_entries stream=codec_type,nb_frames -of csv=p=0 "$OUT/h-seashot.mp4" | paste -sd' ')"
}

for part in a k h e; do
  ! has "$part" || "part_$part"
done
say "done: $PARTS"
