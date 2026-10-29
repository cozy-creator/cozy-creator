# Runtime 0.35.4 + TensorFS 0.17.11 + cozy 0.13.13 pre-roll sanity, one GPU (release lane, proof-only). Sourced by gate.sh
# --cells after the selected product cells (h3 on its fixed request, h3-1440p, sdxl, qwen) from the cohort packages tree:
# the loop cells, Krea 2 with a caller LoRA, the H3 receipt, the cohort minimax-h3 (a guide video, a video reference
# against released 0.1.10 on one seed, a long_form motion segment), then the same pod on Runtime 0.35.2 (TensorFS
# unchanged) after `machine update`: H3, Krea and MOSS-Image again from the same tree, against the candidate's.
. "$HERE/suites/execution/h3-route.sh"
. "$HERE/suites/execution/core-cells.sh"
ex() { jq -r '.execution_ms // 0 | . / 1000 | . * 10 | round / 10' "$1.json" 2>/dev/null; }
sha() { f=$(output "$1" mp4); [ -n "$f" ] && sha256sum "$f" | cut -c1-16; }
KR=$(pkg krea-2 generate) KRT=$(pkg krea-2 generate_turbo) HT=$(pkg minimax-h3 ref2va_turbo) MO=$(pkg moss-image generate)
cell krea "$KR" prompt="$SCENE, photo" seed:=$((R + 381))
cell krea-turbo-lora "$KRT" prompt="$OBJECT, ink wash painting" seed:=$((R + 383)) \
  --lora "model:transformer=fidika/krea-2-darkbrush@sha256:95afffbed63209deecd40744849fa42e8ed61aaada1e52b09c033d447d428ed0,1.0"
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
    --lora "generate_ref2va_turbo.models.base_model:transformer_ref=fidika/minimax-h3-head-swap@sha256:2adf611eee7e5f94e9de733b9dd62734058a4664d4ea5ca686c116045178af37,1.0"
  out=$(output h3-guide mp4)
  a=$([ -n "$out" ] && uv run -q --no-project --with numpy python -I "$GD/align.py" "$GD/source.mp4" "guide=$out" 2>&1 | tail -1)
  s=$(grep -oE 'same-index mean [0-9.]+' <<< "$a" | grep -oE '[0-9.]+$') w=$(grep -oE 'in [0-9]+% of sampled' <<< "$a" | grep -oE '[0-9]+')
  expect h3-guide "$([ "$(run_get "$OUT" h3-guide .status)" = completed ] && awk -v s="${s:-0}" -v w="${w:-0}" 'BEGIN{exit !(s >= 0.7 && w >= 90)}' && echo PASS || echo FAIL)" \
    "$(run_get "$OUT" h3-guide .status) $(ex h3-guide) s; $a"
  VR="[{\"name\": \"Subject 1\", \"kind\": \"character\", \"image\": \"$GD/face.png\"}, {\"name\": \"Video 1\", \"kind\": \"video\", \"video\": \"$GD/source.mp4\"}]"
  VP="<Subject 1>'s motion comes from <Video 1>. <Subject 1> turns his head left, then right, on a quiet city sidewalk."
  cell h3-video-010 "minimax-h3/ref2va_turbo@0.1.10" references:="$VR" clean_references:=false describe_references:=false seed:=515252 duration_s:=5 prompt="$VP" --lane int8
  cell h3-video-new "$HT" references:="$VR" clean_references:=false describe_references:=false seed:=515252 duration_s:=5 prompt="$VP" --lane int8
  a=$(sha h3-video-010) b=$(sha h3-video-new)
  expect h3-video-ref "$([ "$(run_get "$OUT" h3-video-new .status)" = completed ] && [ "$(run_get "$OUT" h3-video-010 .status)" = completed ] && echo PASS || echo FAIL)" \
    "0.1.10 $(ex h3-video-010) s $a, candidate $(ex h3-video-new) s $b: $([ -n "$a" ] && [ "$a" = "$b" ] && echo byte-identical || echo differ)"
  jq -n --argjson r "$VR" '{references: $r, clean_references: false, describe_references: false, segments: [
      {duration_s: 5, seed: 515253, prompt: "subject_definitions:\n<Subject 1>'"'"'s motion comes from <Video 1>.\n\ndetailed_description:\n[Shot 1] <Subject 1> turns his head left, then right, on a quiet city sidewalk."},
      {duration_s: 5, seed: 515254, prompt: "Cut to <Subject 1> looking into the camera and smiling, the street behind him."}]}' > "$GD/long-form.json"
  cell h3-long-form "$(pkg minimax-h3 long_form)" --input "$GD/long-form.json" --lane int8
  expect h3-long-form "$([ "$(run_get "$OUT" h3-long-form .status)" = completed ] && echo PASS || echo FAIL)" "$(run_get "$OUT" h3-long-form .status) $(ex h3-long-form) s"
fi
cell moss-new "$MO" prompt="$CHAR, studio shot" seed:=$((R + 395)) megapixels:=4.0 steps:=50
for i in 1 2 3; do cell "krea-new-$i" "$KRT" prompt="$OBJECT, Krea call $i, studio photo" seed:=$((R + 400 + i)); done
# The previous release on this pod: Runtime 0.35.2 from PyPI; TensorFS 0.17.11 is the release's already.
PREV_RT=https://files.pythonhosted.org/packages/b0/a0/b3c8fbedd3e28c9023e9747a7943f92e70e9ef9d398724c27c2a420f62e9/cozy_runtime-0.35.2-cp312-abi3-manylinux_2_28_x86_64.whl
if [ -z "$ONLY" ] || [[ $ONLY == *",prev,"* ]]; then
  mkdir -p prev && curl -sfLo "prev/${PREV_RT##*/}" "$PREV_RT" \
    && echo "f4d8e9639a031130ca71cf0cab9c40240a92841ad8c1eb929583fd5a7220c352  prev/${PREV_RT##*/}" | sha256sum -c --quiet \
    && "$CLI" machine update "$M" --runtime "prev/${PREV_RT##*/}" -y > prev-update.log 2>&1
  prev=$("$CLI" --json machine show "$M" 2>/dev/null | jq -r '"\(.status.runtime)/\(.status.tensorfs)"')
  cell h3-prev-1 "$HT" references:="$H3REF" seed:=$((R + 392)) duration_s:=15 resolution=768p prompt="<$NAME> $MOTION Rain starts."
  cell h3-prev-2 "$HT" references:="$H3REF" seed:=$((R + 393)) duration_s:=15 resolution=768p prompt="<$NAME> $MOTION Night falls."
  cell moss-prev "$MO" prompt="$CHAR, studio shot on the last release" seed:=$((R + 396)) megapixels:=4.0 steps:=50
  for i in 1 2 3; do cell "krea-prev-$i" "$KRT" prompt="$OBJECT, Krea call $i on the last release, studio photo" seed:=$((R + 410 + i)); done
  expect release-ab "$([ "$prev" = 0.35.2/0.17.11 ] && echo PASS || echo FAIL)" \
    "H3 candidate $(ex h3)/$(ex h3-new-2) s vs $prev $(ex h3-prev-1)/$(ex h3-prev-2) s; moss candidate $(ex moss-new) s vs $(ex moss-prev) s (steady in the summary); Krea candidate $(ex krea-new-1)/$(ex krea-new-2)/$(ex krea-new-3) s vs $(ex krea-prev-1)/$(ex krea-prev-2)/$(ex krea-prev-3) s"
fi
