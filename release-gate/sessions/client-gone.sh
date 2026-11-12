# client-gone (reports/laptop-wait-audit.md): a run on a pod finishes with the computer that asked gone, and what comes next doesn't wait
# on that computer's link. One rental with SSH. A second user on it is the computer that asks: its own home and Calcifer, adding the
# pod's TensorD over fixtures/client-gone/link.py, a line this session throttles and cuts, with the pod's resolver broken too (as
# hub-outage does) when it needs that computer to reach nothing. fixtures/client-gone/art.py is its local renderer: no GPU, no weights.
# What that computer's page asks of its project, e2e/page.py does there, as that user.
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
# page <verb> <args>: what that computer's page asks of its project (e2e/page.py), of the Calcifer a CLI call there started.
page() { local verb=$1; shift; sh "su client -s /bin/sh -c 'python3 \$HOME/page.py $verb \$HOME/$TAG $*'"; }
line() { sh "echo $* > /tmp/link"; }
holder_pid() { sh 'grep -o "\"pid\":[0-9]*" /home/client/.cozy/calcifer/calcifer.lock.holder | cut -d: -f2'; }
# write <path>: stdin, as the file <path> of the computer that asks.
write() { cozy machine ssh "$NAME" -- "cat > $1; chown client $1" 2>> "$OUT/ssh.err"; }
idle_deadline() { cozy machine show "$NAME" --json | jq -r '.status.idle_deadline_ms // 0'; }

