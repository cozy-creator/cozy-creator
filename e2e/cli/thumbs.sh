# cl-1054: pictures imported into plain projects get their thumbnails stored in them as they land (cl-982: "all thumbnails should be
# generated ahead of time, and stored in the project", Paul 11-13). On macOS the canvas's opens cell, two projects of the same 50
# pictures (1200x1200 PNGs of ~4 MB), found none stored in 60 s (e2e 43791131996): every import's measuring took the thumbnails'
# ffmpeg turns, so none was drawn until the imports ended; and finding ffmpeg, which runs it, on the threads that answer, so the page
# itself went unanswered for 30 s (43800786375). As it set up: Calcifer's page answers within a second throughout the imports, and every
# thumbnail is stored within 60 s of the last import, as the canvas's cell waits; how long they took is said (a Mac runner's three
# cores drew them 4 s to 49 s after the imports, run to run: no bar on speed). Else what Calcifer, its ffmpeg and the thumbnail route
# said, in $OUT/thumbs/. Meanwhile, each second: how many are stored, which ffmpeg and ffprobe run (thumbs/progress.txt), and how long
# the page took (thumbs/page.txt). The pictures and the projects' copies go at the end: the job keeps what it found, not 1 GB of them.
thumbs_stored() {
step "pictures imported into two plain projects get their thumbnails stored (cl-1054)"
local out=$OUT/thumbs n=${THUMBS_PICTURES:-50} i p made began short= watcher
mkdir -p "$out/pictures"
python3 - "$out/pictures" "$n" <<'PY' || { check "thumbs: no pictures made"; return; }
import os, struct, sys, zlib
out, n = sys.argv[1], int(sys.argv[2])
side = 1200
def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
for i in range(n):
    noise = os.urandom(side * side * 3)
    rows = b"".join(b"\0" + noise[y * side * 3:(y + 1) * side * 3] for y in range(side))
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", side, side, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows, 1)) + chunk(b"IEND", b"")
    with open(os.path.join(out, f"pic_{i}.png"), "wb") as f:
        f.write(png)
PY
stored() { find "$out/$1/.thumbs" -type f ! -name '.making*' 2> /dev/null | wc -l | tr -d ' ' || true; }
( while :; do
    echo "$(date -u +%T) one:$(stored one) two:$(stored two) $(ps -axo pid=,etime=,command= 2> /dev/null | grep -E '[f]fmpeg|[f]fprobe' | awk '{print $1"/"$2"/"$3}' | tr '\n' ' ')"
    sleep 1
  done ) > "$out/progress.txt" 2>&1 & watcher=$!
local holder=$HOME/.cozy/calcifer/calcifer.lock.holder pager slow
( while :; do
    echo "$(date -u +%T) $(curl -sS -o /dev/null -m 5 -w '%{http_code} %{time_total}' "http://$(jq -r .address "$holder")/" 2>&1)"
    sleep 1
  done ) > "$out/page.txt" 2>&1 & pager=$!
for p in one two; do
  cozy --json new --unencrypted "$out/$p" > "$out/new-$p.json" 2>&1 || { check "thumbs: cozy new $p: $(tail -c 300 "$out/new-$p.json")"; kill "$watcher" "$pager"; return; }
  for i in $(seq 0 $((n - 1))); do
    project_import "$out/$p" "$out/pictures/pic_$i.png" "pic_$i" > "$out/import-$p-$i.json" 2>&1 \
      || { check "thumbs: import pic_$i into $p: $(tail -c 300 "$out/import-$p-$i.json")"; kill "$watcher" "$pager"; return; }
  done
done
kill "$pager" 2> /dev/null || true
slow=$(awk '$2 !~ /^2/ || $3 > 1.0' "$out/page.txt" | wc -l | tr -d ' ')
echo "thumbs: the page during the imports: $(wc -l < "$out/page.txt" | tr -d ' ') asks, $slow over a second or not answered$( [ "$slow" = 0 ] || echo ": $(awk '$2 !~ /^2/ || $3 > 1.0' "$out/page.txt" | head -5 | tr '\n' '|')")"
[ "$slow" = 0 ] || check "thumbs: Calcifer's page took over a second, or didn't answer, $slow times during the imports"
echo "thumbs: when the imports ended: one $(stored one), two $(stored two) of $n"
began=$SECONDS
while [ $((SECONDS - began)) -lt 60 ]; do
  short=
  for p in one two; do
    made=$(stored "$p")
    [ "${made:-0}" -ge "$n" ] || short="$short $p:${made:-0}"
  done
  [ -z "$short" ] && break
  sleep 1
done
kill "$watcher" 2> /dev/null || true
echo "thumbs: $((SECONDS - began)) s after the last import, short of $n:${short:- none}"
[ -n "$short" ] || { rm -rf "$out/pictures" "$out/one/files" "$out/two/files"; return 0; }
check "thumbs: thumbnails stored 60 s after the last import, short of $n:$short"
# What says why: Calcifer's threads, the folders, cozy's ffmpeg and what it encodes, the thumbnail route, Calcifer's warnings.
local P=$out/one ffmpeg hex pid
pid=$(jq -r .pid "$HOME/.cozy/calcifer/calcifer.lock.holder" 2> /dev/null) || pid=
[ -z "$pid" ] || ! command -v sample > /dev/null || sample "$pid" 3 -file "$out/calcifer.sample" > /dev/null 2>&1 || true
{ ls -la "$P" "$P/files" "$P/.thumbs" 2>&1; ls -laR "$HOME/.cozy/tools" 2>&1 | head -60; } > "$out/folders.txt"
cozy --json doctor > "$out/doctor.json" 2>&1 || true
ffmpeg=$(find "$HOME/.cozy/tools" -name ffmpeg -type f 2> /dev/null | head -1)
{ echo "ffmpeg: ${ffmpeg:-none}"; [ -z "$ffmpeg" ] || { "$ffmpeg" -hide_banner -version 2>&1 | head -3; "$ffmpeg" -hide_banner -encoders 2>&1 | grep -c libx264;
  time "$ffmpeg" -y -v error -i "$out/pictures/pic_0.png" -vf "scale='min(iw,512)':-2" -q:v 3 -frames:v 1 "$out/by-hand.jpg" 2>&1; ls -la "$out/by-hand.jpg" 2>&1; }; } > "$out/ffmpeg.txt" 2>&1
hex=$(shasum -a 256 "$out/pictures/pic_0.png" 2> /dev/null | cut -c1-64) || hex=
[ -z "$hex" ] || project_api "$P" "/thumbs/sha256:$hex" -o "$out/thumb-0.bin" -D "$out/thumb-0.headers" -w '%{http_code}\n' > "$out/thumb-0.code" 2>&1 || true
cp "$HOME/.cozy/calcifer/diagnostics.log" "$out/diagnostics.log" 2> /dev/null || true
echo "thumbs: progress: $(head -3 "$out/progress.txt" | tr '\n' '|') … $(tail -3 "$out/progress.txt" | tr '\n' '|')"
echo "thumbs: ffmpeg: $(head -2 "$out/ffmpeg.txt" | tr '\n' ' ') route: $(cat "$out/thumb-0.code" 2> /dev/null) $(head -c 300 "$out/thumb-0.bin" 2> /dev/null)"
echo "thumbs: Calcifer's warnings: $(grep -i 'thumb\|ffmpeg\|measur' "$out/diagnostics.log" 2> /dev/null | tail -5)"
rm -rf "$out/pictures" "$out/one/files" "$out/two/files"
}

