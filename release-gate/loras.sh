# loras.sh (xs-184): a --cells hook (release-gate.yml `cells=release-gate/loras.sh`, `only=` the rows to run) for
# community LoRAs. Each is uploaded from its provider by $M as a private throwaway, so the candidate cozy's importer
# converts it, and its function runs twice at one seed: without it, then with it at 1.0. Per LoRA: `<name>-convert`
# (the upload's format names the table that read it), `<name>-applied` (the LoRA run completed, its record counts the
# layers that apply, and its run was told of none skipped) and `<name>-changed` (the two outputs differ); LoKr files
# too. `wrong-family` must fail loudly: a LoRA on another family's model, at its run. Each model is deleted after.

# lora_upload <name> <source> <format|->: uploads it as LORA_MODEL; LORA_COMMIT is its commit, empty when refused.
lora_upload() {
  local name=$1 source=$2 format=$3 who read
  who=$("$CLI" --json whoami 2> /dev/null | jq -r '.account // empty')
  LORA_MODEL=$who/ci-lora-$name-$R
  if [ "$format" = - ]; then
    run_submit "$OUT" "$name-upload" model upload "$source" "$LORA_MODEL" --lane bf16 --on "$M" > /dev/null
  else
    run_submit "$OUT" "$name-upload" model upload "$source" "$LORA_MODEL" --lane bf16 --source-format "$format" --on "$M" > /dev/null
  fi
  run_wait "$OUT" "$name-upload"
  LORA_COMMIT=$(run_get "$OUT" "$name-upload" '.result.models[0].published.commit')
  read=$(run_get "$OUT" "$name-upload" '.result.models[0].format')
  log "$name: upload run $(cat "$name-upload.number"): ${read:-$(run_get "$OUT" "$name-upload" '.reason.message')}"
}

lora_delete() {
  "$CLI" model delete "$LORA_MODEL" -y > "$1-delete.log" 2>&1 || log "$1: $LORA_MODEL not deleted: $(tail -c 200 "$1-delete.log")"
}

# lora_ab <name> <package> <function> <source> <format|-> <slot|-> [args...]
lora_ab() {
  local name=$1 package=$2 fn=$3 source=$4 format=$5 slot=$6 spec target said layers a b SAVED=$ONLY
  shift 6
  [ -z "$ONLY" ] || [[ $ONLY == *",$name,"* ]] || return 0
  [ -z "$ONLY" ] || ONLY="$ONLY$name-base,$name-lora,"
  lora_upload "$name" "$source" "$format"
  expect "$name-convert" "$([ -n "$LORA_COMMIT" ] && echo PASS || echo FAIL)" \
    "upload run $(cat "$name-upload.number"): $(run_get "$OUT" "$name-upload" '.result.models[0].format // .reason.message')"
  [ -n "$LORA_COMMIT" ] || { ONLY=$SAVED; return 0; }
  spec=$LORA_MODEL@$LORA_COMMIT,1.0
  [ "$slot" = - ] || spec=$slot=$spec
  target=$(pkg "$package" "$fn")
  cell "$name-base" "$target" "$@"
  cell "$name-lora" "$target" --lora "$spec" "$@"
  said=$(grep -o 'do not apply[^"]*' "$name-lora.ndjson" 2> /dev/null | head -1)
  layers=$(jq '[.. | objects | select(has("adapters")) | .adapters[]? | .layers // 0] | add // 0' "$name-lora.json" 2> /dev/null)
  expect "$name-applied" "$([ "$(run_get "$OUT" "$name-lora" .status)" = completed ] && [ "${layers:-0}" -gt 0 ] && [ -z "$said" ] \
    && echo PASS || echo FAIL)" "run $(cat "$name-lora.number"): $(run_get "$OUT" "$name-lora" .status), ${layers:-0} layers applied${said:+; $said}"
  a=$(output "$name-base" 'png|webp|jpg|mp4') b=$(output "$name-lora" 'png|webp|jpg|mp4')
  expect "$name-changed" "$([ -n "$a" ] && [ -n "$b" ] && ! cmp -s "$a" "$b" && echo PASS || echo FAIL)" "${a:-no output} vs ${b:-no output}"
  lora_delete "$name"
  ONLY=$SAVED
}

