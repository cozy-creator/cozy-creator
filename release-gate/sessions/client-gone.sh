# client-gone (reports/laptop-wait-audit.md): a run on a pod finishes with the computer that asked gone, and what comes next doesn't wait
# on that computer's link. One rental with SSH. A second user on it is the computer that asks: its own home and Calcifer, adding the
# pod's TensorD over fixtures/client-gone/link.py, a line this session throttles and cuts, with the pod's resolver broken too (as
# hub-outage does) when it needs that computer to reach nothing. fixtures/client-gone/art.py is its local renderer: no GPU, no weights.
#   a. Cut mid-run: the line down, its CLI and Calcifer killed. The pod counts as busy, and stops counting when the run ends, with nothing
#      connected. The line back and Calcifer started: the run completed once, its output landed whole.
#   g. The same with a real model (SDXL), cut as soon as the machine has the run: it fetches its weights and renders with nobody there.
#   b. A reader over a slow line: `still`, 48 MB of noise, lands at 1 MB/s; `shot`, which reads it, starts on the pod before it has
#      landed, reads it by its hash, and the render ends with the file here, its bytes its hash.
#   c. No network here: a render asked with the line down and no resolver waits, failing nothing, and goes on when they're back.
#   v. A cut and a frame of a clip still on its way: each waits for its bytes, which come once and whole.
#   m. ffmpeg this computer doesn't have, its download going nowhere: the next job is sent all the same.
#   s. This computer asleep 11 min (its Calcifer stopped, the line down) with a run going: no false "no progress" on waking. Long, so asked for by name.
#   l. A file that never comes (its machine removed mid-way): its version goes, and its job fails saying why. Only of the cozy under test.
# params:
#   cozy        the cozy under test, for `machine update --tensord`: a version, a file or URL#sha256= (default: the session's)
#   before      a cozy to run `before_parts` (default "b c m s") on first, without judging them, for what they did before a fix
#   parts       which of a, g, b, c, v, m, s, l run (default all but s)
#   rent_lock   a file to hold while ordering, where several sessions rent from one computer
#   runtime     a Runtime for the pod with `cozy`: a wheel or a version (default: the pod's)
#   seconds     the least time the cap must buy (default 2400)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures/client-gone" && pwd)
[ -f "$FIX/art.py" ] && [ -f "$FIX/link.py" ] && [ -f "$FIX/hole.py" ] || fail "no fixture checkout"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
UNDER=$(param cozy "$(command -v cozy)") BEFORE=$(param before) LOCK=$(param rent_lock) RUNTIME=$(param runtime) LINK=7777
PARTS=,$(param parts a,g,b,c,v,m,l), BEFORE_PARTS=$(param before_parts "b c m s")
has() { [[ $PARTS == *",$1,"* ]]; }
RENT_ARGS=--ssh
quote "$(param seconds 2400)"
[ -z "$LOCK" ] || { exec 9>"$LOCK"; flock 9; }
order
[ -z "$LOCK" ] || { flock -u 9; exec 9>&-; }
ready

# A shell there. ssh waits forever on a connection that goes quiet (seen here: 196 s on a one-word echo), and everything asked this way
# can be asked twice, so a quiet one is asked again.
sh() {
  local tries=0 code
  while :; do
    timeout 120 cozy machine ssh "$NAME" -- "$*" < /dev/null 2>> "$OUT/ssh.err"; code=$?
    [ "$code" = 124 ] && [ "$tries" -lt 3 ] || return "$code"
    tries=$((tries + 1)); say "ssh went quiet: asked again: ${*:0:80}"
  done
}
mend() { sh 'test -f /root/resolv.conf.gone && cat /root/resolv.conf.gone > /etc/resolv.conf; test -f /root/hosts.gone && cat /root/hosts.gone > /etc/hosts; echo up > /tmp/link'; }
trap 'mend; finish' EXIT
# c <cozy args>: as the computer that asks, in its project.
c() { sh "su client -s /bin/sh -c 'cd \$HOME/\$(cat /tmp/film) 2>/dev/null; /opt/client/cozy $*'"; }
line() { sh "echo $* > /tmp/link"; }
holder_pid() { sh 'grep -o "\"pid\":[0-9]*" /home/client/.cozy/calcifer/calcifer.lock.holder | cut -d: -f2'; }
# write <path>: stdin, as the file <path> of the computer that asks.
write() { cozy machine ssh "$NAME" -- "cat > $1; chown client $1" 2>> "$OUT/ssh.err"; }
idle_deadline() { cozy machine show "$NAME" --json | jq -r '.status.idle_deadline_ms // 0'; }

