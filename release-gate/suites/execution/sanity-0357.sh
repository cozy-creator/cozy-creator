# Runtime 0.35.7 + TensorFS 0.17.14 + cozy 0.13.16 pre-roll sanity, one GPU (release lane, proof-only). Sourced by gate.sh
# --cells after the selected product cells (h3 on its fixed request, h3-1440p, sdxl, qwen) from the cohort packages tree:
# the loop cells, Krea 2 with a caller LoRA, the H3 receipt, minimax-h3 from the packages tree (a guide video, a video
# reference against released 0.1.13 on one seed, a long_form motion segment), then the same pod on production Runtime
# 0.35.6 / TensorFS 0.17.13 after `machine update`: H3, Krea and MOSS-Image again from the same tree, against the candidate's.
. "$HERE/suites/execution/h3-route.sh"
. "$HERE/suites/execution/core-cells.sh"
ex() { jq -r '.execution_ms // 0 | . / 1000 | . * 10 | round / 10' "$1.json" 2>/dev/null; }
sha() { f=$(output "$1" mp4); [ -n "$f" ] && sha256sum "$f" | cut -c1-16; }
KR=$(pkg krea-2 generate) KRT=$(pkg krea-2 generate_turbo) HT=$(pkg minimax-h3 ref2va_turbo) MO=$(pkg moss-image generate)
cell krea "$KR" prompt="$SCENE, photo" seed:=$((R + 381))
cell krea-turbo-lora "$KRT" prompt="$OBJECT, ink wash painting" seed:=$((R + 383)) \
  --lora "fidika/krea-2-darkbrush@sha256:95afffbed63209deecd40744849fa42e8ed61aaada1e52b09c033d447d428ed0,1.0"
if [ -f h3.json ]; then
  expect h3-route "$([ "$(run_get "$OUT" h3 .status)" = completed ] && echo PASS || echo FAIL)" \
    "$(run_get "$OUT" h3 .status); LoRA $(lora h3); served $(served h3); exact $(exact_sites h3); vae $(vae h3); decode $(decode_s h3) s"
fi
H3REF="[{\"name\": \"$NAME\", \"kind\": \"character\", \"image\": \"$REF\"}]"
cell h3-new-2 "$HT" references:="$H3REF" seed:=$((R + 391)) duration_s:=15 resolution=768p prompt="<$NAME> $MOTION Wind picks up."
# #706: the guide clip, a video reference on 0.1.8 and on the candidate, and a long_form motion segment.
GD=$OUT/guide GU=https://github.com/cozy-creator/cozy-creator/releases/download/proof-h3-guide-20261029
mkdir -p "$GD" && for f in head-swap.json face.png source.mp4 align.py; do curl -sfLo "$GD/$f" "$GU/$f" || rm -f "$GD/head-swap.json"; done
if [ -f "$GD/head-swap.json" ]; then
  jq --arg g "$GD" '.references |= map(if .image then .image = ($g + "/" + (.image | ltrimstr("./"))) else . end
    | if .video then .video = ($g + "/" + (.video | ltrimstr("./"))) else . end)' "$GD/head-swap.json" > "$GD/head-swap-abs.json"
  cell h3-guide "$HT" --input "$GD/head-swap-abs.json" --lane int8 \
    --lora "fidika/minimax-h3-head-swap@sha256:2adf611eee7e5f94e9de733b9dd62734058a4664d4ea5ca686c116045178af37,1.0"
  out=$(output h3-guide mp4)
  a=$([ -n "$out" ] && uv run -q --no-project --with numpy python -I "$GD/align.py" "$GD/source.mp4" "guide=$out" 2>&1 | tail -1)
  s=$(grep -oE 'same-index mean [0-9.]+' <<< "$a" | grep -oE '[0-9.]+$') w=$(grep -oE 'in [0-9]+% of sampled' <<< "$a" | grep -oE '[0-9]+')
  expect h3-guide "$([ "$(run_get "$OUT" h3-guide .status)" = completed ] && awk -v s="${s:-0}" -v w="${w:-0}" 'BEGIN{exit !(s >= 0.7 && w >= 90)}' && echo PASS || echo FAIL)" \
    "$(run_get "$OUT" h3-guide .status) $(ex h3-guide) s; $a"
  VR="[{\"name\": \"Subject 1\", \"kind\": \"character\", \"image\": \"$GD/face.png\"}, {\"name\": \"Video 1\", \"kind\": \"video\", \"video\": \"$GD/source.mp4\"}]"
  VP="<Subject 1>'s motion comes from <Video 1>. <Subject 1> turns his head left, then right, on a quiet city sidewalk."
  cell h3-video-013 "minimax-h3/ref2va_turbo@0.1.13" references:="$VR" clean_references:=false describe_references:=false seed:=515452 duration_s:=5 prompt="$VP" --lane int8
  cell h3-video-new "$HT" references:="$VR" clean_references:=false describe_references:=false seed:=515452 duration_s:=5 prompt="$VP" --lane int8
  a=$(sha h3-video-013) b=$(sha h3-video-new)
  expect h3-video-ref "$([ "$(run_get "$OUT" h3-video-new .status)" = completed ] && [ "$(run_get "$OUT" h3-video-013 .status)" = completed ] && echo PASS || echo FAIL)" \
    "0.1.13 $(ex h3-video-013) s $a, candidate $(ex h3-video-new) s $b: $([ -n "$a" ] && [ "$a" = "$b" ] && echo byte-identical || echo differ)"
  jq -n --argjson r "$VR" '{references: $r, clean_references: false, describe_references: false, segments: [
      {duration_s: 5, seed: 515453, prompt: "subject_definitions:\n<Subject 1>'"'"'s motion comes from <Video 1>.\n\ndetailed_description:\n[Shot 1] <Subject 1> turns his head left, then right, on a quiet city sidewalk."},
      {duration_s: 5, seed: 515454, prompt: "Cut to <Subject 1> looking into the camera and smiling, the street behind him."}]}' > "$GD/long-form.json"
  cell h3-long-form "$(pkg minimax-h3 long_form)" --input "$GD/long-form.json" --lane int8
  expect h3-long-form "$([ "$(run_get "$OUT" h3-long-form .status)" = completed ] && echo PASS || echo FAIL)" "$(run_get "$OUT" h3-long-form .status) $(ex h3-long-form) s"