lora_ab moss-axiom moss-image generate \
  hf://axiomofmind/Moss-image-nsfw-uncensored-lora@5f24e258ebb2a7d8aa77ee4940504dd57b54d5b9/MOSS-Image-NSFW-Uncensored-LoRA-ComfyUI.safetensors - - \
  prompt="moss_xvstyle, a woman sitting by a window in soft morning light, photo" seed:=$((R + 41))
# Civitai answers a pod HTTP 451 (11-12), so the rows read Hugging Face copies: Danrisi's Lenovo UltraReal (AI-Toolkit,
# ComfyUI's fused proj.qkv) from mossloras' archive, and ACCC1380's realstyle2 (kohya's networks.lora_anima).
lora_ab moss-lenovo moss-image generate \
  hf://mossloras/moss-image-lora-archive@c585b695025a70c7da77e6b86e732d5854c451ea/2026-10-17/MOSS-Image/1662740_Lenovo-UltraReal/3410543/lenovo_moss.safetensors - - \
  prompt="a woman in a loose linen dress in a sunlit kitchen, photo" seed:=$((R + 42))
lora_ab anima-realstyle anima generate \
  hf://ACCC1380/realstyle2_anima@86c3a5b72c97daefe1d99492e3d211650436d789/realstyle2_anima.safetensors - - \
  prompt="masterpiece, best quality, 1girl, white sundress, standing in a flower field" seed:=$((R + 43))
lora_ab qwen-edit-nsfw qwen-image generate_turbo \
  hf://ScottzillaSystems/qwen-image-edit-plus-nsfw-lora@66e89e998dd4ea1a359c4bf0dd5e17d2f0b06ef0/qwen-image-edit-plus-nsfw-lora.safetensors - edit_model \
  prompt="<Scene> in warm evening light, photo" references:="[{\"name\": \"Scene\", \"image\": \"$REF\"}]" seed:=$((R + 44))
lora_ab h3-mystic minimax-h3 generate civitai://3416478/MysticXXX_MMH3-V5.safetensors h3.lora - \
  prompt="A woman dances slowly in a dim club, neon light on her face, slow push-in, music" duration_s:=5 seed:=$((R + 45))
lora_ab h3-hearmeman minimax-h3 generate \
  hf://Hearmeman/minimax-h3-loras@e5f4fb9030d1ac27caa0f773b96fbd2d4cafadbb/HMNSFW-AIO-V2.5.safetensors - - \
  prompt="A woman walks along a beach at sunset, waves rolling in, wide shot, ocean sound" duration_s:=5 seed:=$((R + 46))

# LoKr (LyCORIS Kronecker) adapters: Hearmeman's two newest H3 ones and SAkizuki's Anima-Aniart.
HM=hf://Hearmeman/minimax-h3-loras@e5f4fb9030d1ac27caa0f773b96fbd2d4cafadbb
lora_ab h3-lokr-cumshot minimax-h3 generate "$HM/HMCumshot_V2.safetensors" - - \
  prompt="A woman laughs on a sofa in a sunlit living room, medium shot, room tone" duration_s:=5 seed:=$((R + 56))
lora_ab h3-lokr-pussy minimax-h3 generate "$HM/hmpussy_v6_epoch30.safetensors" - - \
  prompt="A woman stretches on a bed in soft morning light, slow push-in, quiet room" duration_s:=5 seed:=$((R + 57))
lora_ab anima-lokr-aniart anima generate \
  hf://SAkizuki/Anima-Aniart@09f3687270186577aeecc40ad53055c960959230/anima-aniart.safetensors - - \
  prompt="masterpiece, best quality, 1girl, school uniform, under cherry blossoms" seed:=$((R + 58))
# An Anima LoRA on MOSS Image fails at its run (another family); it never runs as if it applied.
if [ -z "$ONLY" ] || [[ $ONLY == *",wrong-family,"* ]]; then
  lora_upload wrong-family hf://ACCC1380/realstyle2_anima@86c3a5b72c97daefe1d99492e3d211650436d789/realstyle2_anima.safetensors -
  if [ -n "$LORA_COMMIT" ]; then
    cell wrong-family "$(pkg moss-image generate)" --lora "$LORA_MODEL@$LORA_COMMIT,1.0" prompt="a red bicycle, photo" seed:=$((R + 47))
    why=$(run_get "$OUT" wrong-family '.reason.message')
    expect wrong-family "$([ "$(run_get "$OUT" wrong-family .status)" = failed ] && [[ $why == *anima* ]] && echo PASS || echo FAIL)" \
      "run $(cat wrong-family.number): $(run_get "$OUT" wrong-family .status): ${why:-no reason}"
    lora_delete wrong-family
  fi