# thumb_arriving (cl-1108; xs-203's user-flow, pod-sessions 43961722220 and 43875113783): the page asks for a take's thumbnail a second
# after its run completes on its rental, while its file is still on its way here, and a 404 logs a console error the web cells fail on.
# A run's file held from landing (its project's .runs/ unwritable: its export waits and tries again, as a rental's slow link holds it),
# its picture's thumbnail is asked for as the page asks: an answer that's no error, 202 (not yet), as one not drawn yet is. Let land,
# the thumbnail is there.
thumb_arriving() {
step "a take's thumbnail asked for while its file is on its way here is no error (cl-1108)"
[ -n "$LOCAL" ] || { echo "thumb_arriving: this computer runs no models itself"; return 0; }
local P=$OUT/arriving pid number='' digest='' w=$OUT/arriving-watch.json code=''
cozy --json new --unencrypted "$P" > "$OUT/arriving-new.json" 2>&1 || { check "thumb_arriving: new: $(tail -c 300 "$OUT/arriving-new.json")"; return; }
mkdir -p "$P/.runs" && chmod 555 "$P/.runs"
(in_project "$P" --json run "$here/project/draw.py/draw" instructions=arriving --on local > "$OUT/arriving-run.ndjson" 2>&1) & pid=$!
# Its first local code here installs first: minutes on a cold runner.
for _ in $(seq 300); do
  [ -n "$number" ] || number=$(in_project "$P" --json run ls --limit 10 2> /dev/null | jq -r '[.runs[] | select(.kind == "call")][0].number // empty' 2> /dev/null || true)
  if [ -n "$number" ]; then
    in_project "$P" --json run watch "$number" --once > "$w" 2>&1 || true
    digest=$(jq -r '.output[]? | select(.sha256 != null and .url == null) | .sha256' "$w" 2> /dev/null | head -1 || true)
    [ -z "$digest" ] || break
    J '.status == "failed" or .status == "canceled"' "$w" && break
  fi
  sleep 2
done
if [ -z "$digest" ]; then
  check "thumb_arriving: no run with its file on its way: $(jq -c '{number, status, output: [.output[]? | {sha256, url}]}' "$w" 2> /dev/null) $(tail -c 300 "$OUT/arriving-run.ndjson")"
else
  code=$(project_api "$P" "/thumbs/$digest" -o "$OUT/arriving-thumb.json" -w '%{http_code}' 2> /dev/null || true)
  in_project "$P" --json run watch "$number" --once > "$w" 2>&1 || true
  echo "thumb_arriving: run $number, $(jq -r .status "$w" 2> /dev/null), its file $digest on its way ($(jq -c '[.output[]? | {sha256, url, arriving: (.url == null)}]' "$w" 2> /dev/null)): its thumbnail answered $code $(head -c 200 "$OUT/arriving-thumb.json")"
  J --arg d "$digest" '[.output[]? | select(.sha256 == $d and .url == null)] | length == 1' "$w" || check "thumb_arriving: $digest landed while it was held, so the cell judges nothing"
  [ "$code" = 202 ] || check "thumb_arriving: a thumbnail of $digest, on its way here, answered $code, not 202: $(head -c 300 "$OUT/arriving-thumb.json")"
  # Let land: the export's next try takes it, and the thumbnail drawn as it landed is there.
  chmod 755 "$P/.runs"
  for _ in $(seq 60); do
    code=$(project_api "$P" "/thumbs/$digest" -o "$OUT/arriving-landed.bin" -w '%{http_code}' 2> /dev/null || true)
    [ "$code" = 200 ] && break
    sleep 1
  done
  [ "$code" = 200 ] || check "thumb_arriving: once $digest could land, its thumbnail answered $code after 60 s: $(head -c 300 "$OUT/arriving-landed.bin")"
fi
chmod 755 "$P/.runs" 2> /dev/null || true
kill "$pid" 2> /dev/null || true
wait "$pid" 2> /dev/null || true
}