sh 'useradd -m client && install -d /opt/client && command -v python3 && command -v su' > "$OUT/tools.txt" || fail "no second user: $(tail -3 "$OUT/ssh.err")"
cozy machine ssh "$NAME" -- 'cat > /root/link.py' < "$FIX/link.py" 2>> "$OUT/ssh.err" || fail "copy link"
cozy machine ssh "$NAME" -- 'cat > /root/hole.py' < "$FIX/hole.py" 2>> "$OUT/ssh.err" || fail "copy hole"
write /home/client/art.py < "$FIX/art.py" || fail "copy art"
sh 'cp /etc/resolv.conf /root/resolv.conf.gone; cp /etc/hosts /root/hosts.gone'

# Puts the cozy under test on the pod and on the computer that asks, whose machine `box` is the pod over the line.
put() {
  local tag=$1 version key
  step "update-$tag" machine update "$NAME" --tensord "$2" ${RUNTIME:+--runtime "$RUNTIME"} -y || fail "update to $2"
  version=$(sh 'cozy --version'); say "$tag: the pod runs $version"
  sh 'cozy machine show local --json' > "$OUT/local-$tag.json" || fail "cozy machine show local"
  ADDRESS=$(jq -r .address "$OUT/local-$tag.json") FINGERPRINT=$(jq -r .fingerprint "$OUT/local-$tag.json")
  [ -n "$ADDRESS" ] && [ "$ADDRESS" != null ] || fail "local has no address: $(head -c 300 "$OUT/local-$tag.json")"
  # The line, to where the machine now listens.
  sh "test -f /tmp/link.pid && kill \$(cat /tmp/link.pid) && sleep 1; echo up > /tmp/link; nohup python3 /root/link.py $LINK ${ADDRESS%:*} ${ADDRESS##*:} /tmp/link > /tmp/link.log 2>&1 < /dev/null & echo \$! > /tmp/link.pid" > /dev/null
  # What an earlier part left, as if that computer had just been replaced by this cozy.
  sh "p=$(holder_pid); [ -z \"\$p\" ] || kill -9 \$p; rm -rf /home/client/.cozy; true"
  sh 'cp "$(command -v cozy)" /opt/client/cozy && chmod 755 /opt/client/cozy'
  sh "echo $tag > /tmp/film"
  c "project new /home/client/$tag" > "$OUT/new-$tag.txt" 2>&1 || fail "project new: $(tail -3 "$OUT/new-$tag.txt")"
  sh "cp /home/client/art.py /home/client/$tag/art.py; chown client /home/client/$tag/art.py"
  c "machine add box 127.0.0.1:$LINK --fingerprint $FINGERPRINT" > "$OUT/add-$tag.txt" 2>&1
  key=$(grep -oE 'cozy machine authorize [A-Za-z0-9_-]{43}' "$OUT/add-$tag.txt" | awk '{print $4}' | head -1)
  [ -n "$key" ] || fail "no key to authorize: $(tail -3 "$OUT/add-$tag.txt")"
  sh "cozy machine authorize $key" > "$OUT/authorize-$tag.txt" 2>&1 || fail "authorize"
  c "machine add box 127.0.0.1:$LINK --fingerprint $FINGERPRINT" > "$OUT/add-$tag.txt" 2>&1 || fail "add: $(tail -3 "$OUT/add-$tag.txt")"
}

# The parts; `judged` fails on what a fix should have changed, and only says it otherwise.
wrong() { if [ "$JUDGED" = 1 ]; then fail "$*"; else say "$TAG (not judged): $*"; fi; }