fi

# MOSS Image's built-in edits (xs-184 phase 2): a prompt mentioning pictures runs under Moss-Image-Edit with no --lora;
# image_to_image remakes a picture. Judged by eye (previews); each must complete with a picture. `only=moss-edit` runs all.
moss_cell() {  # moss_cell <name> <function> [args...]
  local name=$1 fn=$2 SAVED=$ONLY
  shift 2
  [ -z "$ONLY" ] || [[ $ONLY == *",$name,"* || $ONLY == *",moss-edit,"* ]] || return 0
  [ -z "$ONLY" ] || ONLY="$ONLY$name,"
  cell "$name" "$(pkg moss-image "$fn")" "$@"
  expect "$name-made" "$([ -n "$(output "$name" 'png|webp|jpg')" ] && echo PASS || echo FAIL)" \
    "run $(cat "$name.number"): $(run_get "$OUT" "$name" .status)"
  ONLY=$SAVED
}
# The gate's reference picture is a place, so MOSS makes the person the edits start from.
moss_cell moss-person generate seed:=$((R + 49)) aspect_ratio=3:4 \
  prompt="a photo of one young woman with short black hair, alone, in a plain white t-shirt and jeans, standing in a city park, full body, natural light"
SUBJECT=$(output moss-person 'png|webp|jpg')
SUBJECT=${SUBJECT:-$REF}
PERSON="[{\"name\": \"Person\", \"image\": \"$SUBJECT\"}]"
moss_cell moss-edit-red generate seed:=$((R + 50)) references:="$PERSON" \
  prompt="Change only the clothing of <Person> to a bright red jacket. Keep the face, hair, pose and background unchanged."
moss_cell moss-edit-beach generate seed:=$((R + 51)) references:="$PERSON" \
  prompt="Put <Person> on a sunny beach with the sea behind, keeping the person unchanged."
moss_cell moss-edit-watercolor generate seed:=$((R + 52)) references:="$PERSON" \
  prompt="Turn <Person> into a soft watercolor painting, keeping the woman, her pose and the composition."
moss_cell moss-hat generate seed:=$((R + 53)) prompt="a straw sun hat with a blue ribbon on a white table, studio photo"
HAT=$(output moss-hat 'png|webp|jpg')
[ -z "$HAT" ] || moss_cell moss-edit-two generate seed:=$((R + 54)) \
  references:="[{\"name\": \"Person\", \"image\": \"$SUBJECT\"}, {\"name\": \"Hat\", \"image\": \"$HAT\"}]" \
  prompt="Put the straw hat from <Hat> on the woman in <Person>. Keep her face, clothes, pose and the background unchanged."
moss_cell moss-img2img image_to_image --file image="$SUBJECT" prompt="an oil painting, thick brush strokes" strength:=0.6 seed:=$((R + 55))

# MOSS image_to_image's strength (xs-184): 0.6, 0.75 and 0.85 on two pictures (a woman MOSS makes, the gate's place),
# one seed, judged by eye: the default is where the picture changes but keeps its layout. `only=moss-i2i` runs them.
if [ -z "$ONLY" ] || [[ $ONLY == *",moss-i2i,"* ]]; then
  SAVED_I2I=$ONLY
  [ -z "$ONLY" ] || ONLY="$ONLY,moss-i2i-person,"
  cell moss-i2i-person "$(pkg moss-image generate)" seed:=$((R + 49)) aspect_ratio=3:4 \
    prompt="a photo of one young woman with short black hair, alone, in a plain white t-shirt and jeans, standing in a city park, full body, natural light"
  WOMAN=$(output moss-i2i-person 'png|webp|jpg')
  for strength in 0.6 0.75 0.85; do
    for picture in woman place; do
      source=$([ $picture = woman ] && echo "$WOMAN" || echo "$REF")
      [ -n "$source" ] || continue
      name=moss-i2i-$picture-${strength#0.}
      [ -z "$SAVED_I2I" ] || ONLY="$ONLY$name,"
      cell "$name" "$(pkg moss-image image_to_image)" --file image="$source" prompt="an oil painting, thick brush strokes" \
        strength:=$strength seed:=$((R + 60))
      expect "$name-made" "$([ -n "$(output "$name" 'png|webp|jpg')" ] && echo PASS || echo FAIL)" "run $(cat "$name.number")"
    done
  done
  ONLY=$SAVED_I2I
fi
