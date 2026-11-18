# h3-edit-ab (se-163, proof only): packages #1005's speed and memory on one RTX 4090 against the RC, the same H3 edit on each,
# and the why of any difference. Packages from packages_refs checkouts: `rc` (rc/0.27.0, minimax-h3 0.16.0rc6), `pk` (#1005,
# 0.16.0rc7) and `pg` (a probe of pk: its guide held only to the next 17n+5 at or above the plate, not to the sample's end).
# The edit: NASA's "Astronaut Scott Kelly Speaks Out Against Bullying" (public domain) from 6.4 s for 12.6 s, 303 slots
# at 24 fps, Sintel (CC BY 3.0, Blender Foundation) swapped in, character swap, Turbo, seed 163163, every field at its
# default (so rc7 regenerates its sound: it hears the plate's; rc6 can't). One 13 s Ref2VA call, 328 frames sampled.
# Before renting, at $0, for each tree in its own locked env: the Runtime and TensorFS it resolves (both arms must match),
# and what its own code guides (fixtures/h3-edit-sound/guided.py: the guide read and the frames its encode keeps); then
# each tree's edit described through cozy, and its Triton sets built.
# Cells, in order (params.cells; every run names its machine, `--on`):
#   warm        pk on the first 5 s of the plate: fetches every weight, never compared
#   a1 b1 b2 a2 rc, pk, pk, rc: each its own arm (derived state reset, TensorD restarted, uncapped)
#   c1 c2       pg (optional): its own arms too
#   cap         pk under an 8 GiB VRAM cap (params.vram_cap): it must complete
# Per run (results.txt, ab.tsv): exec, conditioning wall (the guide read and encoded), each denoise step (step 1, the dense
# 2-4 and the sparse 5-8 means, steady 2-8), decode, peak VRAM (driver and allocated), SM clock and throttle share, and its
# Runtime and TensorFS: a run on another pair than the first's fails the session. A1/A2 and B1/B2 are checked for the same
# frames (one build, one seed). The pod ends before the summary.
# params: runtime (the Runtime wheel, https://…#sha256=<hex>), cells, vram_cap (8GiB), need_s (7800). (`cap` is lib.sh's dollar cap.)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures/h3-edit-sound" && pwd) || fail "no fixtures"
CELLS=$(param cells "warm a1 b1 b2 a2 cap") VCAP=$(param vram_cap 8GiB)
[[ $VCAP =~ ^[0-9]+GiB$ ]] || fail "vram_cap $VCAP"
declare -A TREE=([warm]=pk [a1]=rc [a2]=rc [b1]=pk [b2]=pk [c1]=pg [c2]=pg [cap]=pk)
for c in $CELLS; do [ -n "${TREE[$c]:-}" ] || fail "no cell $c"; done
for t in $(for c in $CELLS; do echo "${TREE[$c]}"; done | sort -u); do
  [ -f "$PACKAGES/$t/minimax-h3/pyproject.toml" ] || fail "no packages_refs checkout $t"
