# h3-joins' measures of a video, ffmpeg and awk only (sourced). Each prints its numbers; the session judges them.

# frames <video>: its decoded frames.
frames() { ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$1"; }
size() { ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0:s=x "$1"; }
has_sound() { ffprobe -v error -select_streams a -show_entries stream=codec_type -of csv=p=0 "$1" | grep -q audio; }

# stills <video> <dir> <prefix> <n…>: those frames as <prefix>-<n>.png.
stills() {
  local video=$1 dir=$2 prefix=$3 n; shift 3
  for n in "$@"; do ffmpeg -v error -y -i "$video" -vf "select=eq(n\\,$n)" -vsync 0 -frames:v 1 "$dir/$prefix-$n.png"; done
}

# around <video> <join> <png>: the 8 frames join-4 … join+3 as one sheet, 4 a row, the join starting the second row.
around() { ffmpeg -v error -y -i "$1" -vf "select='between(n,$(($2 - 4)),$(($2 + 3)))',scale=672:-2,tile=4x2" -vsync 0 -frames:v 1 "$3"; }

# pairs <video> <dir>: each frame against the next (line n: frame n-1 to frame n), PSNR, SSIM and luma, into <dir>.
pairs() {
  local next="split[a][b];[b]trim=start_frame=1,setpts=N/FRAME_RATE/TB[c];[a]setpts=N/FRAME_RATE/TB[d];[d][c]"
  ffmpeg -v error -i "$1" -lavfi "${next}psnr=stats_file=$2/psnr.txt:shortest=1" -f null - &&
    ffmpeg -v error -i "$1" -lavfi "${next}ssim=stats_file=$2/ssim.txt:shortest=1" -f null - &&
    ffmpeg -v error -i "$1" -vf "signalstats,metadata=print:key=lavfi.signalstats.YAVG:file=$2/luma.txt" -f null -
}

# join_step <dir> <join>: frame join-1 to frame join against the 24 pairs either side, from `pairs`: "psnr J median M low L ssim J median M
# low L luma J median M high H".
join_step() {
  local psnr ssim luma
  psnr=$(awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^psnr_avg:/) { v = substr($i, 10); print (v == "inf" ? 100 : v + 0) } }' "$1/psnr.txt")
  ssim=$(awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^All:/) print substr($i, 5) + 0 }' "$1/ssim.txt")
  luma=$(awk -F= '/YAVG/ { y[n++] = $2 } END { for (i = 1; i < n; i++) { d = y[i] - y[i - 1]; print (d < 0 ? -d : d) } }' "$1/luma.txt")
  for series in "$psnr" "$ssim" "$luma"; do
    awk -v j="$2" '{ s[NR] = $1 } END {
      for (i = j - 24; i <= j + 24; i++) if (i != j && (i in s)) { n++; v[n] = s[i] }
      for (i = 1; i <= n; i++) for (k = i + 1; k <= n; k++) if (v[k] < v[i]) { t = v[i]; v[i] = v[k]; v[k] = t }
      printf "%.4f %.4f %.4f %.4f ", (j in s) ? s[j] : -1, v[int((n + 1) / 2)], v[1], v[n] }' <<<"$series"
  done | awk '{ printf "psnr %s median %s low %s ssim %s median %s low %s luma %s median %s high %s", $1, $2, $3, $5, $6, $7, $9, $10, $12 }'
}

# fitted <picture> <png>: the picture as H3 fills its own canvas with it (ImageOps.fit): its middle at 1344:768, scaled to that.
fitted() { ffmpeg -v error -y -i "$1" -vf "crop='if(gt(iw/ih,1.75),ih*1.75,iw)':'if(gt(iw/ih,1.75),ih,iw/1.75)',scale=1344:768:flags=bicubic" "$2"; }
squashed() { ffmpeg -v error -y -i "$1" -vf "scale=1344:768:flags=bicubic" "$2"; }

# likeness <a> <b>: SSIM and PSNR of two pictures of one size, "ssim S psnr P".
likeness() {
  local s p
  s=$(ffmpeg -i "$1" -i "$2" -lavfi "[0:v]format=yuv444p[a];[1:v]format=yuv444p[b];[a][b]ssim" -f null - 2>&1 | grep -oE 'All:[0-9.]+' | tail -1 | cut -d: -f2)
  p=$(ffmpeg -i "$1" -i "$2" -lavfi "[0:v]format=yuv444p[a];[1:v]format=yuv444p[b];[a][b]psnr" -f null - 2>&1 | grep -oE 'average:[0-9.inf]+' | tail -1 | cut -d: -f2)
  echo "ssim ${s:-none} psnr ${p:-none}"
}

# side <png> <a> <b> <c>: three pictures of one size side by side, halved.
side() { ffmpeg -v error -y -i "$2" -i "$3" -i "$4" -filter_complex "[0:v]scale=672:384[a];[1:v]scale=672:384[b];[2:v]scale=672:384[c];[a][b][c]hstack=3" "$1"; }

# grid <first segment s> <later segment s…>: the frames each segment delivers on H3's 17n+5 grid, carrying 22 frames: one a line.
grid() {
  local first=1 s f
  for s in "$@"; do
    if [ "$first" = 1 ]; then f=$((24 * s)); else f=$((22 + 24 * s)); fi
    f=$((f + ((5 - f) % 17 + 17) % 17))
    if [ "$first" = 1 ]; then echo "$f"; else echo $((f - 22)); fi
    first=0
  done
}
