# project-remote (xs-143): a project on a second computer, through `cozy push` and `pull` at tensorhub.com. No rental.
#   First computer: this runner. A project with an imported image, two plates at a model's 1344x768 and bulk references. A
#     push whose Calcifer is killed mid-way sends only the rest, and another moves nothing.
#   Second computer: a second user here (its own home and Calcifer, none of the first's paths), holding only project.yaml
#     and project.lock. Signed out it is refused, and the Hub shows anonymous callers nothing. Signed in with the runner's
#     key, a pull killed mid-way goes on from the bytes it had: nothing missing, real files, the lock unchanged. On one CPU
#     (the first has them all), an edit that joins with no encode is the same file, and one that encodes its edges has
#     the same duration and frames, each with an SSIM of at least 0.99.
#   A file over the Hub's 5 GiB goes as parts (cl-731), which project.lock lists: a push killed mid-way sends only the parts it
#     had not, and a pull killed mid-way goes on inside the file and comes out its sha256.
#   What stays on a machine (a hash with no file) is neither sent, fetched nor warned of.
#   A file not at the Hub (cl-730): a pull gets everything else, names it in a warning and exits 3, and so does a push from
#     that copy.
# Its commits are deleted after; the Hub's GC takes their bytes.
. "$(dirname "$0")/lib.sh"
command -v ffmpeg > /dev/null || sudo apt-get install -y -qq ffmpeg > /dev/null 2>&1 || fail "no ffmpeg"
MODEL=$(cozy --json whoami | jq -r .account)/project-assets
tidy() { local c; for c in $(jq -r '.commit // empty' "$OUT"/push-*.json 2> /dev/null | sort -u); do cozy model delete "$MODEL@$c" -y >> "$OUT/delete.txt" 2>&1 || say "could not delete $MODEL@$c"; done; }
trap 'tidy; finish' EXIT
J() { jq -e "$@" > /dev/null; }
sha() { sha256sum "$1" | cut -c1-64; }
pid_of() { sudo cat "$1/.cozy/calcifer/calcifer.lock.holder" | jq -r .pid; }
in_project() { local directory=$1; shift; (cd "$directory" && cozy "$@"); }
second_in_project() { local directory=$1; shift; sudo -u "$U" -H sh -c 'cd "$1" && shift && exec "$@"' sh "$directory" "/opt/$U/cozy" "$@" < /dev/null; }