done
for tool in ffmpeg ffprobe curl jq uv; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh  # derived_state reads and clears the pod's derived state over its root shell
URL=$(param runtime)
[[ $URL == https://*'#sha256='* ]] || fail "runtime is not https://…#sha256=<hex>"
mkdir -p "$OUT/wheel" "$OUT/media"
W=$OUT/wheel/$(basename "${URL%%#*}" | sed 's/%2B/+/g')
curl -fsSL --retry 5 --retry-all-errors "${URL%%#*}" -o "$W" || fail "cannot fetch the runtime wheel"
[ "$(sha256sum "$W" | cut -c1-64)" = "${URL##*sha256=}" ] || fail "runtime wheel sha256"
WANT_RT=$(basename "$W" | cut -d- -f2)

M=$OUT/media
fetch() { curl -fsSL --retry 5 -A "cozy-release-gate/1.0" "$2" -o "$M/$1" || fail "fetch $1"; }
fetch kelly.webm "https://upload.wikimedia.org/wikipedia/commons/c/ce/Astronaut_Scott_Kelly_Speaks_Out_Against_Bullying.webm"
fetch sintel-cover.jpg "https://upload.wikimedia.org/wikipedia/commons/6/69/Sintel_Cover_Durian_Project.jpg"
ffmpeg -v error -y -ss 6.4 -i "$M/kelly.webm" -t 12.6 -c:v libx264 -crf 18 -pix_fmt yuv420p -c:a aac -b:a 160k \
  -movflags +faststart "$M/kelly.mp4" || fail "kelly plate"
ffmpeg -v error -y -ss 6.4 -i "$M/kelly.webm" -t 5 -c:v libx264 -crf 18 -pix_fmt yuv420p -c:a aac -b:a 160k \
  -movflags +faststart "$M/kelly5.mp4" || fail "the 5 s plate"
ffmpeg -v error -y -i "$M/sintel-cover.jpg" -vf "crop=748:1000:330:250" -q:v 2 "$M/sintel.jpg" || fail "sintel"
say "plate: $(ffprobe -v error -show_entries stream=codec_name,r_frame_rate,duration -of compact=p=0:nk=1 "$M/kelly.mp4" | paste -sd' ')"

# Each tree's locked env (Torch beside it as the worker image has it, on CPU): its Runtime and TensorFS, and its guide.
echo '{}' > "$OUT/guided.json"
for t in $(for c in $CELLS; do echo "${TREE[$c]}"; done | sort -u); do
  E=$OUT/env-$t
  { (cd "$PACKAGES/$t/minimax-h3" && UV_PROJECT_ENVIRONMENT=$E uv sync -q --frozen --no-dev --python 3.12) &&
    VIRTUAL_ENV=$E uv pip install -q --index-url https://download.pytorch.org/whl/cpu torch==2.15.0 torchvision==0.29.2; } \
    > "$OUT/env-$t.log" 2>&1 || fail "$t's env: $(tail -c 500 "$OUT/env-$t.log")"
  g=$(cd "$PACKAGES/$t/minimax-h3" && "$E/bin/python" -I "$FIX/guided.py" 2> "$OUT/guided-$t.err") || fail "$t's guide: $(tail -c 500 "$OUT/guided-$t.err")"
  say "tree $t ($(git -C "$PACKAGES/$t" rev-parse --short HEAD)): $g"
  jq --arg t "$t" --argjson g "$g" '. + {($t): $g}' "$OUT/guided.json" > "$OUT/guided.next" && mv "$OUT/guided.next" "$OUT/guided.json"
  rm -rf "$E"
done
[ "$(jq -r '[.[] | "\(.runtime) \(.tensorfs)"] | unique | length' "$OUT/guided.json")" = 1 ] ||
  fail "the trees resolve different Runtime/TensorFS pairs: $(jq -c 'map_values({runtime, tensorfs})' "$OUT/guided.json")"
[ "$(jq -r '[.[] | .runtime] | first' "$OUT/guided.json")" = "$WANT_RT" ] ||
  fail "the trees lock Runtime $(jq -r '[.[] | .runtime] | first' "$OUT/guided.json"), not the wheel's $WANT_RT"
for t in $(for c in $CELLS; do echo "${TREE[$c]}"; done | sort -u); do
  (cd "$RUN_PROJECT" && cozy run "$PACKAGES/$t/minimax-h3/edit" --describe) > "$OUT/describe-$t.txt" 2>&1 ||
    fail "$t's edit doesn't describe: $(tail -c 400 "$OUT/describe-$t.txt")"
  say "tree $t: edit described ($(grep -c -i -E 'audio' "$OUT/describe-$t.txt") lines name audio)"
  kernels "$PACKAGES/$t/minimax-h3"
done

quote "$(param need_s 7800)"
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$W" -y || fail "update"
say "pod $(cozy machine show "$NAME" --json | jq -c '{v: .status.version, rt: .status.runtime, tfs: .status.tensorfs, gpu: .status.gpus[0].name}')"

run_page import "$M/kelly.mp4" --name kelly > "$OUT/import-kelly.txt" 2>&1 || fail "import kelly: $(cat "$OUT/import-kelly.txt")"
run_page import "$M/kelly5.mp4" --name kelly5 > "$OUT/import-kelly5.txt" 2>&1 || fail "import kelly5: $(cat "$OUT/import-kelly5.txt")"
run_page import "$M/sintel.jpg" --name sintel --description "Sintel, a young woman with short red hair, a grey laced top and leather arm wraps, a small dragon on her shoulder" \
  > "$OUT/import-sintel.txt" 2>&1 || fail "import sintel: $(cat "$OUT/import-sintel.txt")"
SWAP='<sintel> takes the place of the man in the plate, keeping its scene, camera and his whole performance: pose, gestures, head motion, gaze, speech, lip movement and expressions.'
BEAT='<sintel> speaks to the camera in front of the rocket.'
newest_run() { run_page asset "$1" | jq -r '.versions[-1].runs[-1]'; }
file_of() {
  local f t=0
  f=$(run_page asset "$1" | jq -r '.versions[-1].files.video.path // empty')
  [ -n "$f" ] || return 1
  until [ -s "$RUN_PROJECT/$f" ] || [ "$t" -ge 900 ]; do sleep 2; t=$((t + 2)); done
  echo "$RUN_PROJECT/$f"
}
# arm <cell> <gpu memory limit>: a fresh arm (lib.sh derived_state; ab.sh's Arms): the derived state goes, the update restarts
# TensorD with the limit, and what the ended executors still wrote is settled; nothing derived may be left.
arm() {
  derived_state "$1" reset
  step "arm-$1" machine update "$NAME" --gpu-memory-limit "$2" -y || fail "$1: the restart"
  derived_state "$1" settle
  derived_zero || fail "$1: its arm starts with derived state it did not make: $DERIVED"
  answers "$1"
  say "$1: arm at $2, pod Runtime $(cozy machine show "$NAME" --json | jq -r .status.runtime), TensorFS $(cozy machine show "$NAME" --json | jq -r .status.tensorfs)"
}
# answers <label>: wait (10 min at most) until the pod answers this runner's Calcifer: in 44289620346 a render 4 s after a
# restart couldn't install its code ("bickslow didn't answer … Connection refused", machine_unreachable) for 3 min.
answers() {
  local t0=$SECONDS
  until timeout 60 cozy model ls --on "$NAME" --json > "$OUT/answers-$1.json" 2>&1; do
    [ $((SECONDS - t0)) -lt 600 ] || { podlog "$1-unanswered"; fail "$1: $NAME hasn't answered in 10 min: $(tail -c 300 "$OUT/answers-$1.json")"; }
    sleep 15
  done
  say "$1: $NAME answers ($((SECONDS - t0)) s)"
}
# podlog <label>: the pod's TensorD log, its last 300 lines (bounded, agent-rules).
podlog() { timeout 60 cozy machine logs "$NAME" tensord -n 300 > "$OUT/$1-tensord.log" 2>&1 || true; }
FAILS=0
printf 'cell\ttree\tstatus\texec_s\tconditioning_s\tstep1_s\tdense_s\tsparse_s\tsteady_s\tdenoise_s\tdecode_s\tpeak_driver_gib\tpeak_alloc_gib\tsm_mhz\tthrottled\tframes\truntime\ttensorfs\n' > "$OUT/ab.tsv"
FIRST=''
# cell <name>: the edit on its tree, rendered on the pod; its record, log, video and numbers kept.
cell() {
  local k=$1 t=${TREE[$1]} plate=kelly fields n v t0=$SECONDS
  [ "$k" != warm ] || plate=kelly5
  fields=$(jq -cn --arg p "<$plate>" --arg s "$SWAP" --arg b "$BEAT" \
    '{plate: $p, edit: "character_swap", summary: $s, seed: 163163, beats: [{duration: 15, description: $b}]}')
  run_page add "$k" --tool "$PACKAGES/$t/minimax-h3/edit" --fields "$fields" > "$OUT/add-$k.txt" 2>&1 || fail "add $k: $(cat "$OUT/add-$k.txt")"
  say "render $k ($t, $plate)"
  if ! run_page render --on "$NAME" "$k" > "$OUT/render-$k.txt" 2>&1; then
    podlog "$k"
    if grep -q -E 'machine_unreachable|didn.t answer' "$OUT/render-$k.txt"; then
      say "$k: $NAME unreachable ($(grep -o -m1 -E 'hasn.t answered[^)]*' "$OUT/render-$k.txt")); once more when it answers"
      answers "$k-again"
      run_page render --on "$NAME" "$k" > "$OUT/render-$k-again.txt" 2>&1 || { podlog "$k-again"; say "FAIL $k again: $(tail -c 600 "$OUT/render-$k-again.txt")"; }
    else
      say "FAIL $k: $(tail -c 600 "$OUT/render-$k.txt")"
    fi
  fi
  n=$(newest_run "$k")
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$k.json" 2>&1
  (cd "$RUN_PROJECT" && cozy --full run watch "$n" --once) > "$OUT/$k.log" 2>&1
  v=$(file_of "$k") && cp "$v" "$OUT/$k.mp4"
  local row frames
  frames=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$OUT/$k.mp4" 2>/dev/null)
  row=$(jq -r --arg k "$k" --arg t "$t" --arg f "${frames:-?}" --arg cw "$(LC_ALL=C grep -a -o -m1 -E 'h3 conditioning wall_ms=[0-9.]+' "$OUT/$k.log" | cut -d= -f2)" '
    (.calls[0].measurements // {}) as $m | ($m.attribution.steps.denoise.device.series // []) as $s | ($m.memory.driver // [{}])[0] as $d
    | def avg(a): if (a | length) > 0 then (a | add / length / 1000 * 10 | round / 10) else null end;
    [$k, $t, (.status // "none"), ((.execution_ms // 0) / 1000 | floor), (($cw | tonumber? // 0) / 100 | round / 10),
     (($s[0] // 0) / 100 | round / 10), avg($s[1:4]), avg($s[4:8]), avg($s[1:]),
     (($m.attribution.stages.denoise.total_ms // 0) / 1000 | floor), (($m.attribution.stages.decode_video.total_ms // 0) / 100 | round / 10),
     (($d.peak_bytes // 0) / 107374182.4 | floor / 10), (($m.memory.peak_allocated_bytes // 0) / 107374182.4 | floor / 10),
     ($d.sm_mhz_mean // 0 | floor), ($d.throttled_share // 0 | . * 100 | round / 100), $f,
     (.measurements.software.runtime // "?"), (.measurements.software.tensorfs // "?")] | @tsv' "$OUT/$k.json" 2>/dev/null)
  [ -n "$row" ] || row="$k	$t	none"
  echo "$row" >> "$OUT/ab.tsv"
  say "$k: run $n in $((SECONDS - t0)) s: $(tr '\t' ' ' <<< "$row")"
  say "$k: $(LC_ALL=C grep -a -o -m1 -E 'continuation window [^"]*' "$OUT/$k.log") $(LC_ALL=C grep -a -o -m1 -E 'h3 output geometry denoise_rows=[0-9]+' "$OUT/$k.log")"
  # A cell that didn't complete is counted (two end the session); one that did must run on the first run's pair.
  if [ "$(jq -r '.status // "none"' "$OUT/$k.json" 2>/dev/null)" != completed ]; then
    RC=1; FAILS=$((FAILS + 1)); [ "$FAILS" -lt 2 ] || fail "$FAILS cells failed"; return 0
  fi
  # Both arms on one pair: a run that fell back to another Runtime or TensorFS than the first's ends the session.
  local pair; pair=$(jq -r '"\(.measurements.software.runtime // "?") \(.measurements.software.tensorfs // "?")"' "$OUT/$k.json" 2>/dev/null)
  [ -n "$FIRST" ] || FIRST=$pair
  [ "$pair" = "$FIRST" ] || fail "$k ran on Runtime/TensorFS $pair, not the first run's $FIRST"
  [ "${pair%% *}" = "$WANT_RT" ] || fail "$k ran on Runtime ${pair%% *}, not the candidate $WANT_RT"
}
for c in $CELLS; do
  case $c in
    warm) ;;
    cap) arm "$c" "$VCAP" ;;
    *) arm "$c" off ;;
  esac
  cell "$c"
done

# The pod ends here: nothing below needs it.
for r in "${RIDS[@]}"; do
  until gone "$r"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1; sleep 10; done
  say "GONE $r; billed $(billed "$r")"
done
column -t -s $'\t' "$OUT/ab.tsv" | tee -a "$R"
md5() { ffmpeg -nostdin -v error -i "$OUT/$1.mp4" -map 0:v -f framemd5 - 2> /dev/null | grep -v '^#' | md5sum | cut -c1-16; }
for p in "a1 a2" "b1 b2" "c1 c2"; do
  set -- $p; [ -s "$OUT/$1.mp4" ] && [ -s "$OUT/$2.mp4" ] || continue
  say "$1 vs $2 (one build, one seed): frames $(md5 "$1") / $(md5 "$2")"
done
# Each build's steady step, dense and sparse, against rc's: the A/B's answer (Paul 11-15: within 5% is a tie).
jq -Rr -s --argjson g "$(cat "$OUT/guided.json")" '
  split("\n") | .[1:] | map(select(length > 0) | split("\t")) | map({cell: .[0], tree: .[1], status: .[2], steady: (.[8] | tonumber? ),
    dense: (.[6] | tonumber?), sparse: (.[7] | tonumber?), cond: (.[4] | tonumber?), decode: (.[10] | tonumber?), peak: (.[11] | tonumber?)})
  | map(select(.cell != "warm" and .cell != "cap" and .status == "completed")) | group_by(.tree)
  | map({tree: .[0].tree, runs: length, steady: (map(.steady) | add / length), dense: (map(.dense) | add / length),
         sparse: (map(.sparse) | add / length), cond: (map(.cond) | add / length), decode: (map(.decode) | add / length),
         peak: (map(.peak) | max), guided: $g[.[0].tree].guided, guided_latents: $g[.[0].tree].guided_latents})
  | (map(select(.tree == "rc")) | first) as $a
  | .[] | "\(.tree) (\(.runs) runs, guides \(.guided) frames = \(.guided_latents) latents): steady \(.steady * 10 | round / 10) s"
    + (if $a and .tree != "rc" then " (\((.steady / $a.steady - 1) * 1000 | round / 10)% vs rc)" else "" end)
    + ", dense \(.dense * 10 | round / 10), sparse \(.sparse * 10 | round / 10)"
    + (if $a and .tree != "rc" then " (\((.sparse / $a.sparse - 1) * 1000 | round / 10)%)" else "" end)
    + ", conditioning \(.cond) s, decode \(.decode) s, peak \(.peak) GiB"' "$OUT/ab.tsv" | tee -a "$R"
exit "$RC"