sh 'useradd -m client && install -d /opt/client && command -v python3 && command -v su' > "$OUT/tools.txt" || fail "no second user: $(tail -3 "$OUT/ssh.err")"
cozy machine ssh "$NAME" -- 'cat > /root/link.py' < "$FIX/link.py" 2>> "$OUT/ssh.err" || fail "copy link"
cozy machine ssh "$NAME" -- 'cat > /root/hole.py' < "$FIX/hole.py" 2>> "$OUT/ssh.err" || fail "copy hole"
write /home/client/art.py < "$FIX/art.py" || fail "copy art"
write /home/client/page.py < "$RUN_PAGE" || fail "copy page.py"
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
  # 0.24.0's `cozy new` makes a plain project; rc/0.25.0's makes one with no passphrase only with --unencrypted.
  local plain=''
  c "new --help" > "$OUT/new-help-$tag.txt" 2>&1
  grep -q -- --unencrypted "$OUT/new-help-$tag.txt" && plain=--unencrypted
  c "new /home/client/$tag $plain" > "$OUT/new-$tag.txt" 2>&1 || fail "cozy new: $(tail -3 "$OUT/new-$tag.txt")"
  sh "cp /home/client/art.py /home/client/$tag/art.py; chown client /home/client/$tag/art.py"
  # The first add is refused machine_unauthorized, its hint naming the key: on stderr, which ssh keeps apart.
  c "machine add box 127.0.0.1:$LINK --fingerprint $FINGERPRINT 2>&1" > "$OUT/add-$tag.txt" 2>&1
  key=$(grep -oE 'cozy machine authorize [A-Za-z0-9_-]{43}' "$OUT/add-$tag.txt" | awk '{print $4}' | head -1)
  [ -n "$key" ] || fail "no key to authorize: $(tail -3 "$OUT/add-$tag.txt")"
  sh "cozy machine authorize $key" > "$OUT/authorize-$tag.txt" 2>&1 || fail "authorize"
  c "machine add box 127.0.0.1:$LINK --fingerprint $FINGERPRINT 2>&1" > "$OUT/add-$tag.txt" 2>&1 || fail "add: $(tail -3 "$OUT/add-$tag.txt")"
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
  [ -n "$run" ] || fail "$part: run never had $when: $(c 'run ls --limit 3' | cut -c1-200); its cozy run said: $(sh "tail -c 600 /home/client/$part.out")"
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
    record=$(c "--json run watch $run --once")
    jq -e '.status == "completed" and (.output[0].url // "") != ""' <<<"$record" > /dev/null 2>&1 && break
    t=$((t + 5)); sleep 5
  done
  echo "$record" > "$OUT/$part-$TAG.json"
  jq -e '.status == "completed" and (.output[0].url // "") != ""' <<<"$record" > /dev/null || { wrong "$part: back again, run $run did not land: $(head -c 400 <<<"$record")"; return; }
  local path sha want
  # Downloaded by its digest, as any client fetches what it asks for.
  c "run watch $run --once -o /tmp/landed-$part-$TAG" > /dev/null
  path=$(sh "ls /tmp/landed-$part-$TAG/* | head -1") want=$(jq -r '.output[0].sha256' <<<"$record")
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
  jq -n --arg n "$RANDOM" --arg m "$RANDOM" '[
    {name: "still", tool: "./art.py/make", kind: "image", fields: {side: 4000, seconds: 2, prompt: "noise \($n)"}},
    {name: "shot", tool: "./art.py/make", kind: "image", fields: {prompt: "reads <still> \($m)"}}
  ]' | write /home/client/b.json
  page boxes /home/client/b.json > "$OUT/b-$TAG-boxes.txt" 2>&1 || wrong "b: its boxes: $(tail -3 "$OUT/b-$TAG-boxes.txt")"
  line "up 1000000"
  page render --on box --queue still shot > "$OUT/b-$TAG.txt" 2>&1 || wrong "b: render: $(tail -3 "$OUT/b-$TAG.txt")"
  local t=0 got still shot
  while [ $t -lt 900 ]; do
    got=$(page asset still shot)
    jq -se 'length == 2 and all(.[]; .versions != [] and (.asset.making // []) == [])' <<<"$got" > /dev/null 2>&1 && break
    t=$((t + 3)); sleep 3
  done
  echo "$got" > "$OUT/b-$TAG.assets.json"
  still=$(jq -sr '.[0].versions[-1].runs[-1] // empty' <<<"$got") shot=$(jq -sr '.[1].versions[-1].runs[-1] // empty' <<<"$got")
  [ -n "$still" ] && [ -n "$shot" ] || wrong "b: no versions: $(page board | jq -c '[.jobs[]? | {asset, state, why}]' | cut -c1-300)"
  local sha path began landed t2=0
  sha=$(jq -sr '.[0].versions[-1].files.image.sha256' <<<"$got") path=$(jq -sr '.[0].versions[-1].files.image.path' <<<"$got")
  until sh "test -f /home/client/$TAG/$path" > /dev/null || [ $t2 -ge 600 ]; do t2=$((t2 + 3)); sleep 3; done
  [ "sha256:$(sh "sha256sum /home/client/$TAG/$path" | cut -c1-64)" = "$sha" ] || wrong "b: $path is not its hash"
  landed=$(sh "stat -c %Y /home/client/$TAG/$path")000
  began=$(c "--json run watch $shot --once" | jq -r '.created_at_ms // 0')
  c "--json run watch $shot --once" > "$OUT/b-$TAG-shot.json"
  c "--json run watch $still --once" > "$OUT/b-$TAG-still.json"
  say "$TAG b: still is run $still, shot run $shot: shot was sent $(( (landed - began) / 1000 )) s before still landed; what it says of its input: $(jq -r '[.logs[]? | select(.level == "inputs") | .text] | first // "nothing"' "$OUT/b-$TAG-shot.json")"
  [ $((landed - began)) -ge 5000 ] || wrong "b: shot was sent only $(( (landed - began) / 1000 )) s before still landed: it waited for this computer"
  line up
}

part_c() {
  say "$TAG c: a render asked with no network here"
  jq -n --arg n "$RANDOM" --arg m "$RANDOM" '[
    {name: "first", tool: "./art.py/make", kind: "image", fields: {prompt: "noise \($n)"}},
    {name: "second", tool: "./art.py/make", kind: "image", fields: {prompt: "more noise \($m)"}}
  ]' | write /home/client/c.json
  page boxes /home/client/c.json > "$OUT/c-$TAG-boxes.txt" 2>&1 || wrong "c: its boxes: $(tail -3 "$OUT/c-$TAG-boxes.txt")"
  line down
  sh 'printf "nameserver 127.0.0.9\n" > /etc/resolv.conf; ! getent hosts tensorhub.com' || fail "c: the resolver did not break"
  page render --on box --queue first second > "$OUT/c-$TAG.txt" 2>&1 || say "$TAG c: asked: $(tail -2 "$OUT/c-$TAG.txt" | cut -c1-200)"
  sleep 75
  page board > "$OUT/c-$TAG-away.json" 2>&1
  local failed
  failed=$(jq '[.jobs[]? | select(.state == "failed" or .state == "not_sent")] | length' "$OUT/c-$TAG-away.json" 2> /dev/null)
  say "$TAG c: after 75 s with no network: ${failed:-?} jobs failed or not sent; $(jq -c '[.jobs[]? | {asset, state, why}]' "$OUT/c-$TAG-away.json" 2> /dev/null | cut -c1-300)"
  sh 'cat /root/resolv.conf.gone > /etc/resolv.conf'
  line up
  local t=0 got
  while [ $t -lt 300 ]; do
    got=$(page asset first second)
    jq -se 'length == 2 and all(.[]; .versions != [])' <<<"$got" > /dev/null 2>&1 && break
    t=$((t + 5)); sleep 5
  done
  echo "$got" > "$OUT/c-$TAG.assets.json"
  jq -se 'length == 2 and all(.[]; .versions != [])' <<<"$got" > /dev/null 2>&1 ||
    { wrong "c: the network back, first and second weren't made: $(page board | jq -c '[.jobs[]? | {asset, state, why}]' | cut -c1-300)"; return; }
  page board > "$OUT/c-$TAG-board.json"
  [ "$(jq '[.jobs[]? | select(.state == "failed")] | length' "$OUT/c-$TAG-board.json")" = 0 ] || wrong "c: a job failed: $(jq -c '[.jobs[] | select(.state == "failed") | .error.message]' "$OUT/c-$TAG-board.json" | cut -c1-300)"
  say "$TAG c: the network back, both made $t s later, nothing failed"
}

