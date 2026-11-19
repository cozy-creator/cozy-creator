# h3-layout-caps (se-170): H3 Upscale gives the same video for the same request whatever the card's memory. Paul, 11-18: "yeah I
# would prefer the same output regardless of memory size available. Also do whatever produces the highest quality video in
# the end." The refine's window layout changes the picture, so it must come from the request alone (minimax-h3
# `regenerate.WINDOW_ROWS`), never from the room on the card.
# One rental: each packages_refs tree in `trees` runs one Upscale request (`res` of the plate's first `seconds`) uncapped,
# then again with the GPU at `cap`; each arm starts from no derived state (lib.sh `derived_state`: no memo answers the second
# run, which is the same request). Per tree: the `h3 regenerate` line's layout (windows, latent frames) and the output's bytes
# under both caps. SAME: one layout and identical bytes (else identical decoded frames and sound); DIFFERENT otherwise, with the PSNR
# between them. The session passes when each tree says what `expect` asks (default SAME): a tree whose layout follows the
# card is red, one whose layout is the request's is green.
# The plate: `source` (default Tears of Steel, CC BY 3.0 Blender Foundation, 4K) from `start` s, center-cropped to 7:4, H3's
# 1344x768 at 24 fps with its sound, a box of the session's project. Before renting, at $0: each tree's layout from the
# Runtime's own `author/_split.py` (none: the card's), its Triton sets, and each tree locked and installed on this runner's
# local machine through cozy. Every run names its machine. The pod ends after the last run; the verdicts follow.
# params: runtime, tensorfs (wheels, https://…#sha256=<hex>: every run must run on them), trees ("rc fix": packages_refs
# names, in order), expect ({"<tree>": "same"|"different"}), res (1080), seconds (15), cap (6GiB), source, start (25), prompt,
# need_s (2700). (`cap` the dollar cap is lib.sh's CAP; this one is the GPU's.)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")" && pwd)/fixtures/h3-layout-caps
TREES=$(param trees "rc fix") EXPECT=$(param expect '{}') RES=$(param res 1080) SECS=$(param seconds 15) GCAP=$(param cap 6GiB)
SOURCE=$(param source "https://upload.wikimedia.org/wikipedia/commons/1/10/Tears_of_Steel_in_4k_-_Official_Blender_Foundation_release.webm")
START=$(param start 25) PROMPT=$(param prompt "")
[[ $RES =~ ^(1080|1440|2160)$ && $SECS =~ ^(5|10|15)$ && $GCAP =~ ^[0-9]+GiB$ && $START =~ ^[0-9]+([.][0-9]+)?$ ]] ||
  fail "res $RES, seconds $SECS, cap $GCAP, start $START"
[ -n "$PROMPT" ] || fail "no prompt"
jq -e 'type == "object" and all(.[]; . == "same" or . == "different")' <<< "$EXPECT" > /dev/null || fail "expect: {tree: same|different}"
for tool in ffmpeg ffprobe curl jq unzip; do command -v "$tool" > /dev/null || fail "no $tool"; done
for t in $TREES; do [ -f "$PACKAGES/$t/minimax-h3/minimax_h3/regenerate.py" ] || fail "no packages_refs checkout $t"; done
RENT_ARGS=--ssh  # derived_state reads and clears the pod's derived state over its root shell
WORK=${RUNNER_TEMP:-$OUT/..}/h3-layout-caps; mkdir -p "$WORK" "$OUT/wheel" "$OUT/records" "$OUT/media"
fetch_wheel() {
  local url=$1 f
  [[ $url == https://*'#sha256='* ]] || return 1
  f=$OUT/wheel/$(basename "${url%%#*}" | sed 's/%2B/+/g')
  curl -fsSL --retry 5 --retry-all-errors "${url%%#*}" -o "$f" && [ "$(sha256sum "$f" | cut -c1-64)" = "${url##*sha256=}" ] && echo "$f"
}
W=$(fetch_wheel "$(param runtime)") || fail "runtime: no wheel https://…#sha256=<hex> that matches"
TW=$(fetch_wheel "$(param tensorfs)") || fail "tensorfs: no wheel https://…#sha256=<hex> that matches"
WANT_RT=$(basename "$W" | cut -d- -f2) WANT_TFS=$(basename "$TW" | cut -d- -f2)
unzip -p "$W" cozy_runtime/author/_split.py > "$OUT/split_rc.py" && [ -s "$OUT/split_rc.py" ] || fail "no author/_split.py in $W"
T=$(( (SECS * 24 - 5 + 16) / 17 * 5 + 2 ))  # latent frames of its 17n+5 video frames
case $RES in 1080) HW="68 120" ;; 1440) HW="90 160" ;; 2160) HW="136 240" ;; esac
layout() { python3 -I "$FIX/layout.py" "$OUT/split_rc.py" "$PACKAGES/$1/minimax-h3/minimax_h3/regenerate.py" "$2" "$3" "$4"; }
for t in $TREES; do
  l=$(layout "$t" "$T" $HW) || fail "$t: layout"
  say "tree $t ($(git -C "$PACKAGES/$t" rev-parse --short HEAD)): ${RES}p of ${SECS} s at $T latent frames: $(jq -r 'if .most == null then "the card chooses its layout"
    else "\(.spans | length) windows \(.spans | map("\(.[0])-\(.[1])") | join(" "))" end' <<< "$l"), expected $(jq -r --arg t "$t" '.[$t] // "same"' <<< "$EXPECT")"