say "a project on the first computer"
P=$HOME/film F=$HOME/art
mkdir -p "$P" && (cd "$P" && cozy --json status) > "$OUT/new.txt" 2>&1 || fail "project initialization: $(tail -3 "$OUT/new.txt")"
mkdir -p "$F" "$P/plates" "$P/refs"
plate() {
  ffmpeg -v error -y -f lavfi -i "$2=size=1344x768:rate=24:duration=6" -f lavfi -i "sine=frequency=$3:sample_rate=32000:duration=6" \
    -c:v libx264 -preset veryfast -b:v 900k -g 24 -keyint_min 24 -sc_threshold 0 -pix_fmt yuv420p -c:a aac -b:a 128k -ac 2 -ar 32000 -shortest -metadata comment="$RANDOM$RANDOM" "$1"
}
# A tone of this run's, so its cuts are new: a cut keeps no comment, and the Hub keeps what earlier runs sent.
plate "$P/plates/a.mp4" testsrc2 "$((200 + RANDOM % 2000))" || fail "plate"
plate "$P/plates/b.mp4" testsrc "$((200 + RANDOM % 2000))" || fail "plate"
ffmpeg -v error -y -f lavfi -i "color=c=teal:size=832x480" -frames:v 1 -metadata comment="$RANDOM$RANDOM" "$F/coast.png" || fail "image"
# hash_of <file>: the file imported, and the prefix that names it.
hash_of() { in_project "$P" --json import "$1" | jq -r '.sha256 | sub("sha256:"; "") | .[:12]' | sed 's/.*/"&"/'; }
{
  cat <<YAML
version: 3
assets:
  coast:
    description: |
      A grey coast.
    approved: $(hash_of "$F/coast.png")
YAML
  # Bulk that moves for seconds, so a kill lands mid-way; never seen before, so it moves at all.
  for i in $(seq 1 12); do
    { printf 'RIFF\0\0\0\0WAVE'; head -c 32M /dev/urandom; } > "$P/refs/r$i.wav"
    printf '  r%s:\n    approved: %s\n' "$i" "$(hash_of "$P/refs/r$i.wav")"
  done
  cat <<YAML
  a:
    approved: $(hash_of "$P/plates/a.mp4")
  b:
    approved: $(hash_of "$P/plates/b.mp4")
  edit:
    tool: cozy/edit
    tracks:
      - clips:
          - {video: <a>, in: 0, out: 120}
          - {video: <b>, in: 24, out: 120}
YAML
} > "$P/project.yaml" || fail "project.yaml"
(cd "$P" && step render render all --on local) || fail "render all: $(tail -5 "$OUT/render.txt")"
[ "$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$P/edit.mp4")" = 1344,768 ] || fail "the edit is not its takes' size"
# The cut and its timeline are files of the project's too: what project.lock lists is what moves.
CUT=$(sha "$P/edit.mp4") TOTAL=$(cat "$F/coast.png" "$P"/plates/* "$P"/refs/* "$P/edit.mp4" "$P/edit.otio" | wc -c) FILES=$(ls "$P/files" | wc -l)
say "edit.mp4 $CUT; $TOTAL bytes to move"
! grep -qE "$HOME|\"original\"|\"method\"" "$P/project.lock" || fail "project.lock names this computer: $(grep -nE "$HOME|\"original\"|\"method\"" "$P/project.lock" | head -3)"

say "push, its Calcifer killed once the first file landed"
in_project "$P" push > "$OUT/push-killed.txt" 2>&1 & PUSH=$!
until grep -q '^sent files/' "$OUT/push-killed.txt" || ! kill -0 "$PUSH" 2> /dev/null; do sleep 0.05; done
kill -0 "$PUSH" 2> /dev/null || fail "push ended before a file landed: $(tail -3 "$OUT/push-killed.txt")"
kill -9 "$(pid_of "$HOME")"; wait "$PUSH"; code=$?
[ "$code" != 0 ] || fail "the push finished before its Calcifer was killed: make the bulk bigger"
say "killed: $(grep -c '^sent files/' "$OUT/push-killed.txt") files had landed; the CLI said $(tail -1 "$OUT/push-killed.txt" | cut -c1-120)"
in_project "$P" --json push > "$OUT/push-2.json" 2> "$OUT/push-2.err" || fail "push again: $(tail -3 "$OUT/push-2.err")"
COMMIT=$(jq -r .commit "$OUT/push-2.json"); say "push again: $(jq -c . "$OUT/push-2.json")"
J --argjson t "$TOTAL" --argjson f "$FILES" '.files == $f and .moved > 0 and .moved < $f and .bytes > 0 and .bytes < $t' "$OUT/push-2.json" || fail "the second push did not send only the rest"
in_project "$P" --json push > "$OUT/push-3.json" 2> "$OUT/push-3.err" || fail "third push: $(tail -3 "$OUT/push-3.err")"
say "push a third time: $(jq -c . "$OUT/push-3.json")"
J --arg c "$COMMIT" --argjson f "$FILES" '.files == $f and .moved == 0 and .bytes == 0 and .commit == $c' "$OUT/push-3.json" || fail "the third push moved something, or made another commit"
model_details "$MODEL" > "$OUT/model-info.txt" || say "model metadata: $(tail -3 "$OUT/model-info.txt")"
step model-ls model ls "$MODEL" || say "model ls: $(tail -3 "$OUT/model-ls.txt")"

say "a second computer, holding only project.yaml and project.lock"
U=cozyfilm H=/home/$U C=/home/$U/film
sudo useradd -m "$U" && sudo install -D -m 755 "$(command -v cozy)" "/opt/$U/cozy" || fail "no second user"
second() { sudo -u "$U" -H "/opt/$U/cozy" "$@" < /dev/null; }
sudo install -d -o "$U" -m 700 "$C" && sudo install -o "$U" -m 644 "$P/project.yaml" "$P/project.lock" "$C/" || fail "no copy"
code=0; second_in_project "$C" pull > "$OUT/pull-signed-out.txt" 2>&1 || code=$?
{ [ "$code" != 0 ] && grep -q 'not_logged_in\|not signed in' "$OUT/pull-signed-out.txt"; } || fail "signed out, pull did not say so (exit $code): $(tail -3 "$OUT/pull-signed-out.txt")"
SHA=$(sha "$P/plates/a.mp4")
ANON=$(curl -sS -o /dev/null -w '%{http_code}' "https://tensorhub.com/blobs/${SHA:0:2}/${SHA:2:2}/$SHA") ; say "anonymous read of an asset: HTTP $ANON"
[ "$ANON" = 404 ] || fail "an asset is readable anonymously: $ANON"
second_in_project "$C" status > "$OUT/status-bare.txt" 2>&1 || true
grep -q 'next: cozy pull' "$OUT/status-bare.txt" || fail "a clone's status does not say to pull: $(tail -5 "$OUT/status-bare.txt")"
sudo install -o "$U" -m 600 "$(find ~/.cozy/keys -name 'device-*.json' ! -name '*.bearer.json')" "$H/key.json" || fail "no key"
second login --key "$H/key.json" > "$OUT/login-second.txt" 2>&1; code=$?; sudo rm -f "$H/key.json"
[ "$code" = 0 ] || fail "second login: $(tail -3 "$OUT/login-second.txt")"

say "pull, it and its Calcifer killed mid-way"
sudo -u "$U" -H sh -c "echo \$\$ > '$H/pull.pid'; cd '$C' && exec '/opt/$U/cozy' pull" > "$OUT/pull-killed.txt" 2>&1 < /dev/null & PULL=$!
parts() { sudo find "$C/files" -name '.*.part' -size +1M 2> /dev/null | head -1; }
until [ -n "$(parts)" ] || ! kill -0 "$PULL" 2> /dev/null; do sleep 0.05; done
[ -n "$(parts)" ] || fail "pull ended before any file part: $(tail -3 "$OUT/pull-killed.txt")"
sudo kill -9 "$(sudo cat "$H/pull.pid")" "$(pid_of "$H")"; wait "$PULL"; code=$?
[ "$code" != 0 ] || fail "the pull finished before it was killed: make the bulk bigger"
sudo find "$C/files" -type f -printf '%f %s\n' | sort > "$OUT/after-kill.txt"
HELD=$(awk '{ n += $2 } END { print n + 0 }' "$OUT/after-kill.txt")
say "killed: $HELD of $TOTAL bytes held, $(grep -c part "$OUT/after-kill.txt") partial files"
second_in_project "$C" --json pull > "$OUT/pull-2.json" 2> "$OUT/pull-2.err" || fail "pull again: $(tail -3 "$OUT/pull-2.err")"
say "pull again: $(jq -c . "$OUT/pull-2.json")"
J --argjson held "$HELD" --argjson t "$TOTAL" --argjson f "$FILES" '.files == $f and .moved > 0 and .bytes > 0 and .bytes == $t - $held' "$OUT/pull-2.json" \
  || fail "the second pull did not fetch exactly what the first left ($TOTAL - $HELD)"
second_in_project "$C" --json pull > "$OUT/pull-3.json" 2> "$OUT/pull-3.err" || fail "third pull: $(tail -3 "$OUT/pull-3.err")"
J --argjson f "$FILES" '.files == $f and .moved == 0 and .bytes == 0' "$OUT/pull-3.json" || fail "a third pull moved something: $(jq -c . "$OUT/pull-3.json")"

say "the second computer has it all"
sudo find "$C/files" -mindepth 1 ! -type f -printf '%p %y\n' > "$OUT/not-real.txt"; [ ! -s "$OUT/not-real.txt" ] || fail "not real files: $(head -3 "$OUT/not-real.txt")"
[ -z "$(sudo find "$C/files" -name '.*')" ] || fail "parts left in files/: $(sudo find "$C/files" -name '.*' | head -3)"
sudo sh -c "cd '$C/files' && for f in *; do [ \"\$(sha256sum \"\$f\" | cut -c1-64)\" = \"\${f%%.*}\" ] || echo \"\$f\"; done" > "$OUT/wrong.txt"
[ ! -s "$OUT/wrong.txt" ] || fail "not their sha256: $(head -3 "$OUT/wrong.txt")"
second_in_project "$C" --json status > "$OUT/status-pulled.json" 2> "$OUT/status-pulled.err" || fail "status: $(tail -3 "$OUT/status-pulled.err")"
J '(.missing | length == 0) and (.stale | length == 0) and (.warnings | length == 0) and .absent == 0' "$OUT/status-pulled.json" \
  || fail "status after pull: $(jq -c '{missing, stale, warnings, absent}' "$OUT/status-pulled.json")"
sudo cmp "$C/project.lock" "$P/project.lock" || fail "reading the clone changed project.lock"

say "edit.mp4 cut again on one CPU of the runner's $(nproc)"
sudo kill "$(pid_of "$H")"; for _ in $(seq 1 50); do sudo kill -0 "$(pid_of "$H")" 2> /dev/null || break; sleep 0.1; done
sudo -u "$U" -H taskset -c 0 "/opt/$U/cozy" --json whoami < /dev/null > "$OUT/whoami-one-cpu.json" 2>&1 || fail "no Calcifer on one CPU"
say "its Calcifer may use CPUs $(sudo taskset -pc "$(pid_of "$H")" | sed 's/.*: //')"
second_in_project "$C" render all --missing --on local > "$OUT/recut.txt" 2>&1 || fail "cut again: $(tail -5 "$OUT/recut.txt")"
AGAIN=$(sudo sha256sum "$C/edit.mp4" | cut -c1-64); say "edit.mp4 with no encode: $AGAIN, the first computer's $CUT"
[ "$AGAIN" = "$CUT" ] || fail "an edit with no encode is another file on the second computer"

say "an edit that encodes its edges: the same film, not the same bytes"
edges='s/{video: <a>, start: 0, end: 120}/{video: <a>, start: 7, end: 131}/; s/{video: <b>, start: 24, end: 120}/{video: <b>, start: 13, end: 140}/'
sed -i "$edges" "$P/project.yaml"; sudo sed -i "$edges" "$C/project.yaml"
(cd "$P" && step render-edges render all --missing --on local) || fail "render edges: $(tail -5 "$OUT/render-edges.txt")"
second_in_project "$C" render all --missing --on local > "$OUT/recut-edges.txt" 2>&1 || fail "cut edges: $(tail -5 "$OUT/recut-edges.txt")"
sudo cp "$C/edit.mp4" "$OUT/edges-second.mp4" && sudo chown "$(id -u)" "$OUT/edges-second.mp4"
shape() { ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames:format=duration -of csv=p=0 "$1" | paste -sd' '; }
[ "$(shape "$P/edit.mp4")" = "$(shape "$OUT/edges-second.mp4")" ] || fail "frames and duration: $(shape "$P/edit.mp4") and $(shape "$OUT/edges-second.mp4")"
ffmpeg -v error -i "$P/edit.mp4" -i "$OUT/edges-second.mp4" -lavfi "ssim=stats_file=$OUT/ssim.log" -f null - || fail "no SSIM"
LOW=$(grep -oE 'All:[0-9.]+' "$OUT/ssim.log" | cut -d: -f2 | sort -n | head -1)
say "$(shape "$P/edit.mp4") on both; the same bytes: $([ "$(sha "$P/edit.mp4")" = "$(sha "$OUT/edges-second.mp4")" ] && echo yes || echo no); lowest per-frame SSIM $LOW"
awk -v s="$LOW" 'BEGIN { exit !(s >= 0.99) }' || fail "per-frame SSIM $LOW is under 0.99"
rm -f "$OUT/edges-second.mp4"
second_in_project "$C" --json push > "$OUT/push-second.json" 2> "$OUT/push-second.err" || fail "push from the second computer: $(tail -3 "$OUT/push-second.err")"
# Its own cut's bytes are all that is new, and its timeline's if that came out different.
SIZE=$(sudo stat -c %s "$C/edit.mp4") OTIO=$(sudo stat -c %s "$C/edit.otio")
J --argjson n "$SIZE" --argjson o "$OTIO" --argjson f "$(sudo ls "$C/files" | wc -l)" '.files == $f and ((.moved == 1 and .bytes == $n) or (.moved == 2 and .bytes == $n + $o))' "$OUT/push-second.json" || fail "the second computer's push did not send only its cut: $(jq -c . "$OUT/push-second.json")"
in_project "$P" --json push > "$OUT/push-edges.json" 2> "$OUT/push-edges.err" || fail "push the first computer's cut: $(tail -3 "$OUT/push-edges.err")"

say "a file over the Hub's limit goes as parts, a new file beside it, and its push killed mid-way"
{ printf 'RIFF\0\0\0\0WAVE'; head -c 1500M /dev/urandom; } > "$P/refs/huge.wav" && truncate -s 5368709121 "$P/refs/huge.wav" || fail "no big file"
{ printf 'RIFF\0\0\0\0WAVE'; head -c 1048564 /dev/urandom; } > "$P/refs/fresh.wav" || fail "no fresh file"
HUGE=$(hash_of "$P/refs/huge.wav") FRESH=$(hash_of "$P/refs/fresh.wav") HUGE_SHA=$(sha "$P/refs/huge.wav")
sed -i "/^  edit:$/i\  huge:\n    audio: $HUGE\n  fresh:\n    audio: $FRESH" "$P/project.yaml"
in_project "$P" push > "$OUT/push-parts-killed.txt" 2>&1 & PUSH=$!
# Past the first part: its bytes, and the parts listed in project.lock, are what a second push goes on from.
until grep -qE 'sent [0-9.]+ ?GB|[0-9.]+ GiB' "$OUT/push-parts-killed.txt" && grep -A2 "$HUGE_SHA" "$P/project.lock" | grep -q 'parts:' || ! kill -0 "$PUSH" 2> /dev/null; do sleep 1; done
kill -0 "$PUSH" 2> /dev/null && { kill -9 "$(pid_of "$HOME")"; wait "$PUSH"; code=$?; say "killed mid-way, exit $code: $(tail -1 "$OUT/push-parts-killed.txt" | cut -c1-120)"; }
KEPT=$(grep -A30 "$HUGE_SHA" "$P/project.lock" | grep -E '^ +(- )?length:' | awk '{print $NF}' | paste -sd,); say "project.lock lists the parts: $KEPT"
[ "$KEPT" = 1073741824,1073741824,1073741824,1073741824,1073741824,1 ] || fail "project.lock does not list six parts: $KEPT"
code=0; in_project "$P" --json push > "$OUT/push-huge.json" 2> "$OUT/push-huge.err" || code=$?
say "push: exit $code, $(jq -c . "$OUT/push-huge.json" | cut -c1-400)"
[ "$code" = 0 ] || fail "a push of a file over the limit exited $code: $(tail -3 "$OUT/push-huge.err")"
J --argjson f "$(ls "$P/files" | wc -l)" '.files == $f and (.skipped // [] | length == 0) and .commit != null' "$OUT/push-huge.json" || fail "the push did not send everything"
in_project "$P" --json push > "$OUT/push-huge-2.json" 2> "$OUT/push-huge-2.err"; code=$?
{ [ "$code" = 0 ] && J '.moved == 0 and .bytes == 0' "$OUT/push-huge-2.json"; } || fail "pushed again, exit $code: $(jq -c . "$OUT/push-huge-2.json")"

say "a file not at the Hub, and a pull that gets the parted file whole"
{ printf 'RIFF\0\0\0\0WAVE'; head -c 1048564 /dev/urandom; } > "$P/refs/never.wav" || fail "no unsent file"
NEVER=$(hash_of "$P/refs/never.wav")
sed -i "/^  edit:$/i\  never:\n    audio: $NEVER" "$P/project.yaml"
C2=/home/$U/film-partial
sudo install -d -o "$U" -m 700 "$C2" && sudo install -o "$U" -m 644 "$P/project.yaml" "$P/project.lock" "$C2/" || fail "no second copy"
sudo -u "$U" -H sh -c "echo \$\$ > '$H/pull.pid'; cd '$C2' && exec '/opt/$U/cozy' pull" > "$OUT/pull-parts-killed.txt" 2>&1 < /dev/null & PULL=$!
until [ -n "$(sudo find "$C2/files" -name '.*.part' -size +1100M 2> /dev/null | head -1)" ] || ! kill -0 "$PULL" 2> /dev/null; do sleep 0.5; done
[ -n "$(sudo find "$C2/files" -name '.*.part' -size +1100M 2> /dev/null | head -1)" ] || fail "the pull ended inside no part: $(tail -3 "$OUT/pull-parts-killed.txt")"
sudo kill -9 "$(sudo cat "$H/pull.pid")" "$(pid_of "$H")"; wait "$PULL"
LANDED=$(sudo find "$C2/files" -name '.*.part' -size +1100M -printf '%s\n' | head -1); say "killed inside the file, with $LANDED bytes of it"
code=0; second_in_project "$C2" --json pull > "$OUT/pull-huge.json" 2> "$OUT/pull-huge.err" || code=$?
say "pull: exit $code, $(jq -c . "$OUT/pull-huge.json" | cut -c1-400)"; grep -h 'skipped' "$OUT/pull-huge.err" | tee -a "$R" | cut -c1-200
[ "$code" = 3 ] || fail "a pull that skipped a file exited $code, not 3"
J --argjson f "$(ls "$P/files" | wc -l)" '.files == $f and (.skipped | length == 1) and (.skipped[0].path | test("^files/"))' "$OUT/pull-huge.json" || fail "the pull did not fetch everything else and name the file"
J --argjson l "$LANDED" '.bytes <= (5368709121 - $l + 1000000000)' "$OUT/pull-huge.json" || fail "the pull began the big file again"
GOT=$(sudo sh -c "sha256sum '$C2/files/$HUGE_SHA'.*" | cut -c1-64); [ "$GOT" = "$HUGE_SHA" ] || fail "the parted file came out $GOT, not $HUGE_SHA"
second_in_project "$C2" --json status > "$OUT/status-huge.json" 2> "$OUT/status-huge.err" || fail "status: $(tail -3 "$OUT/status-huge.err")"
J '.absent == 1' "$OUT/status-huge.json" || fail "status does not count the one file: $(jq -c '{absent, missing}' "$OUT/status-huge.json")"
second_in_project "$C2" --json push > "$OUT/push-partial-copy.json" 2> "$OUT/push-partial-copy.err"; code=$?
{ [ "$code" = 3 ] && J '(.skipped | length == 1) and (.skipped[0].path | test("^files/"))' "$OUT/push-partial-copy.json"; } || fail "push from the partial copy, exit $code: $(tail -3 "$OUT/push-partial-copy.err")"

say "what stays on a machine is no file of this computer's or the Hub's: no warning, and it counts for nothing"
KEPT=$(printf 'ab%.0s' $(seq 32))
sudo sh -c "jq --arg h '$KEPT' '.files[\"sha256:\" + \$h] = {path: (\"files/\" + \$h), sha256: (\"sha256:\" + \$h), machine: \"gone\", length: 1}' '$C2/project.lock' > '$C2/lock.new' && cat '$C2/lock.new' > '$C2/project.lock' && rm '$C2/lock.new'" || fail "no such row"
second_in_project "$C2" --json status > "$OUT/status-stays.json" 2> "$OUT/status-stays.err" || fail "status: $(tail -3 "$OUT/status-stays.err")"
J '.absent == 1' "$OUT/status-stays.json" || fail "status counts what stays on a machine: $(jq -c .absent "$OUT/status-stays.json")"
for verb in push pull; do
  second_in_project "$C2" --json "$verb" > "$OUT/$verb-stays.json" 2> "$OUT/$verb-stays.err"; code=$?
  { [ "$code" = 3 ] && J '(.skipped | length == 1) and all(.skipped[]; .path | test("abab") | not)' "$OUT/$verb-stays.json"; } || fail "$verb with what stays on a machine, exit $code: $(tail -3 "$OUT/$verb-stays.err")"
done

say "PROVEN"
