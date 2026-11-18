# h3-refine-layout (se-170, proof only): H3 Upscale's refine in one window against the same request in windows, on one card,
# uncapped, so the layout is the only difference. Two packages_refs checkouts of minimax-h3 that differ only in
# `regenerate.WINDOW_ROWS`: `one` (`_MOST_ROWS`, 385,200 rows: one window up to 1440p 15 s) and `r` (37 x 3600 = 133,200: rule R).
# The plate: `source` (default Tears of Steel, CC BY 3.0 Blender Foundation, 4K) from `start` s, center-cropped to 7:4 and
# scaled to H3's 1344x768 at 24 fps with its sound: 15 s (clip15) and its first 5 s (clip5), each a box of the session's
# project, so a cell's seed (the clip's hash) and `prompt` are the same in both arms.
# Before renting, at $0: the source and both clips (sheets: clip-sheet.jpg, source-sheet.jpg with `sheet`), each tree's layout
# for each request from the Runtime's own `author/_split.py` (fixtures/h3-refine-layout/layout.py), its Triton sets, and each
# tree locked and installed on this runner's local machine through cozy (`local_code.rs` `locked`), then described.
# Cells, in `cells` order, every run `--on` its machine: `warm` (tree one, 1080p of clip5: fetches every weight, never
# compared), then `<tree>-<1080|1440|2160>`: 1080p and 1440p of clip15, 2160p of clip5. Each cell's run must have run on the
# Runtime and TensorFS asked (`runtime`, `tensorfs`), not memoized, and in the layout its tree's rule gives the latent shape its
# `h3 regenerate` line logs. Records go to records/ (not the ledger: the trees' code isn't their version's).
# The pod ends after the last cell; then, on the runner, fixtures/h3-refine-layout/analyze.py compares each pair.
# params: runtime, tensorfs (wheels, https://…#sha256=<hex>), source, start, prompt, cells, sheet (true: the source's contact
# sheet, 30 frames from sheet_from s every sheet_step s), max_failures (2), need_s (5400). (`cap` is lib.sh's dollar cap.)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/h3-refine-layout FRAMES_PY=$(cd "$HERE/.." && pwd)/frames.py
CELLS=$(param cells "warm r-1440 one-1440 r-1080 one-1080 r-2160 one-2160") MOST=$(param max_failures 2)
SOURCE=$(param source "https://upload.wikimedia.org/wikipedia/commons/1/10/Tears_of_Steel_in_4k_-_Official_Blender_Foundation_release.webm")
START=$(param start 0) PROMPT=$(param prompt "") SHEET=$(param sheet false) SHEET_FROM=$(param sheet_from 0) SHEET_STEP=$(param sheet_step 10)
[ -n "$PROMPT" ] || fail "no prompt"
[[ $START =~ ^[0-9]+([.][0-9]+)?$ ]] || fail "start $START"
for tool in ffmpeg ffprobe curl jq uv unzip; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -f "$FRAMES_PY" ] && [ -f "$LEDGER" ] || fail "no frames.py or ledger.py"
tree_of() { if [ "$1" = warm ]; then echo one; else echo "${1%%-*}"; fi; }
res_of() { if [ "$1" = warm ]; then echo 1080; else echo "${1##*-}"; fi; }
clip_of() { if [ "$1" = warm ] || [ "$(res_of "$1")" = 2160 ]; then echo clip5; else echo clip15; fi; }
latent_hw() { case $1 in 1080) echo "68 120" ;; 1440) echo "90 160" ;; 2160) echo "136 240" ;; esac; }
for c in $CELLS; do [[ $c == warm || $c =~ ^(one|r)-(1080|1440|2160)$ ]] || fail "cell $c: warm or <one|r>-<1080|1440|2160>"; done
for t in one r; do
  f=$PACKAGES/$t/minimax-h3/minimax_h3/regenerate.py
  [ -f "$f" ] || fail "no packages_refs checkout $t"
  say "tree $t ($(git -C "$PACKAGES/$t" rev-parse --short HEAD)): $(grep -m1 '^WINDOW_ROWS' "$f")"
