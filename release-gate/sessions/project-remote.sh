# project-remote (xs-143): a project on a second computer, through `cozy project push` and `pull` at tensorhub.com. No rental.
#   First computer: this runner. A project with a local image, two plates at a model's 1344x768 and bulk references. A
#     push whose Calcifer is killed mid-way sends only the rest, and another moves nothing.
#   Second computer: a second user here (its own home and Calcifer, none of the first's paths), holding only project.yaml
#     and project.lock. Signed out it is refused, and the Hub shows anonymous callers nothing. Signed in with the runner's
#     key, a pull killed mid-way goes on from the bytes it had: nothing missing, real files, the lock unchanged. On one CPU
#     (the first has them all), an edit that joins with no encode is the same file, and one that encodes its edges has
#     the same duration and frames, each with an SSIM of at least 0.99.
# Its commit is deleted after; the Hub's GC takes its bytes.
. "$(dirname "$0")/lib.sh"
command -v ffmpeg > /dev/null || sudo apt-get install -y -qq ffmpeg > /dev/null 2>&1 || fail "no ffmpeg"
MODEL=$(cozy --json whoami | jq -r .account)/project-assets
COMMIT=''
tidy() { [ -z "$COMMIT" ] || cozy model delete "$MODEL@$COMMIT" -y >> "$OUT/delete.txt" 2>&1 || say "could not delete $MODEL@$COMMIT"; }
trap 'tidy; finish' EXIT
J() { jq -e "$@" > /dev/null; }
sha() { sha256sum "$1" | cut -c1-64; }
pid_of() { sudo cat "$1/.cozy/calcifer/calcifer.lock.holder" | jq -r .pid; }

