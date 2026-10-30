# h3-8gb: the 8 GB tier. H3 Turbo 15 s (fl2va_turbo, PDD-8, 1344x768 with audio, int8) on one real 8 GB card must
# complete and stay near its time. First proof: pod-session 40557852009, RTX 3070, Runtime 0.34.7 with minimax-h3 at
# packages eb0eaf63: 2020 s (dense steps ~290 s, sparse ~159 s, decode 186 s; peak driver 8.21 GB). Production
# 0.34.5 / minimax-h3 0.1.2 ran out of memory in step 1 there, and so did ComfyUI. One rental (rtx-3070), driven from a
# runner. params:
#   target      the function: "<pkg>/<fn>[@release]" (minimax-h3/fl2va_turbo), or "tree:<name>" for that packages_refs
#               checkout's minimax-h3/fl2va_turbo
#   runtime     a candidate Runtime wheel, https://…#sha256=<hex>, installed with `cozy machine update` (optional)
#   prompt      the request (each session draws a new seed)
#   max_exec_s  the run's execution bar (2400: the proof's 2020 s and some host spread)
#   traffic_gb  GB the provider bills (85)
#   arms        [{"name", "runtime": https://…#sha256= or a release, "tensorfs": "artifact:<cozy-creator run>:<name>" or a release}]: one
#               request per arm on the same pod, each after its machine update (default: one arm of `runtime`)
#               "tensord": a cozy release or prerelease version for the machine's TensorD (`machine update --tensord`)
#   repeat      requests per arm, one after another on the arm's executor, each with a new seed (1)
#   max_gpu_gb  the card's size bound (9: an 8 GB card; 25 for a 4090 no-regression run)
#   hold_free_mib  on a larger card, a separate non-Cozy process holds VRAM so only this much stays free, as a user's
#               game would (lowvram's holder: its ComfyUI venv's torch, no models); 0 holds nothing
. "$(dirname "$0")/lib.sh"
T=$(param target minimax-h3/fl2va_turbo) URL=$(param runtime) MAX=$(param max_exec_s 2400)
ARMS=$(param arms "$(jq -cn --arg u "$URL" '[{name: "run", runtime: $u}]')") GPU_GB=$(param max_gpu_gb 9)
PROMPT=$(param prompt "integrated_multimodal_description: A steady tracking shot of an adult courier cycling a cargo bike through a narrow cobbled old-town street after rain, past cafe tables and a flower stall, then down a gentle slope into a sunlit square.

audio: Tires hissing on wet cobbles, a bicycle bell, cafe chatter and a distant tram.

non_diegetic_music: N/A")
if [[ $T == tree:* ]]; then
  T=$PACKAGES/${T#tree:}/minimax-h3/fl2va_turbo; [ -f "${T%/*}/h3.py" ] || fail "no packages checkout ${T%/*}"
fi
fetch() {  # fetch <arm> <kind>: the arm's wheel path ("" for none)
  local v d; v=$(jq -r --arg a "$1" --arg k "$2" '.[] | select(.name==$a) | .[$k] // empty' <<<"$ARMS"); [ -n "$v" ] || return 0
  [[ $v =~ ^[0-9][0-9a-z.+]*$ ]] && { echo "$v"; return 0; }  # a release the machine fetches itself
  d=$OUT/wheel/$1-$2; mkdir -p "$d"
  if [[ $v == artifact:* ]]; then
    IFS=: read -r _ rid name <<<"$v"
    gh run download "$rid" -R cozy-creator/cozy-creator -n "$name" -D "$d" >/dev/null 2>&1 || fail "no artifact $v"
  else
    [[ $v == https://*'#sha256='* ]] || fail "$v is not https://…#sha256=<hex>"
    local f; f=$(basename "${v%%#*}"); f=${f//%2B/+}
    curl -fsSL --retry 5 --retry-all-errors "${v%%#*}" -o "$d/$f" || fail "cannot fetch $v"
    [ "$(sha256sum "$d/$f" | cut -c1-64)" = "${v##*sha256=}" ] || fail "$v sha256"
  fi
  ls "$d"/*.whl | grep -m1 'manylinux.*x86_64'
}
mapfile -t NAMES < <(jq -r '.[].name' <<<"$ARMS")
declare -A RT TF
for a in "${NAMES[@]}"; do RT[$a]=$(fetch "$a" runtime) TF[$a]=$(fetch "$a" tensorfs); done
HOLD=$(param hold_free_mib 0)
if [ "$HOLD" -gt 0 ]; then
  L=$(cd "$(dirname "$0")" && pwd)/lowvram G=$OUT/gen; mkdir -p "$G"
  python3 - "$L/comfy_setup.py" "$L/comfy-requirements.lock" "$G/comfy_setup.py" <<'PY2' || fail "render comfy_setup"
import sys
src, lock, dst = sys.argv[1:]
s = open(src).read()
assert s.count("REQUIREMENTS = None  # @REQUIREMENTS@") == 1
open(dst, "w").write(s.replace("REQUIREMENTS = None  # @REQUIREMENTS@", "REQUIREMENTS = " + repr(open(lock).read())))
PY2
  cp "$L/tool.py" "$G/" && python3 -I -m py_compile "$G/comfy_setup.py" "$G/tool.py" || fail "holder scripts"
fi
command -v ffprobe >/dev/null || sudo apt-get install -y -qq ffmpeg >/dev/null 2>&1 || fail "no ffmpeg"
jq -n --arg p "$PROMPT" --argjson s "$RANDOM$RANDOM" '{prompt: $p, seed: $s, duration_s: 15}' > "$OUT/request.json"
quote "$(param traffic_gb 85)" 3600
rent

jq -e --argjson g "$GPU_GB" '.status.gpus | length == 1 and .[0].memory_bytes < $g * 1e9' "$OUT/show.json" >/dev/null ||
  fail "not one GPU under $GPU_GB GB"
if [ "$HOLD" -gt 0 ]; then
  ( cd "$G" && step holder-env run ./comfy_setup.py "models:=[]" --on "$NAME" --out "$OUT/holder-env" ) || fail "holder venv"
fi
hold() {  # hold <arm>: after each machine update (a restart can take the holder down), all but HOLD MiB held again
  [ "$HOLD" -gt 0 ] || return 0
  ( cd "$G" && step "hold-$1" run ./tool.py op=hold "free_mib:=$HOLD" --on "$NAME" --out "$OUT/hold-$1" ) || fail "hold $HOLD MiB"
  say "$1: holding all but $HOLD MiB: $(tail -c 300 "$OUT/hold-$1.txt" | tr '\n' ' ')"
}
step model model download _/minimax-h3@1.0.0-rc.6 --lane int8 --on "$NAME" || fail "model download"
step lora model download _/minimax-h3-turbo-lora@1.0.1 --on "$NAME" || fail "LoRA download"
FAILED="" REPEAT=$(param repeat 1)
for arm in "${NAMES[@]}"; do for i in $(seq 1 "$REPEAT"); do
  a=$arm; [ "$REPEAT" = 1 ] || a=$arm-$i
  flags=(); [ -z "${RT[$arm]}" ] || flags+=(--runtime "${RT[$arm]}"); [ -z "${TF[$arm]}" ] || flags+=(--tensorfs "${TF[$arm]}")
  td=$(jq -r --arg a "$arm" '.[] | select(.name==$a) | .tensord // empty' <<<"$ARMS"); [ -z "$td" ] || flags+=(--tensord "$td")
  [ "${#flags[@]}" -eq 0 ] || [ "$i" -gt 1 ] || step "update-$arm" machine update "$NAME" "${flags[@]}" -y || fail "update $arm"
  [ "$i" -gt 1 ] || hold "$arm"
  say "$a: pod $(cozy machine show "$NAME" --json | jq -c '{rt: .status.runtime, tfs: .status.tensorfs, gpu: .status.gpus[0].name}')"
  jq --argjson s "$RANDOM$RANDOM" '.seed = $s' "$OUT/request.json" > "$OUT/request-$a.json"
  step "h3-$a" run "$T" --input "$OUT/request-$a.json" --lane int8 --gpus=1 --on "$NAME" --out "$OUT/media-$a"
  N=$(runno "h3-$a"); [ -n "$N" ] || { FAILED+=" $a"; continue; }
  cozy run show "$N" --once --json > "$OUT/h3-$a.json" 2>&1
  cozy run show "$N" --once > "$OUT/h3-$a.log" 2>&1
  say "$a: $(jq -r '([.calls[] | select(.function | startswith("generate_")) | .measurements] | last) as $m |
    "run \(.number) \(.status), exec \((.execution_ms // 0) / 1000 | floor) s; steps \([$m.attribution.steps.denoise.series[]?[1] / 1000 | floor]),
    decode \(($m.attribution.stages.decode_video.total_ms // 0) / 1000 | floor) s; peak driver \(([$m.memory.driver[]?.peak_bytes] | max // 0) / 1e9 * 100 | floor / 100) GB;
    host tier \([$m.memory.stats.varena[]?.stats.baked_pages | {pages, gb: (.bytes / 1e9 * 100 | floor / 100), pinned}]);
    moved store \(([$m.memory.stats.varena[]?.stats.moved.store.bytes] | add // 0) / 1e9 | floor) GB host \(([$m.memory.stats.varena[]?.stats.moved.baked.bytes] | add // 0) / 1e9 | floor) GB"' "$OUT/h3-$a.json" | tr -s ' \n' ' ')"
  [ "$(jq -r .status "$OUT/h3-$a.json")" = completed ] || { FAILED+=" $a"; continue; }
  V=$(jq -r '[.output[]?.path | select(endswith(".mp4"))][0] // empty' "$OUT/h3-$a.json")
  F=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$V" 2>/dev/null)
  [ "$F" = 362 ] || { say "$a: video decodes to ${F:-no} frames, not 362"; FAILED+=" $a"; continue; }
  S=$(jq '(.execution_ms // 0) / 1000 | floor' "$OUT/h3-$a.json")
  [ "$S" -le "$MAX" ] || { say "$a: $S s, over $MAX s"; FAILED+=" $a"; continue; }
  say "PASS $a: H3 Turbo 15 s in $S s (bar $MAX s), 362 frames"
done; done
[ -z "$FAILED" ] || fail "failed:$FAILED"