# cut_run <part> <when> <seconds it may take> <cozy run args…>: a run sent, then, once its record has <when>, the line down and its CLI and
# Calcifer killed. The pod stays busy until the run ends, with nothing connected; then the line comes back, and it landed, once, whole.
cut_run() {
  local part=$1 when=$2 most=$3; shift 3
  sh "su client -s /bin/sh -c 'cd \$HOME/$TAG; nohup /opt/client/cozy run $* --on box > \$HOME/$part.out 2>&1 < /dev/null & echo \$! > /tmp/$part.pid'" > /dev/null
  local run='' t=0 record busy=0 ended=0 at function=${1##*/}
  while [ -z "$run" ] && [ $t -lt 600 ]; do
    record=$(c "--json run ls --limit 5" | jq -c --arg f "/${function%@*}" '[.runs[] | select(.target | split("@")[0] | endswith($f))][0] // empty')
    jq -e --arg when "$when" '.[$when] and .status == "in_progress"' <<<"$record" > /dev/null 2>&1 && run=$(jq -r .number <<<"$record")
    t=$((t + 3)); sleep 3
  done
  [ -n "$run" ] || fail "$part: run never had $when: $(c 'run ls --limit 3' | cut -c1-200)"
  say "$TAG $part: run $run has $when; cut: the line down, its CLI and Calcifer killed"
  line down
  sh "kill -9 \$(cat /tmp/$part.pid); kill -9 $(holder_pid)"
  t=0
  while [ $t -lt "$most" ]; do
    at=$(idle_deadline)
    case $at in
      0) busy=$((busy + 1)) ;;
      *) ended=$at; break ;;
    esac
    t=$((t + 5)); sleep 5
  done
  [ "$busy" -gt 0 ] || wrong "$part: with its client gone the pod never counted as busy"
  [ "$ended" != 0 ] || wrong "$part: the run never ended with its client gone (no idle deadline in $t s)"
  say "$TAG $part: with nothing connected the pod was busy for $busy looks over $t s, then its idle clock started: it ends at $ended"
  line up
  t=0
  while [ $t -lt 300 ]; do
    record=$(c "--json run show $run --once")
    jq -e '.status == "completed" and (.output[0].path // "") != ""' <<<"$record" > /dev/null 2>&1 && break
    t=$((t + 5)); sleep 5
  done
  echo "$record" > "$OUT/$part-$TAG.json"
  jq -e '.status == "completed" and (.output[0].path // "") != ""' <<<"$record" > /dev/null || { wrong "$part: back again, run $run did not land: $(head -c 400 <<<"$record")"; return; }
  local path sha want
  path=$(jq -r '.output[0].path' <<<"$record") want=$(jq -r '.output[0].sha256' <<<"$record")
  sha=$(sh "sha256sum $path" | cut -c1-64)
  [ "sha256:$sha" = "$want" ] || wrong "$part: its file is $sha, not $want"
  cozy machine ssh "$NAME" -- "cat $path" < /dev/null > "$OUT/$part-$TAG.output" 2>> "$OUT/ssh.err"
  say "$TAG $part: run $run completed once, execution $(jq -r '.execution_ms // 0' <<<"$record") ms, its file whole"
}

part_a() {
  say "$TAG a: a run, its client cut mid-way"
  cut_run a started_at_ms 300 ./art.py/make seconds:=75 side:=256 prompt=gone-$RANDOM
}

# The same with a real model, cut as soon as the machine has it: it downloads its weights and renders with nobody there.
part_g() {
  say "$TAG g: SDXL, its client cut as soon as the machine has it"
  cut_run g accepted_at_ms 1200 sdxl/generate prompt=a-lighthouse-on-a-black-rock-at-dusk-with-gulls seed:=$RANDOM
}