say "a project on the first computer"
P=$HOME/film F=$HOME/art
cozy project new "$P" > "$OUT/new.txt" 2>&1 || fail "project new: $(tail -3 "$OUT/new.txt")"
mkdir -p "$F" "$P/plates" "$P/refs"
plate() {
  ffmpeg -v error -y -f lavfi -i "$2=size=1344x768:rate=24:duration=6" -f lavfi -i "sine=frequency=$3:sample_rate=32000:duration=6" \
    -c:v libx264 -preset veryfast -b:v 900k -g 24 -keyint_min 24 -sc_threshold 0 -pix_fmt yuv420p -c:a aac -b:a 128k -ac 2 -ar 32000 -shortest -metadata comment="$RANDOM$RANDOM" "$1"
}
plate "$P/plates/a.mp4" testsrc2 440 || fail "plate"
plate "$P/plates/b.mp4" testsrc 550 || fail "plate"
ffmpeg -v error -y -f lavfi -i "color=c=teal:size=832x480" -frames:v 1 -metadata comment="$RANDOM$RANDOM" "$F/coast.png" || fail "image"
{
  cat <<'YAML'
entities:
  - name: coast
    kind: environment
    description: |
      A grey coast.
    image: ~/art/coast.png
references:
YAML
  # Bulk that moves for seconds, so a kill lands mid-way; never seen before, so it moves at all.
  for i in $(seq 1 12); do
    head -c 32M /dev/urandom > "$P/refs/r$i.wav"
    printf '  - name: r%s\n    audio: refs/r%s.wav\n' "$i" "$i"
  done
  cat <<'YAML'
shots:
  - name: a
    description: |
      Test pattern a.
    duration_s: 6
    video: plates/a.mp4
  - name: b
    description: |
      Test pattern b.
    duration_s: 6
    video: plates/b.mp4
edit:
  - {shot: a, start: 0, end: 120}
  - {shot: b, start: 24, end: 120}
YAML
} > "$P/project.yaml" || fail "project.yaml"
step render project -p "$P" render all --on local || fail "render all: $(tail -5 "$OUT/render.txt")"
[ "$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$P/edit.mp4")" = 1344,768 ] || fail "the edit is not its takes' size"
CUT=$(sha "$P/edit.mp4") TOTAL=$(cat "$F/coast.png" "$P"/plates/* "$P"/refs/* | wc -c)
say "edit.mp4 $CUT; $TOTAL bytes to move"
! grep -qE "$HOME|\"original\"|\"method\"" "$P/project.lock" || fail "project.lock names this computer: $(grep -nE "$HOME|\"original\"|\"method\"" "$P/project.lock" | head -3)"

say "push, its Calcifer killed once the first file landed"
cozy project -p "$P" push > "$OUT/push-killed.txt" 2>&1 & PUSH=$!
until grep -q '^sent assets/' "$OUT/push-killed.txt" || ! kill -0 "$PUSH" 2> /dev/null; do sleep 0.05; done
kill -0 "$PUSH" 2> /dev/null || fail "push ended before a file landed: $(tail -3 "$OUT/push-killed.txt")"
kill -9 "$(pid_of "$HOME")"; wait "$PUSH"; code=$?
[ "$code" != 0 ] || fail "the push finished before its Calcifer was killed: make the bulk bigger"
say "killed: $(grep -c '^sent assets/' "$OUT/push-killed.txt") files had landed; the CLI said $(tail -1 "$OUT/push-killed.txt" | cut -c1-120)"
cozy --json project -p "$P" push > "$OUT/push-2.json" 2> "$OUT/push-2.err" || fail "push again: $(tail -3 "$OUT/push-2.err")"
COMMIT=$(jq -r .commit "$OUT/push-2.json"); say "push again: $(jq -c . "$OUT/push-2.json")"
J --argjson t "$TOTAL" '.files == 15 and .moved > 0 and .moved < 15 and .bytes > 0 and .bytes < $t' "$OUT/push-2.json" || fail "the second push did not send only the rest"
cozy --json project -p "$P" push > "$OUT/push-3.json" 2> "$OUT/push-3.err" || fail "third push: $(tail -3 "$OUT/push-3.err")"
say "push a third time: $(jq -c . "$OUT/push-3.json")"
J --arg c "$COMMIT" '.files == 15 and .moved == 0 and .bytes == 0 and .commit == $c' "$OUT/push-3.json" || fail "the third push moved something, or made another commit"
step model-info model info "$MODEL" || say "model info: $(tail -3 "$OUT/model-info.txt")"
step model-ls model ls "$MODEL" || say "model ls: $(tail -3 "$OUT/model-ls.txt")"

say "a second computer, holding only project.yaml and project.lock"
U=cozyfilm H=/home/$U C=/home/$U/film
sudo useradd -m "$U" && sudo install -D -m 755 "$(command -v cozy)" "/opt/$U/cozy" || fail "no second user"
second() { sudo -u "$U" -H "/opt/$U/cozy" "$@" < /dev/null; }
sudo install -d -o "$U" -m 700 "$C" && sudo install -o "$U" -m 644 "$P/project.yaml" "$P/project.lock" "$C/" || fail "no copy"
code=0; second project -p "$C" pull > "$OUT/pull-signed-out.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'not_logged_in\|not signed in' "$OUT/pull-signed-out.txt"; } || fail "signed out, pull did not say so (exit $code): $(tail -3 "$OUT/pull-signed-out.txt")"
SHA=$(sha "$P/plates/a.mp4")
ANON=$(curl -sS -o /dev/null -w '%{http_code}' "https://tensorhub.com/blobs/${SHA:0:2}/${SHA:2:2}/$SHA") ; say "anonymous read of an asset: HTTP $ANON"
[ "$ANON" = 404 ] || fail "an asset is readable anonymously: $ANON"
second project -p "$C" status > "$OUT/status-bare.txt" 2>&1 || true
grep -q 'next: cozy project pull' "$OUT/status-bare.txt" || fail "a clone's status does not say to pull: $(tail -5 "$OUT/status-bare.txt")"
sudo install -o "$U" -m 600 "$(find ~/.cozy/keys -name 'device-*.json' ! -name '*.bearer.json')" "$H/key.json" || fail "no key"
second login --key "$H/key.json" > "$OUT/login-second.txt" 2>&1; code=$?; sudo rm -f "$H/key.json"
[ "$code" = 0 ] || fail "second login: $(tail -3 "$OUT/login-second.txt")"

say "pull, it and its Calcifer killed mid-way"
sudo -u "$U" -H sh -c "echo \$\$ > '$H/pull.pid'; exec '/opt/$U/cozy' project -p '$C' pull" > "$OUT/pull-killed.txt" 2>&1 < /dev/null & PULL=$!
parts() { sudo find "$C/assets" -name '.*.part' -size +1M 2> /dev/null | head -1; }
until [ -n "$(parts)" ] || ! kill -0 "$PULL" 2> /dev/null; do sleep 0.05; done
[ -n "$(parts)" ] || fail "pull ended before any file part: $(tail -3 "$OUT/pull-killed.txt")"
sudo kill -9 "$(sudo cat "$H/pull.pid")" "$(pid_of "$H")"; wait "$PULL"; code=$?
[ "$code" != 0 ] || fail "the pull finished before it was killed: make the bulk bigger"
sudo find "$C/assets" -type f -printf '%f %s\n' | sort > "$OUT/after-kill.txt"
HELD=$(awk '{ n += $2 } END { print n + 0 }' "$OUT/after-kill.txt")
say "killed: $HELD of $TOTAL bytes held, $(grep -c part "$OUT/after-kill.txt") partial files"
second --json project -p "$C" pull > "$OUT/pull-2.json" 2> "$OUT/pull-2.err" || fail "pull again: $(tail -3 "$OUT/pull-2.err")"
say "pull again: $(jq -c . "$OUT/pull-2.json")"
J --argjson held "$HELD" --argjson t "$TOTAL" '.files == 15 and .moved > 0 and .bytes > 0 and .bytes == $t - $held' "$OUT/pull-2.json" \
  || fail "the second pull did not fetch exactly what the first left ($TOTAL - $HELD)"
second --json project -p "$C" pull > "$OUT/pull-3.json" 2> "$OUT/pull-3.err" || fail "third pull: $(tail -3 "$OUT/pull-3.err")"
J '.files == 15 and .moved == 0 and .bytes == 0' "$OUT/pull-3.json" || fail "a third pull moved something: $(jq -c . "$OUT/pull-3.json")"

say "the second computer has it all"
sudo find "$C/assets" -mindepth 1 ! -type f -printf '%p %y\n' > "$OUT/not-real.txt"; [ ! -s "$OUT/not-real.txt" ] || fail "not real files: $(head -3 "$OUT/not-real.txt")"
[ -z "$(sudo find "$C/assets" -name '.*')" ] || fail "parts left in assets/: $(sudo find "$C/assets" -name '.*' | head -3)"
sudo sh -c "cd '$C/assets' && for f in *; do [ \"\$(sha256sum \"\$f\" | cut -c1-64)\" = \"\${f%%.*}\" ] || echo \"\$f\"; done" > "$OUT/wrong.txt"
[ ! -s "$OUT/wrong.txt" ] || fail "not their sha256: $(head -3 "$OUT/wrong.txt")"
second --json project -p "$C" status > "$OUT/status-pulled.json" 2> "$OUT/status-pulled.err" || fail "status: $(tail -3 "$OUT/status-pulled.err")"
J '(.missing | length == 0) and (.stale | length == 0) and (.warnings | length == 0) and .absent == 0' "$OUT/status-pulled.json" \
  || fail "status after pull: $(jq -c '{missing, stale, warnings, absent}' "$OUT/status-pulled.json")"
sudo cmp "$C/project.lock" "$P/project.lock" || fail "reading the clone changed project.lock"

say "edit.mp4 cut again on one CPU of the runner's $(nproc)"
sudo kill "$(pid_of "$H")"; for _ in $(seq 1 50); do sudo kill -0 "$(pid_of "$H")" 2> /dev/null || break; sleep 0.1; done
sudo -u "$U" -H taskset -c 0 "/opt/$U/cozy" --json whoami < /dev/null > "$OUT/whoami-one-cpu.json" 2>&1 || fail "no Calcifer on one CPU"
say "its Calcifer may use CPUs $(sudo taskset -pc "$(pid_of "$H")" | sed 's/.*: //')"
second project -p "$C" render all --on local > "$OUT/recut.txt" 2>&1 || fail "cut again: $(tail -5 "$OUT/recut.txt")"
AGAIN=$(sudo sha256sum "$C/edit.mp4" | cut -c1-64); say "edit.mp4 with no encode: $AGAIN, the first computer's $CUT"
[ "$AGAIN" = "$CUT" ] || fail "an edit with no encode is another file on the second computer"

say "an edit that encodes its edges: the same film, not the same bytes"
edges='s/{shot: a, start: 0, end: 120}/{shot: a, start: 7, end: 131}/; s/{shot: b, start: 24, end: 120}/{shot: b, start: 13, end: 140}/'
sed -i "$edges" "$P/project.yaml"; sudo sed -i "$edges" "$C/project.yaml"
step render-edges project -p "$P" render all --on local || fail "render edges: $(tail -5 "$OUT/render-edges.txt")"
second project -p "$C" render all --on local > "$OUT/recut-edges.txt" 2>&1 || fail "cut edges: $(tail -5 "$OUT/recut-edges.txt")"
sudo cp "$C/edit.mp4" "$OUT/edges-second.mp4" && sudo chown "$(id -u)" "$OUT/edges-second.mp4"
shape() { ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames:format=duration -of csv=p=0 "$1" | paste -sd' '; }
[ "$(shape "$P/edit.mp4")" = "$(shape "$OUT/edges-second.mp4")" ] || fail "frames and duration: $(shape "$P/edit.mp4") and $(shape "$OUT/edges-second.mp4")"
ffmpeg -v error -i "$P/edit.mp4" -i "$OUT/edges-second.mp4" -lavfi "ssim=stats_file=$OUT/ssim.log" -f null - || fail "no SSIM"
LOW=$(grep -oE 'All:[0-9.]+' "$OUT/ssim.log" | cut -d: -f2 | sort -n | head -1)
say "$(shape "$P/edit.mp4") on both; the same bytes: $([ "$(sha "$P/edit.mp4")" = "$(sha "$OUT/edges-second.mp4")" ] && echo yes || echo no); lowest per-frame SSIM $LOW"
awk -v s="$LOW" 'BEGIN { exit !(s >= 0.99) }' || fail "per-frame SSIM $LOW is under 0.99"
rm -f "$OUT/edges-second.mp4"
second --json project -p "$C" push > "$OUT/push-second.json" 2> "$OUT/push-second.err" || fail "push from the second computer: $(tail -3 "$OUT/push-second.err")"
J '.files == 15 and .moved == 0' "$OUT/push-second.json" || fail "the second computer's push moved something: $(jq -c . "$OUT/push-second.json")"

say "a file at its path, not what the lock names, goes out of date as ever"
sudo -u "$U" mkdir -p "$H/art" && ffmpeg -v error -y -f lavfi -i "color=c=maroon:size=832x480" -frames:v 1 "$OUT/other.png" && sudo install -o "$U" "$OUT/other.png" "$H/art/coast.png" || fail "no other file"
second --json project -p "$C" status > "$OUT/status-other.json" 2> "$OUT/status-other.err" || fail "status: $(tail -3 "$OUT/status-other.err")"
J '.nodes[] | select(.name == "coast") | .path | test("^assets/")' "$OUT/status-other.json" || fail "no coast: $(head -c 300 "$OUT/status-other.json")"
[ "$(jq -r '.nodes[] | select(.name == "coast") | .assets.image.sha256' "$OUT/status-other.json")" = "sha256:$(sha "$OUT/other.png")" ] \
  || fail "coast does not read the file now at its path: $(jq -c '.nodes[] | select(.name == "coast")' "$OUT/status-other.json")"

say "PROVEN"