fi
cell moss-new "$MO" prompt="$CHAR, studio shot" seed:=$((R + 395)) megapixels:=4.0 steps:=50
for i in 1 2 3; do cell "krea-new-$i" "$KRT" prompt="$OBJECT, Krea call $i, studio photo" seed:=$((R + 400 + i)); done
# The previous release on this pod: Runtime 0.35.6 / TensorFS 0.17.13 from PyPI.
PREV_RT=https://files.pythonhosted.org/packages/ea/c8/93e9be4c5f4922f4b93ddd682bcaba51a1942b008655fecb6f02c8d9e291/cozy_runtime-0.35.6-cp312-abi3-manylinux_2_28_x86_64.whl
PREV_TFS=https://files.pythonhosted.org/packages/d1/1b/a367336be6c08f197ada509467c35d3c409c42b65ed137bc154c06782bcd/tensorfs-0.17.13-cp311-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl
if [ -z "$ONLY" ] || [[ $ONLY == *",prev,"* ]]; then
  mkdir -p prev && curl -sfLo "prev/${PREV_RT##*/}" "$PREV_RT" && curl -sfLo "prev/${PREV_TFS##*/}" "$PREV_TFS" \
    && echo "36007f33d132a67ddcf31251bc83e2a721c9ff3ce3a0f32380a4ca5fdee0c6fd  prev/${PREV_RT##*/}" | sha256sum -c --quiet \
    && echo "d809cad9d6784f4f491f6b1843a35b26a94a4f7b4310c234ae9a3bc194dd3170  prev/${PREV_TFS##*/}" | sha256sum -c --quiet \
    && "$CLI" machine update "$M" --runtime "prev/${PREV_RT##*/}" --tensorfs "prev/${PREV_TFS##*/}" -y > prev-update.log 2>&1
  prev=$("$CLI" --json machine show "$M" 2>/dev/null | jq -r '"\(.status.runtime)/\(.status.tensorfs)"')
  cell h3-prev-1 "$HT" references:="$H3REF" seed:=$((R + 392)) duration_s:=15 resolution=768p prompt="<$NAME> $MOTION Rain starts."
  cell h3-prev-2 "$HT" references:="$H3REF" seed:=$((R + 393)) duration_s:=15 resolution=768p prompt="<$NAME> $MOTION Night falls."
  cell moss-prev "$MO" prompt="$CHAR, studio shot on the last release" seed:=$((R + 396)) megapixels:=4.0 steps:=50
  for i in 1 2 3; do cell "krea-prev-$i" "$KRT" prompt="$OBJECT, Krea call $i on the last release, studio photo" seed:=$((R + 410 + i)); done
  expect release-ab "$([ "$prev" = 0.35.6/0.17.13 ] && echo PASS || echo FAIL)" \
    "H3 candidate $(ex h3)/$(ex h3-new-2) s vs $prev $(ex h3-prev-1)/$(ex h3-prev-2) s; moss candidate $(ex moss-new) s vs $(ex moss-prev) s (steady in the summary); Krea candidate $(ex krea-new-1)/$(ex krea-new-2)/$(ex krea-new-3) s vs $(ex krea-prev-1)/$(ex krea-prev-2)/$(ex krea-prev-3) s"
fi