done
mkdir -p "$OUT/wheel" "$OUT/media" "$OUT/records"
WORK=${RUNNER_TEMP:-$OUT/..}/h3-refine-layout; mkdir -p "$WORK"
fetch_wheel() {
  local url=$1 f
  [[ $url == https://*'#sha256='* ]] || fail "$url is not https://…#sha256=<hex>"
  f=$OUT/wheel/$(basename "${url%%#*}" | sed 's/%2B/+/g')
  curl -fsSL --retry 5 --retry-all-errors "${url%%#*}" -o "$f" || fail "cannot fetch $url"
  [ "$(sha256sum "$f" | cut -c1-64)" = "${url##*sha256=}" ] || fail "sha256 of $f"
  echo "$f"
}
W=$(fetch_wheel "$(param runtime)") || exit 1
TW=$(fetch_wheel "$(param tensorfs)") || exit 1
WANT_RT=$(basename "$W" | cut -d- -f2) WANT_TFS=$(basename "$TW" | cut -d- -f2)
say "candidate: cozy $(cozy --version 2>/dev/null | awk '{print $NF}'), Runtime $WANT_RT, TensorFS $WANT_TFS"
unzip -p "$W" cozy_runtime/author/_split.py > "$OUT/split_rc.py" && [ -s "$OUT/split_rc.py" ] || fail "no author/_split.py in $W"
# layout <tree> <T> <H> <W>: its windows over (1, 24, T, H, W) latents (layout.py).
layout() { python3 -I "$FIX/layout.py" "$OUT/split_rc.py" "$PACKAGES/$1/minimax-h3/minimax_h3/regenerate.py" "$2" "$3" "$4"; }
for c in $CELLS; do
  t=$(tree_of "$c") T=107; [ "$(clip_of "$c")" = clip15 ] || T=37
  l=$(layout "$t" "$T" $(latent_hw "$(res_of "$c")")) || fail "layout of $c"
  say "cell $c: tree $t, $(res_of "$c")p of $(clip_of "$c"), expected at $T latent frames: $(jq -c '{windows: (.spans | length), spans, distinct}' <<< "$l")"
done

# The plate.
M=$OUT/media
curl -fsSL --retry 5 -A "cozy-release-gate/1.0" "$SOURCE" -o "$WORK/source" || fail "fetch $SOURCE"
VF="crop=trunc(ih*7/8)*2:ih,scale=1344:768:flags=lanczos,fps=24,format=yuv420p"
for s in 15 5; do
  ffmpeg -nostdin -v error -y -ss "$START" -i "$WORK/source" -t "$s" -vf "$VF" -c:v libx264 -crf 12 -preset slow -c:a aac -b:a 192k \
    -movflags +faststart "$M/clip$s.mp4" || fail "clip$s"
  say "clip$s: $(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=width,height,r_frame_rate,nb_read_frames -of compact=p=0:nk=1 "$M/clip$s.mp4"), sha256 $(sha256sum < "$M/clip$s.mp4" | cut -c1-16)"
done
# Each second's first frame and the last, 4x4.
ffmpeg -nostdin -v error -y -i "$M/clip15.mp4" -vf "select='not(mod(n\,24))+eq(n\,359)',scale=480:-2,tile=4x4" -vsync 0 -frames:v 1 "$OUT/clip-sheet.jpg" || fail "clip sheet"
if [ "$SHEET" = true ]; then
  for i in $(seq 0 29); do
    at=$((SHEET_FROM + i * SHEET_STEP))
    ffmpeg -nostdin -v error -y -ss "$at" -i "$WORK/source" -frames:v 1 -vf "scale=320:-2,drawtext=text='$at':x=8:y=8:fontsize=28:fontcolor=yellow" \
      "$WORK/s$(printf %02d "$i").png" 2> /dev/null || ffmpeg -nostdin -v error -y -ss "$at" -i "$WORK/source" -frames:v 1 -vf scale=320:-2 "$WORK/s$(printf %02d "$i").png"
  done
  ffmpeg -nostdin -v error -y -i "$WORK/s%02d.png" -vf tile=6x5 -frames:v 1 "$OUT/source-sheet.jpg" || say "no source sheet"
fi

# Each tree: its Triton sets, the lock a staging makes (installed on this runner's local machine through cozy), and Upscale described.
for t in one r; do
  kernels "$PACKAGES/$t/minimax-h3"
  timeout 1800 cozy package install "$PACKAGES/$t/minimax-h3" --on local -y > "$OUT/lock-$t.txt" 2>&1 ||
    fail "$t doesn't lock and install through this cozy: $(tail -c 600 "$OUT/lock-$t.txt")"
  say "tree $t: locked and installed locally through cozy: $(grep -a -o -E '(cozy-runtime|tensorfs)[ =@v]+[0-9][^ ,)]*' "$OUT/lock-$t.txt" | sort -u | paste -sd' ')"
  (cd "$RUN_PROJECT" && cozy run "$PACKAGES/$t/minimax-h3/upscale" --describe) > "$OUT/describe-$t.txt" 2>&1 ||
    fail "$t's upscale doesn't describe: $(tail -c 400 "$OUT/describe-$t.txt")"
done

quote "$(param need_s 5400)"
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$W" --tensorfs "$TW" -y || fail "update"
# The pod answers this runner's Calcifer after the update's restart (10 min at most).
t0=$SECONDS
until timeout 60 cozy machine show "$NAME" --json > "$OUT/answers.json" 2>&1 && ! grep -q -i -E 'unreachable' "$OUT/answers.json" &&
      timeout 60 cozy model ls --on "$NAME" --json > /dev/null 2>&1; do
  [ $((SECONDS - t0)) -lt 600 ] || fail "$NAME hasn't answered in 10 min after its update"; sleep 5
done
say "pod $(jq -c '{v: .status.version, rt: .status.runtime, tfs: .status.tensorfs, gpu: .status.gpus[0].name, limit: .status.gpu_memory_limit}' "$OUT/answers.json")"
for s in 15 5; do
  run_page import "$M/clip$s.mp4" --name "clip$s" --description "$PROMPT" > "$OUT/import-clip$s.txt" 2>&1 || fail "import clip$s: $(cat "$OUT/import-clip$s.txt")"
done

FAILS=0
printf 'cell\ttree\tres\trun\tstatus\twindows\twindow_frames\tlatent\texec_s\trefine_s\tstep1_s\tsteady_s\tupscale_s\tdecode_s\tpeak_gib\tsm_mhz\n' > "$OUT/cells.tsv"
cell() {
  local k=$1 t res clip n code st line latent windows wf T H Wd row
  t=$(tree_of "$k") res=$(res_of "$k") clip=$(clip_of "$k")
  cd "$PACKAGES/$t" || fail "no tree $t"
  step "$k" run ./minimax-h3/upscale "video=<$clip>" "to=${res}p" "prompt=$PROMPT" --on "$NAME"; code=$?
  n=$(runno "$k"); cd "$RUN_PROJECT" || fail "no $RUN_PROJECT"
  [ -n "$n" ] || { say "FAIL $k: no run (exit $code): $(tail -c 400 "$OUT/$k.txt")"; FAILS=$((FAILS + 1)); [ "$FAILS" -lt "$MOST" ] || fail "$FAILS cells failed"; return; }
  cozy run watch "$n" --once --json --full > "$OUT/records/$k.json" 2> /dev/null
  cozy --full run watch "$n" --once > "$OUT/records/$k.log" 2>&1
  mkdir -p "$WORK/$k"; cozy run watch "$n" --once -o "$WORK/$k" > /dev/null 2>&1
  v=$(ls "$WORK/$k"/*.mp4 2> /dev/null | head -1); [ -z "$v" ] || cp "$v" "$M/$k.mp4"
  row=$(python3 -I "$FIX/cellrow.py" "$LEDGER" "$OUT/records/$k.json" 2> "$OUT/records/$k.row.err") || row='{}'
  echo "$row" > "$OUT/records/$k.row.json"
  line=$(LC_ALL=C grep -a -o -m1 -E 'h3 regenerate .*' "$OUT/records/$k.log")
  latent=$(grep -o -E 'latent=[0-9x]+' <<< "$line" | cut -d= -f2) windows=$(grep -o -E '\bwindows=[0-9]+' <<< "$line" | cut -d= -f2)
  wf=$(grep -o -E 'window_latent_frames=[0-9]+' <<< "$line" | cut -d= -f2)
  st=$(jq -r '.status // "none"' <<< "$row")
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$k" "$t" "$res" "$n" "$st" "${windows:-?}" "${wf:-?}" "${latent:-?}" \
    "$(jq -r '[.execution_s, .refine_s, .step1_s, .steady_s, .upscale_latent_s, .decode_s, .peak_vram_gib, .sm_mhz_mean] | map(. // "?" | tostring) | join("\t")' <<< "$row")" >> "$OUT/cells.tsv"
  say "$k: run $n $st, ${line:-no h3 regenerate line}; $(jq -c 'del(.software)' <<< "$row")"
  if [ -n "$latent" ]; then
    IFS=x read -r T H Wd <<< "$latent"
    layout "$t" "$T" "$H" "$Wd" > "$OUT/records/$k.layout.json" || fail "$k: layout"
    [ "$(jq '.spans | length' "$OUT/records/$k.layout.json")" = "$windows" ] ||
      fail "$k ran $windows windows, not its rule's $(jq -c .spans "$OUT/records/$k.layout.json"): not the tree's code"
  fi
  if [ "$st" != completed ] || [ "$(jq -r .memoized <<< "$row")" = true ] || [ ! -s "$M/$k.mp4" ]; then
    say "FAIL $k: $st, memoized $(jq -r .memoized <<< "$row"), video $([ -s "$M/$k.mp4" ] && echo kept || echo none): $(jq -r '.reason.message // .reason // empty' "$OUT/records/$k.json" 2> /dev/null | head -c 400)"
    timeout 60 cozy machine logs "$NAME" tensord -n 200 > "$OUT/records/$k-tensord.log" 2>&1
    RC=1; FAILS=$((FAILS + 1)); [ "$FAILS" -lt "$MOST" ] || fail "$FAILS cells failed"; return 0
  fi
  # Every run on the pair asked: one that fell back to its lock's pair ends the session.
  jq -e --arg r "$WANT_RT" --arg t "$WANT_TFS" '.software | length > 0 and all(.[]; .[0] == $r and .[1] == $t)' <<< "$row" > /dev/null ||
    fail "$k ran on $(jq -c .software <<< "$row"), not Runtime $WANT_RT TensorFS $WANT_TFS"
}
for c in $CELLS; do cell "$c"; done

# The pod ends here: nothing below needs it.
for r in "${RIDS[@]}"; do
  until gone "$r"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1; sleep 10; done
  say "GONE $r; billed $(billed "$r")"
done
column -t -s $'\t' "$OUT/cells.tsv" | tee -a "$R"

# Each pair, on the runner: one window against windows, and each against the plate.
echo '[]' > "$OUT/pairs-in.json"
for res in 1440 1080 2160; do
  [ -s "$M/one-$res.mp4" ] && [ -s "$M/r-$res.mp4" ] && [ -s "$OUT/records/r-$res.layout.json" ] || continue
  clip=$(clip_of "r-$res")
  jq --arg p "${res}p" --arg plate "$M/$clip.mp4" --arg one "$M/one-$res.mp4" --arg r "$M/r-$res.mp4" --slurpfile l "$OUT/records/r-$res.layout.json" \
    '. + [{pair: $p, plate: $plate, one: $one, r: $r, spans: $l[0].spans}]' "$OUT/pairs-in.json" > "$OUT/pairs-in.next" && mv "$OUT/pairs-in.next" "$OUT/pairs-in.json"
done
if [ "$(jq length "$OUT/pairs-in.json")" -gt 0 ]; then
  nice -n 19 uv run -q --no-project --with numpy --with matplotlib python -I "$FIX/analyze.py" "$FRAMES_PY" "$OUT" "$OUT/pairs-in.json" \
    2> "$OUT/analyze.err" | tee -a "$R" || { say "analysis: $(tail -c 600 "$OUT/analyze.err")"; RC=1; }
else
  say "no pair completed"; RC=1
fi
exit "$RC"
