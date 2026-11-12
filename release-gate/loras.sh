# loras.sh (xs-184): a --cells hook (release-gate.yml `cells=release-gate/loras.sh`, `only=` the rows to run) for
# community LoRAs. Each is uploaded from its provider by $M as a private throwaway, so the candidate cozy's importer
# converts it, and its function runs twice at one seed: without it, then with it at 1.0. Per LoRA: `<name>-convert`
# (the upload's format names the table that read it), `<name>-applied` (the LoRA run completed, its record counts the
# layers that apply, and its run was told of none skipped) and `<name>-changed` (the two outputs differ). The last two
# rows must fail loudly: a LoKr file is refused at upload, a LoRA on another family's model at its run. Each model is
# deleted after.

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
lora_ab moss-sagging moss-image generate civitai://3437772 - - \
  prompt="a woman in a loose linen dress in a sunlit kitchen, photo" seed:=$((R + 42))
lora_ab anima-sagging anima generate civitai://3052892 - - \
  prompt="masterpiece, best quality, 1girl, white sundress, standing in a flower field" seed:=$((R + 43))
lora_ab qwen-edit-nsfw qwen-image generate_turbo \
  hf://ScottzillaSystems/qwen-image-edit-plus-nsfw-lora@66e89e998dd4ea1a359c4bf0dd5e17d2f0b06ef0/qwen-image-edit-plus-nsfw-lora.safetensors - edit_model \
  prompt="<Scene> in warm evening light, photo" references:="[{\"name\": \"Scene\", \"image\": \"$REF\"}]" seed:=$((R + 44))
lora_ab h3-mystic minimax-h3 generate civitai://3416478/MysticXXX_MMH3-V5.safetensors h3.lora - \
  prompt="A woman dances slowly in a dim club, neon light on her face, slow push-in, music" duration_s:=5 seed:=$((R + 45))
lora_ab h3-hearmeman minimax-h3 generate \
  hf://Hearmeman/minimax-h3-loras@e5f4fb9030d1ac27caa0f773b96fbd2d4cafadbb/HMNSFW-AIO-V2.5.safetensors - - \
  prompt="A woman walks along a beach at sunset, waves rolling in, wide shot, ocean sound" duration_s:=5 seed:=$((R + 46))

# A LoKr (Hearmeman's pussy v6) is refused at upload, naming its kind; nothing is stored.
if [ -z "$ONLY" ] || [[ $ONLY == *",lokr-refused,"* ]]; then
  lora_upload lokr-refused hf://Hearmeman/minimax-h3-loras@e5f4fb9030d1ac27caa0f773b96fbd2d4cafadbb/hmpussy_v6_epoch30.safetensors -
  [ -z "$LORA_COMMIT" ] || lora_delete lokr-refused
  why=$(run_get "$OUT" lokr-refused-upload '.reason.message')
  expect lokr-refused "$([ -z "$LORA_COMMIT" ] && [[ $why == *"LyCORIS LoKr"* ]] && echo PASS || echo FAIL)" "${why:-it was not refused}"
fi
# An Anima LoRA on MOSS Image fails at its run (another family); it never runs as if it applied.
if [ -z "$ONLY" ] || [[ $ONLY == *",wrong-family,"* ]]; then
  lora_upload wrong-family civitai://3052892 -
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
PERSON="[{\"name\": \"Person\", \"image\": \"$REF\"}]"
moss_cell moss-edit-red generate seed:=$((R + 50)) references:="$PERSON" \
  prompt="Change only the clothing of <Person> to a bright red jacket. Keep the face, hair, pose and background unchanged."
moss_cell moss-edit-beach generate seed:=$((R + 51)) references:="$PERSON" \
  prompt="Put <Person> on a sunny beach with the sea behind, keeping the person unchanged."
moss_cell moss-edit-watercolor generate seed:=$((R + 52)) references:="$PERSON" prompt="Turn <Person> into a soft watercolor painting."
moss_cell moss-hat generate seed:=$((R + 53)) prompt="a straw sun hat with a blue ribbon on a white table, studio photo"
HAT=$(output moss-hat 'png|webp|jpg')
[ -z "$HAT" ] || moss_cell moss-edit-two generate seed:=$((R + 54)) \
  references:="[{\"name\": \"Person\", \"image\": \"$REF\"}, {\"name\": \"Hat\", \"image\": \"$HAT\"}]" \
  prompt="<Person> wearing the hat from <Hat>, keeping the person, pose and background unchanged."
moss_cell moss-img2img image_to_image --file image="$REF" prompt="an oil painting, thick brush strokes" strength:=0.6 seed:=$((R + 55))