part_v() {
  say "$TAG v: a cut and a frame of a clip still on its way"
  jq -n --arg n "$RANDOM" '[
    {name: "clip", tool: "./art.py/film", kind: "video", fields: {seconds: 10, prompt: "noise \($n)"}},
    {name: "edit", tool: "cozy/edit", fields: {tracks: [{clips: ["<clip>"]}]}},
    {name: "aframe", tool: "cozy/frame", kind: "image", fields: {video: "<clip>", at: 1}}
  ]' | write /home/client/v.json
  page boxes /home/client/v.json > "$OUT/v-$TAG-boxes.txt" 2>&1 || wrong "v: its boxes: $(tail -3 "$OUT/v-$TAG-boxes.txt")"
  line "up 1000000"
  page render --on box --queue all > "$OUT/v-$TAG.txt" 2>&1 || wrong "v: render: $(tail -3 "$OUT/v-$TAG.txt" | cut -c1-200)"
  local t=0 got
  while [ $t -lt 900 ]; do
    got=$(page asset clip edit aframe)
    jq -se 'length == 3 and all(.[]; .versions != [])' <<<"$got" > /dev/null 2>&1 && break
    # Its own failure ends the wait: it won't be made.
    [ "$(page board | jq '[.jobs[]? | select(.state == "failed")] | length')" = 0 ] || break
    t=$((t + 6)); sleep 3
  done
  line up
  echo "$got" > "$OUT/v-$TAG.assets.json"
  jq -se 'length == 3 and all(.[]; .versions != [])' <<<"$got" > /dev/null 2>&1 ||
    { wrong "v: not all made in $t s: $(page board | jq -c '[.jobs[]? | {asset, state, why}]' | cut -c1-300)"; return; }
  local clip path
  clip=$(jq -sr '.[0].versions[-1].files.video.sha256' <<<"$got") path=$(jq -sr '.[0].versions[-1].files.video.path' <<<"$got")
  [ "sha256:$(sh "sha256sum /home/client/$TAG/$path" | cut -c1-64)" = "$clip" ] || wrong "v: the clip is not its hash"
  sh "ffprobe -v error -show_entries stream=codec_type,nb_frames -of csv=p=0 /home/client/$TAG/edit.mp4" > "$OUT/v-$TAG-edit.txt" 2>&1
  grep -q video "$OUT/v-$TAG-edit.txt" || wrong "v: no film at edit.mp4: $(head -c 200 "$OUT/v-$TAG-edit.txt")"
  say "$TAG v: all made $t s after asking at 1 MB/s; the clip whole; the film: $(paste -sd' ' "$OUT/v-$TAG-edit.txt")"
}