part_b() {
  say "$TAG b: a reader, with what it reads on a slow line"
  write "/home/client/$TAG/project.yaml" <<YAML
version: 3
assets:
  still:
    tool: ./art.py/make
    side: 4000
    seconds: 2
    instructions: noise $RANDOM
  shot:
    tool: ./art.py/make
    instructions: reads <still> $RANDOM
YAML
  line "up 1000000"
  c "project render still shot --on box --queue" > "$OUT/b-$TAG.txt" 2>&1 || wrong "b: render: $(tail -3 "$OUT/b-$TAG.txt")"
  local t=0 lock still shot
  while [ $t -lt 900 ]; do
    lock=$(lock_now)
    jq -e '(.assets.still.versions // []) != [] and (.assets.shot.versions // []) != [] and (.pending // {}) == {}' <<<"$lock" > /dev/null 2>&1 && break
    t=$((t + 3)); sleep 3
  done
  echo "$lock" > "$OUT/b-$TAG.lock.json"
  still=$(jq -r '.assets.still.versions[-1].run // empty' <<<"$lock") shot=$(jq -r '.assets.shot.versions[-1].run // empty' <<<"$lock")
  [ -n "$still" ] && [ -n "$shot" ] || wrong "b: no versions: $(c 'project status')"
  local sha path began landed t2=0
  sha=$(jq -r '.assets.still.versions[-1].files.image' <<<"$lock") path=files/${sha#sha256:}.png
  until sh "test -f /home/client/$TAG/$path" > /dev/null || [ $t2 -ge 600 ]; do t2=$((t2 + 3)); sleep 3; done
  [ "sha256:$(sh "sha256sum /home/client/$TAG/$path" | cut -c1-64)" = "$sha" ] || wrong "b: $path is not its hash"
  landed=$(sh "stat -c %Y /home/client/$TAG/$path")000
  began=$(c "--json run show $shot --once" | jq -r '.created_at_ms // 0')
  c "--json run show $shot --once" > "$OUT/b-$TAG-shot.json"
  c "--json run show $still --once" > "$OUT/b-$TAG-still.json"
  say "$TAG b: still is run $still, shot run $shot: shot was sent $(( (landed - began) / 1000 )) s before still landed; what it says of its input: $(jq -r '[.logs[]? | select(.level == "inputs") | .text] | first // "nothing"' "$OUT/b-$TAG-shot.json")"
  [ $((landed - began)) -ge 5000 ] || wrong "b: shot was sent only $(( (landed - began) / 1000 )) s before still landed: it waited for this computer"
  line up
}

part_c() {
  say "$TAG c: a render asked with no network here"
  write "/home/client/$TAG/project.yaml" <<YAML
version: 3
assets:
  first:
    tool: ./art.py/make
    instructions: noise $RANDOM
  second:
    tool: ./art.py/make
    instructions: more noise $RANDOM
YAML
  line down
  sh 'printf "nameserver 127.0.0.9\n" > /etc/resolv.conf; ! getent hosts tensorhub.com' || fail "c: the resolver did not break"
  c "project render first second --on box --queue" > "$OUT/c-$TAG.txt" 2>&1 || say "$TAG c: asked: $(tail -2 "$OUT/c-$TAG.txt" | cut -c1-200)"
  sleep 75
  c "project status" > "$OUT/c-$TAG-away.txt" 2>&1
  local failed
  failed=$(grep -ciE 'failed|cannot|not sent' "$OUT/c-$TAG-away.txt")
  say "$TAG c: after 75 s with no network: $failed lines of failure; $(head -4 "$OUT/c-$TAG-away.txt" | cut -c1-120 | paste -sd'|')"
  sh 'cat /root/resolv.conf.gone > /etc/resolv.conf'
  line up
  local t=0 lock
  while [ $t -lt 300 ]; do
    lock=$(lock_now)
    jq -e '(.assets.first.versions // []) != [] and (.assets.second.versions // []) != []' <<<"$lock" > /dev/null 2>&1 && break
    t=$((t + 5)); sleep 5
  done
  echo "$lock" > "$OUT/c-$TAG.lock.json"
  jq -e '(.assets.first.versions // []) != [] and (.assets.second.versions // []) != []' <<<"$lock" > /dev/null 2>&1 ||
    { wrong "c: the network back, first and second weren't made: $(c 'project status' | tail -6 | cut -c1-200 | paste -sd'|')"; return; }
  c "--json --full project status" > "$OUT/c-$TAG-board.json"
  [ "$(jq '[.board.jobs[]? | select(.state == "failed")] | length' "$OUT/c-$TAG-board.json")" = 0 ] || wrong "c: a job failed: $(jq -c '[.board.jobs[] | select(.state == "failed") | .error.message]' "$OUT/c-$TAG-board.json" | cut -c1-300)"
  say "$TAG c: the network back, both made $t s later, nothing failed"
}

# newest <lock> <asset>: the sha256 of its newest version's main file.
# project.lock as JSON, which is how this reads it: on disk it's YAML.
lock_now() { sh "cat /home/client/$TAG/project.lock" | uv run -q --no-project --with pyyaml python -c 'import json, sys, yaml; json.dump(yaml.safe_load(sys.stdin), sys.stdout)'; }
newest() { jq -r --arg a "$2" '.assets[$a].versions[-1].files | (.video // .image // empty)' <<<"$1"; }

part_v() {
  say "$TAG v: a cut and a frame of a clip still on its way"
  write "/home/client/$TAG/project.yaml" <<YAML
version: 3
assets:
  clip:
    tool: ./art.py/film
    seconds: 10
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
  line "up 1000000"
  c "project render all --on box --queue" > "$OUT/v-$TAG.txt" 2>&1 || wrong "v: render: $(tail -3 "$OUT/v-$TAG.txt" | cut -c1-200)"
  local t=0 lock
  while [ $t -lt 900 ]; do
    lock=$(lock_now)
    jq -e '[.assets.clip, .assets.edit, .assets.aframe | (.versions // []) | length > 0] | all' <<<"$lock" > /dev/null 2>&1 && break
    # Its own failure ends the wait: it won't be made.
    [ "$(c "--json --full project status" | jq '[.board.jobs[]? | select(.state == "failed")] | length')" = 0 ] || break
    t=$((t + 6)); sleep 3
  done
  line up
  echo "$lock" > "$OUT/v-$TAG.lock.json"
  jq -e '[.assets.clip, .assets.edit, .assets.aframe | (.versions // []) | length > 0] | all' <<<"$lock" > /dev/null 2>&1 ||
    { wrong "v: not all made in $t s: $(c 'project status' | cut -c1-160 | head -5 | paste -sd'|')"; return; }
  local clip path
  clip=$(newest "$lock" clip); path=files/${clip#sha256:}.mp4
  [ "sha256:$(sh "sha256sum /home/client/$TAG/$path" | cut -c1-64)" = "$clip" ] || wrong "v: the clip is not its hash"
  sh "ffprobe -v error -show_entries stream=codec_type,nb_frames -of csv=p=0 /home/client/$TAG/edit.mp4" > "$OUT/v-$TAG-edit.txt" 2>&1
  grep -q video "$OUT/v-$TAG-edit.txt" || wrong "v: no film at edit.mp4: $(head -c 200 "$OUT/v-$TAG-edit.txt")"
  say "$TAG v: all made $t s after asking at 1 MB/s; the clip whole; the film: $(paste -sd' ' "$OUT/v-$TAG-edit.txt")"
}

part_m() {
  say "$TAG m: ffmpeg this computer doesn't have, its download going nowhere"
  write "/home/client/$TAG/project.yaml" <<YAML
version: 3
assets:
  firstm:
    tool: ./art.py/make
    instructions: noise $RANDOM
  secondm:
    tool: ./art.py/make
    instructions: reads <firstm> $RANDOM
YAML
  # A Calcifer that finds no ffmpeg on its path, and a host that takes its connect and never answers.
  sh "kill -9 $(holder_pid)"
  sh 'nohup python3 /root/hole.py > /dev/null 2>&1 < /dev/null & echo $! > /tmp/hole.pid; echo "127.0.0.1 ffmpeg.martin-riedl.de" >> /etc/hosts'
  sh "su client -s /bin/sh -c 'cd \$HOME/$TAG; PATH=/opt/client /opt/client/cozy project render firstm secondm --on box --queue'" > "$OUT/m-$TAG.txt" 2>&1
  local t=0 lock='' asked=$SECONDS
  while [ $t -lt 150 ]; do
    lock=$(lock_now)
    jq -e '(.assets.secondm.versions // []) != []' <<<"$lock" > /dev/null 2>&1 && break
    t=$((SECONDS - asked)); sleep 3
  done
  echo "$lock" > "$OUT/m-$TAG.lock.json"
  sh 'cat /root/hosts.gone > /etc/hosts; kill $(cat /tmp/hole.pid)'
  if jq -e '(.assets.secondm.versions // []) != []' <<<"$lock" > /dev/null 2>&1 && [ "$t" -lt 60 ]; then
    say "$TAG m: secondm made $t s after asking, its ffmpeg still to come"
  else
    wrong "m: secondm not made in 60 s (made: $(jq -c '.assets.secondm.versions // [] | length' <<<"$lock"), after $t s): the walk waited on this computer's download"
  fi
  # The next part's Calcifer has its path back.
  sh "kill -9 $(holder_pid)"
}

part_s() {
  say "$TAG s: this computer asleep for 11 min, a run going"
  sh "su client -s /bin/sh -c 'cd \$HOME/$TAG; nohup /opt/client/cozy run ./art.py/make --on box seconds:=30 side:=256 prompt=asleep-$RANDOM > \$HOME/s.out 2>&1 < /dev/null & echo \$! > /tmp/s.pid'" > /dev/null
  local run='' t=0 record
  while [ -z "$run" ] && [ $t -lt 300 ]; do
    record=$(c "--json run ls --limit 5" | jq -c '[.runs[] | select(.target | split("@")[0] | endswith("/make"))][0] // empty')
    jq -e '.started_at_ms and .status == "in_progress"' <<<"$record" > /dev/null 2>&1 && run=$(jq -r .number <<<"$record")
    t=$((t + 3)); sleep 3
  done
  [ -n "$run" ] || fail "s: the run never began"
  local pid; pid=$(holder_pid)
  line down
  sh "kill -9 \$(cat /tmp/s.pid); kill -STOP $pid"
  say "$TAG s: run $run going, Calcifer $pid stopped, the line down, for 11 min"
  sleep 660
  sh "kill -CONT $pid"
  sleep 75
  record=$(c "--json run show $run --once")
  echo "$record" > "$OUT/s-$TAG-woke.json"
  local warned; warned=$(jq -r '[.warnings[]? | select(.code == "no_progress") | .message] | first // ""' <<<"$record")
  line up
  [ -z "$warned" ] && say "$TAG s: woken, no false alarm on run $run" || wrong "s: woken, it says: $warned"
  t=0
  while [ $t -lt 120 ]; do
    record=$(c "--json run show $run --once")
    jq -e '.status == "completed"' <<<"$record" > /dev/null 2>&1 && break
    t=$((t + 5)); sleep 5
  done
  jq -e '.status == "completed"' <<<"$record" > /dev/null || wrong "s: run $run did not settle: $(head -c 300 <<<"$record")"
}

part_l() {
  say "$TAG l: a file that never comes"
  write "/home/client/$TAG/project.yaml" <<YAML
version: 3
assets:
  lost:
    tool: ./art.py/make
    side: 4000
    instructions: noise $RANDOM
YAML
  line "up 200000"
  c "project render lost --on box --queue" > "$OUT/l-$TAG.txt" 2>&1 || wrong "l: render: $(tail -3 "$OUT/l-$TAG.txt" | cut -c1-200)"
  local t=0 lock
  while [ $t -lt 300 ]; do
    lock=$(lock_now)
    jq -e '(.assets.lost.versions // []) != []' <<<"$lock" > /dev/null 2>&1 && break
    t=$((t + 3)); sleep 3
  done
  jq -e '(.assets.lost.versions // []) != []' <<<"$lock" > /dev/null 2>&1 || { wrong "l: it was never listed ahead of its file"; line up; return; }
  say "$TAG l: listed ahead of its file; its machine removed"
  c "machine end box -y --force" > "$OUT/l-$TAG-end.txt" 2>&1
  line up
  local board failed=''
  t=0
  while [ $t -lt 120 ]; do
    board=$(c "--json --full project status")
    failed=$(jq -r '[.board.jobs[]? | select(.asset == "lost" and .state == "failed") | .error.message] | first // ""' <<<"$board")
    [ -n "$failed" ] && break
    t=$((t + 3)); sleep 3
  done
  echo "$board" > "$OUT/l-$TAG-board.json"
  lock=$(lock_now); echo "$lock" > "$OUT/l-$TAG.lock.json"
  [ -n "$failed" ] || wrong "l: its job never failed: $(c 'project status' | cut -c1-160 | head -4 | paste -sd'|')"
  [ "$(jq '.assets.lost.versions // [] | length' <<<"$lock")" = 0 ] || wrong "l: a version with no file stands"
  say "$TAG l: its version gone, its job failed: ${failed:0:200}"
}

scenario() {
  TAG=$1 JUDGED=$3
  put "$1" "$2"
  for part in $4; do
    ! has "$part" || "part_$part"
  done
}
# What a fix changed, as it was. A cut run went on before them as well, so it's asked once.
[ -z "$BEFORE" ] || [ -z "$BEFORE_PARTS" ] || { parts=$PARTS; PARTS=,$(tr ' ' , <<<"$BEFORE_PARTS"),; scenario before "$BEFORE" 0 "$BEFORE_PARTS"; PARTS=$parts; }
scenario after "$UNDER" 1 "a g b c v m s l"
say "done: $PARTS"