done
curl -fsSL --retry 5 -A "cozy-release-gate/1.0" "$SOURCE" -o "$WORK/source" || fail "fetch $SOURCE"
ffmpeg -nostdin -v error -y -ss "$START" -i "$WORK/source" -t "$SECS" -vf "crop=trunc(ih*7/8)*2:ih,scale=1344:768:flags=lanczos,fps=24,format=yuv420p" \
  -c:v libx264 -crf 12 -preset slow -c:a aac -b:a 192k -movflags +faststart "$OUT/media/plate.mp4" || fail "the plate"
say "plate: $(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=width,height,nb_read_frames -of compact=p=0:nk=1 "$OUT/media/plate.mp4")"
for t in $TREES; do
  kernels "$PACKAGES/$t/minimax-h3"
  timeout 1800 cozy package install "$PACKAGES/$t/minimax-h3" --on local -y > "$OUT/lock-$t.txt" 2>&1 ||
    fail "$t doesn't lock and install through this cozy: $(tail -c 600 "$OUT/lock-$t.txt")"
  say "tree $t: locked and installed locally through cozy"
done

quote "$(param need_s 2700)"
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$W" --tensorfs "$TW" -y || fail "update"
run_page import "$OUT/media/plate.mp4" --name plate --description "$PROMPT" > "$OUT/import.txt" 2>&1 || fail "import: $(cat "$OUT/import.txt")"
# arm <cap>: no derived state (no memo, bake or learned peak from before), the GPU at <cap> (off: uncapped), the pod answering.
arm() {
  derived_state "cap-$1" reset
  step "cap-$1" machine update "$NAME" --gpu-memory-limit "$1" -y || fail "cap $1"
  local t0=$SECONDS
  until timeout 60 cozy model ls --on "$NAME" --json > /dev/null 2>&1; do [ $((SECONDS - t0)) -lt 600 ] || fail "$NAME hasn't answered at cap $1"; sleep 5; done
  derived_state "cap-$1" settle
  derived_zero || fail "cap $1: the arm starts with derived state it did not make: $DERIVED"
  say "cap $1: $(cozy --json machine show "$NAME" | jq -c '[.status.gpus[]? | .memory_limit_bytes // 0]') bytes"
}
printf 'cell\ttree\tcap\trun\tstatus\twindows\twindow_frames\tdigest\texec_s\trefine_s\tpeak_gib\n' > "$OUT/cells.tsv"
run_cell() {
  local t=$1 c=$2 k=$1-$2 n row line st
  cd "$PACKAGES/$t" || fail "no tree $t"
  step "$k" run ./minimax-h3/upscale "video=<plate>" "to=${RES}p" "prompt=$PROMPT" --on "$NAME"
  n=$(runno "$k"); cd "$RUN_PROJECT" || fail "no $RUN_PROJECT"
  [ -n "$n" ] || { say "$k: no run: $(tail -c 400 "$OUT/$k.txt")"; printf '%s\t%s\t%s\t-\tnone\n' "$k" "$t" "$c" >> "$OUT/cells.tsv"; return; }
  cozy run watch "$n" --once --json --full > "$OUT/records/$k.json" 2> /dev/null
  cozy --full run watch "$n" --once > "$OUT/records/$k.log" 2>&1
  mkdir -p "$WORK/$k"; cozy run watch "$n" --once -o "$WORK/$k" > /dev/null 2>&1
  v=$(ls "$WORK/$k"/*.mp4 2> /dev/null | head -1); [ -z "$v" ] || cp "$v" "$OUT/media/$k.mp4"
  row=$(python3 -I "$FIX/cellrow.py" "$LEDGER" "$OUT/records/$k.json" 2> /dev/null) || row='{}'
  line=$(LC_ALL=C grep -a -o -m1 -E 'h3 regenerate .*' "$OUT/records/$k.log")
  st=$(jq -r '.status // "none"' <<< "$row")
  [ "$(jq -r '.memoized' <<< "$row")" != true ] || { say "$k: answered from the call memo, not rendered"; st=memoized; }
  jq -e --arg r "$WANT_RT" --arg f "$WANT_TFS" '.software | length > 0 and all(.[]; .[0] == $r and .[1] == $f)' <<< "$row" > /dev/null ||
    [ "$st" != completed ] || fail "$k ran on $(jq -c .software <<< "$row"), not Runtime $WANT_RT TensorFS $WANT_TFS"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$k" "$t" "$c" "$n" "$st" "$(grep -o -E '\bwindows=[0-9]+' <<< "$line" | cut -d= -f2)" \
    "$(grep -o -E 'window_latent_frames=[0-9]+' <<< "$line" | cut -d= -f2)" "$(jq -r '[.result | .. | .digest? // empty] | join(",")' "$OUT/records/$k.json" 2> /dev/null)" \
    "$(jq -r '[.execution_s, .refine_s, .peak_vram_gib] | map(. // "?" | tostring) | join("\t")' <<< "$row")" >> "$OUT/cells.tsv"
  say "$k: run $n $st, ${line:-no h3 regenerate line}; $(jq -c 'del(.software)' <<< "$row")"
}
for c in off "$GCAP"; do
  arm "$c"
  for t in $TREES; do run_cell "$t" "$c"; done
done
for r in "${RIDS[@]}"; do
  until gone "$r"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1; sleep 10; done
  say "GONE $r; billed $(billed "$r")"
done
column -t -s $'\t' "$OUT/cells.tsv" | tee -a "$R"

# Each tree's verdict: one layout and the same bytes under both caps.
# frames <file>: its decoded video and audio, hashed (ab.sh's): one file's container stamps aside.
frames() { ffmpeg -nostdin -v error -i "$1" -map 0 -f framemd5 - 2> /dev/null | grep -v '^#' | md5sum | cut -c1-16; }
bad=0
for t in $TREES; do
  a=$(awk -F'\t' -v k="$t-off" '$1 == k' "$OUT/cells.tsv") b=$(awk -F'\t' -v k="$t-$GCAP" '$1 == k' "$OUT/cells.tsv")
  sa=$(cut -f5 <<< "$a") wa=$(cut -f6 <<< "$a") fa=$(cut -f7 <<< "$a") da=$(cut -f8 <<< "$a")
  sb=$(cut -f5 <<< "$b") wb=$(cut -f6 <<< "$b") fb=$(cut -f7 <<< "$b") db=$(cut -f8 <<< "$b")
  why=''
  if [ "${sa:-}" != completed ] || [ "${sb:-}" != completed ]; then
    why="a run did not complete (uncapped ${sa:-none}, at $GCAP ${sb:-none})"
    [ -z "${wa:-}" ] || [ -z "${wb:-}" ] || [ "$wa x $fa" = "$wb x $fb" ] || why="$why; layouts ${wa}x$fa uncapped, ${wb}x$fb at $GCAP"
    v=DIFFERENT
  elif [ "$wa x $fa" != "$wb x $fb" ]; then
    v=DIFFERENT why="layouts ${wa}x$fa uncapped, ${wb}x$fb at $GCAP"
  elif [ -n "$da" ] && [ "$da" = "$db" ]; then
    v=SAME why="layout ${wa}x$fa under both, identical bytes ($da)"
  elif [ -s "$OUT/media/$t-off.mp4" ] && [ -s "$OUT/media/$t-$GCAP.mp4" ] &&
       [ "$(frames "$OUT/media/$t-off.mp4")" = "$(frames "$OUT/media/$t-$GCAP.mp4")" ]; then
    v=SAME why="layout ${wa}x$fa under both, identical decoded frames and sound (the files' bytes differ)"
  else
    v=DIFFERENT why="layout ${wa}x$fa under both, but the frames differ"
  fi
  if [ "$v" = DIFFERENT ] && [ -s "$OUT/media/$t-off.mp4" ] && [ -s "$OUT/media/$t-$GCAP.mp4" ]; then
    why="$why; PSNR $(ffmpeg -nostdin -i "$OUT/media/$t-off.mp4" -i "$OUT/media/$t-$GCAP.mp4" -lavfi '[0:v][1:v]psnr' -f null - 2>&1 | grep -oE 'average:[0-9.inf]+' | tail -1)"
  fi
  want=$(jq -r --arg t "$t" '.[$t] // "same"' <<< "$EXPECT" | tr a-z A-Z)
  say "VERDICT $t: $v ($why); expected $want"
  [ "$v" = "$want" ] || bad=$((bad + 1))
done
[ "$bad" = 0 ] || fail "$bad trees not as expected"
exit "$RC"