part_m() {
  say "$TAG m: ffmpeg this computer doesn't have, its download going nowhere"
  jq -n --arg n "$RANDOM" --arg m "$RANDOM" '[
    {name: "firstm", tool: "./art.py/make", kind: "image", fields: {prompt: "noise \($n)"}},
    {name: "secondm", tool: "./art.py/make", kind: "image", fields: {prompt: "reads <firstm> \($m)"}}
  ]' | write /home/client/m.json
  page boxes /home/client/m.json > "$OUT/m-$TAG-boxes.txt" 2>&1 || wrong "m: its boxes: $(tail -3 "$OUT/m-$TAG-boxes.txt")"
  # A Calcifer that finds no ffmpeg on its path (started by a CLI with none on its own), and a host that takes its connect and never answers.
  sh "kill -9 $(holder_pid)"
  sh 'nohup python3 /root/hole.py > /dev/null 2>&1 < /dev/null & echo $! > /tmp/hole.pid; echo "127.0.0.1 ffmpeg.martin-riedl.de" >> /etc/hosts'
  sh "su client -s /bin/sh -c 'cd \$HOME/$TAG; PATH=/opt/client /opt/client/cozy status'" > "$OUT/m-$TAG-status.txt" 2>&1
  page render --on box --queue firstm secondm > "$OUT/m-$TAG.txt" 2>&1
  local t=0 got='' asked=$SECONDS
  while [ $t -lt 150 ]; do
    got=$(page asset secondm)
    jq -e '.versions != []' <<<"$got" > /dev/null 2>&1 && break
    t=$((SECONDS - asked)); sleep 3
  done
  echo "$got" > "$OUT/m-$TAG.assets.json"
  sh 'cat /root/hosts.gone > /etc/hosts; kill $(cat /tmp/hole.pid)'
  if jq -e '.versions != []' <<<"$got" > /dev/null 2>&1 && [ "$t" -lt 60 ]; then
    say "$TAG m: secondm made $t s after asking, its ffmpeg still to come"
  else
    wrong "m: secondm not made in 60 s (made: $(jq -c '.versions // [] | length' <<<"$got"), after $t s): the walk waited on this computer's download"
  fi
  # The next part's Calcifer has its path back.
  sh "kill -9 $(holder_pid)"
  c "status" > /dev/null 2>&1
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
  record=$(c "--json run watch $run --once")
  echo "$record" > "$OUT/s-$TAG-woke.json"
  local warned; warned=$(jq -r '[.warnings[]? | select(.code == "no_progress") | .message] | first // ""' <<<"$record")
  line up
  [ -z "$warned" ] && say "$TAG s: woken, no false alarm on run $run" || wrong "s: woken, it says: $warned"
  t=0
  while [ $t -lt 120 ]; do
    record=$(c "--json run watch $run --once")
    jq -e '.status == "completed"' <<<"$record" > /dev/null 2>&1 && break
    t=$((t + 5)); sleep 5
  done
  jq -e '.status == "completed"' <<<"$record" > /dev/null || wrong "s: run $run did not settle: $(head -c 300 <<<"$record")"
}

part_l() {
  say "$TAG l: a file that never comes"
  jq -n --arg n "$RANDOM" '[{name: "lost", tool: "./art.py/make", kind: "image", fields: {side: 4000, prompt: "noise \($n)"}}]' | write /home/client/l.json
  page boxes /home/client/l.json > "$OUT/l-$TAG-boxes.txt" 2>&1 || wrong "l: its boxes: $(tail -3 "$OUT/l-$TAG-boxes.txt")"
  line "up 200000"
  page render --on box --queue lost > "$OUT/l-$TAG.txt" 2>&1 || wrong "l: render: $(tail -3 "$OUT/l-$TAG.txt" | cut -c1-200)"
  local t=0 got
  while [ $t -lt 300 ]; do
    got=$(page asset lost)
    jq -e '.versions != []' <<<"$got" > /dev/null 2>&1 && break
    t=$((t + 3)); sleep 3
  done
  jq -e '.versions != []' <<<"$got" > /dev/null 2>&1 || { wrong "l: it was never listed ahead of its file"; line up; return; }
  say "$TAG l: listed ahead of its file; its machine removed"
  c "machine end box -y --force" > "$OUT/l-$TAG-end.txt" 2>&1
  line up
  local board failed=''
  t=0
  while [ $t -lt 120 ]; do
    board=$(page board)
    failed=$(jq -r '[.jobs[]? | select(.asset == "lost" and .state == "failed") | .error.message] | first // ""' <<<"$board")
    [ -n "$failed" ] && break
    t=$((t + 3)); sleep 3
  done
  echo "$board" > "$OUT/l-$TAG-board.json"
  got=$(page asset lost); echo "$got" > "$OUT/l-$TAG.assets.json"
  [ -n "$failed" ] || wrong "l: its job never failed: $(jq -c '[.jobs[]? | {asset, state, why}]' <<<"$board" | cut -c1-300)"
  [ "$(jq '.versions | length' <<<"$got")" = 0 ] || wrong "l: a version with no file stands"
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
