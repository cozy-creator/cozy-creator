# cl-1054: pictures imported into a plain project get their thumbnails stored in it (cl-982: "all thumbnails should be generated
# ahead of time, and stored in the project", Paul 11-13). On macOS rc/0.26.0's Calcifer stored none (canvas e2e 43791131996).
# Each made in `<project>/.thumbs/` within 120 s; else what Calcifer, its ffmpeg and the thumbnail route said, in $OUT/thumbs/.
thumbs_stored() {
step "pictures imported into a plain project get their thumbnails stored (cl-1054)"
local out=$OUT/thumbs P=$OUT/thumbs/project n=${THUMBS_PICTURES:-12} i made=0 began
mkdir -p "$out/pictures"
cozy --json new --unencrypted "$P" > "$out/new.json" 2>&1 || { check "thumbs: cozy new: $(tail -c 300 "$out/new.json")"; return; }
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
for i in $(seq 0 $((n - 1))); do
  project_import "$P" "$out/pictures/pic_$i.png" "pic_$i" > "$out/import-$i.json" 2>&1 || { check "thumbs: import pic_$i: $(tail -c 300 "$out/import-$i.json")"; return; }
done
began=$SECONDS
while [ $((SECONDS - began)) -lt 120 ]; do
  made=$(find "$P/.thumbs" -type f ! -name '.making*' 2> /dev/null | wc -l | tr -d ' ')
  [ "$made" -ge "$n" ] && break
  sleep 1
done
echo "thumbs: $made of $n stored in $((SECONDS - began)) s"
[ "$made" -ge "$n" ] && return 0
check "thumbs: $made of $n pictures' thumbnails stored in $P/.thumbs within 120 s"
# What says why: the folders, the doctor's tools, cozy's ffmpeg and what it encodes, the thumbnail route, Calcifer's warnings.
{ ls -la "$P" "$P/files" "$P/.thumbs" 2>&1; ls -laR "$HOME/.cozy/tools" 2>&1 | head -60; } > "$out/folders.txt"
cozy --json doctor > "$out/doctor.json" 2>&1 || true
local ffmpeg
ffmpeg=$(find "$HOME/.cozy/tools" -name ffmpeg -type f 2> /dev/null | head -1)
{ echo "ffmpeg: ${ffmpeg:-none}"; [ -z "$ffmpeg" ] || { "$ffmpeg" -hide_banner -version 2>&1 | head -3; "$ffmpeg" -hide_banner -encoders 2>&1 | grep -c libx264;
  "$ffmpeg" -y -v error -i "$out/pictures/pic_0.png" -vf "scale='min(iw,512)':-2" -q:v 3 -frames:v 1 "$out/by-hand.jpg" 2>&1; ls -la "$out/by-hand.jpg" 2>&1; }; } > "$out/ffmpeg.txt"
local hex
hex=$(shasum -a 256 "$out/pictures/pic_0.png" 2> /dev/null | cut -c1-64) || hex=
[ -z "$hex" ] || project_api "$P" "/thumbs/sha256:$hex" -o "$out/thumb-0.bin" -D "$out/thumb-0.headers" -w '%{http_code}\n' > "$out/thumb-0.code" 2>&1 || true
cp "$HOME/.cozy/calcifer/diagnostics.log" "$out/diagnostics.log" 2> /dev/null || true
echo "thumbs: ffmpeg: $(head -2 "$out/ffmpeg.txt" | tr '\n' ' ') route: $(cat "$out/thumb-0.code" 2> /dev/null) $(head -c 300 "$out/thumb-0.bin" 2> /dev/null)"
echo "thumbs: Calcifer's warnings: $(grep -i 'thumb\|ffmpeg' "$out/diagnostics.log" 2> /dev/null | tail -5)"
}
