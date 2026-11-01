# The first-party packages' cells, sourced by suites runtime and packages: one function per package, its default call
# with a scenario prompt and its own seed. A cell that takes another's output (SeedVR2 H3's clip, BiRefNet and the
# describer Klein's image, Qwen-Image's edits its render) falls back to the gate's reference when that one did not run.
PRODUCTS=(minimax-h3 seedvr2 sdxl flux-3-klein qwen-image-2 anima birefnet qwen3.5 fibo-2 hidream-o1 moss-image qwen-image
  flux-2 ideogram-4 krea-2 hunyuan-image-3)
# --first runs these packages before the rest, in its order: a clean standalone number before models stack.
[ -z "$FIRST" ] || mapfile -t PRODUCTS < <(tr , '\n' <<< "$FIRST"; printf '%s\n' "${PRODUCTS[@]}" | grep -vxF -f <(tr , '\n' <<< "$FIRST"))
product() {
  local f=product_${1//[-.]/_}
  if declare -F "$f" >/dev/null; then "$f"; else log "$1: no product cells"; fi
}

H3_SEED=2026102801
# av <cell> <height>: its tallest mp4 is <height>p and carries a soundtrack as long as its picture (within 0.1 s).
# seedvr2 0.1.0 wrote a batch's audio after all its frames, past the writer's interleave buffer (run 20347).
av() {
  [ -f "$1.number" ] || return 0
  local f v a
  f=$(run_get "$OUT" "$1" '.output[]?.path' | grep -E '\.mp4$' | while read -r p; do
    echo "$(ffprobe -v error -select_streams v:0 -show_entries stream=height -of csv=p=0 "$p") $p"; done | sort -n | tail -1)
  v=$(ffprobe -v error -select_streams v:0 -show_entries stream=duration -of csv=p=0 "${f#* }" 2>/dev/null)
  a=$(ffprobe -v error -select_streams a:0 -show_entries stream=duration -of csv=p=0 "${f#* }" 2>/dev/null)
  expect "$1-av" "$(awk -v h="${f%% *}" -v want="$2" -v v="$v" -v a="$a" \
    'BEGIN { print (h == want && v != "" && a != "" && (v - a) ^ 2 < 0.01) ? "PASS" : "FAIL" }')" \
    "${f%% *}p, video ${v:-none} s, audio ${a:-none} s"
}
# refs <cell> <package> <function> <seed>: two named pictures and their mentions (xs-135, xs-139): Klein's character
# (else the gate's reference) placed in the gate's reference scene. The output shows the character in that scene.
refs() {
  cell "$1" "$(pkg "$2" "$3")" seed:=$4 prompt="<Hero> stands on the rocks below <Lighthouse>, in its light and weather" \
    pictures:="[{\"name\": \"Hero\", \"image\": \"${K:-$REF}\"}, {\"name\": \"Lighthouse\", \"image\": \"$REF\"}]"
}
product_minimax_h3() {
  # One fixed request (the Skater row, H3_SEED): quality.py holds its CLIP against the last passed gate's on it. The
  # character picture goes to H3 as given (a project cleans it first).
  local name motion p="$SCENE, slow push-in, ambient sound" SAVED=$ONLY r
  IFS=$'\t' read -r name _ motion _ _ <<< "$(grep -m1 $'^Skater\t' "$HERE/scenarios.tsv")"
  cell h3 "$(pkg minimax-h3 generate)" references:="[{\"name\": \"$name\", \"kind\": \"character\", \"image\": \"$REF\"}]" \
    prompt="<$name> $motion" seed:=$H3_SEED duration_s:=15
  CLIP=$(output h3 mp4)
  # Above 768p a video is upscaled whole, by SeedVR2 (the lock's seedvr2) by default, with its soundtrack.
  [[ $ONLY == *",h3-1080p,"* || $ONLY == *",h3-1440p,"* ]] && ONLY="$ONLY,h3-5s,"
  cell h3-5s "$(pkg minimax-h3 generate)" prompt="$p" seed:=$((R + 16)) duration_s:=5
  ONLY=$SAVED
  for r in 1080 1440; do
    cell "h3-${r}p" "$(pkg minimax-h3 upscale)" --file video="$(output h3-5s mp4)" resolution=${r}p seed:=$((R + 17))
    av "h3-${r}p" $r
  done
}
product_seedvr2() {
  local src=${CLIP:-$(ls "$B/ref-$SKU-clip.mp4" 2>/dev/null)}
  if [ -n "$src" ]; then nice -n 19 ffmpeg -v error -y -i "$src" -t 1 -an -c:v libx264 -pix_fmt yuv420p clip-1s.mp4
  else nice -n 19 ffmpeg -v error -y -loop 1 -i "$REF" -t 1 -r 24 -vf scale=-2:768 -c:v libx264 -pix_fmt yuv420p clip-1s.mp4; fi
  cell seedvr2 "$(pkg seedvr2 upscale)" --file video="$OUT/clip-1s.mp4" resolution=1080p seed:=$((R + 2))
  # 5 s with its sound at 1440p, as H3 calls it.
  if [ -n "$src" ]; then nice -n 19 ffmpeg -v error -y -i "$src" -t 5 -c:v libx264 -pix_fmt yuv420p -c:a aac clip-5s.mp4
  else nice -n 19 ffmpeg -v error -y -loop 1 -i "$REF" -f lavfi -i sine=frequency=440 -t 5 -r 24 -vf scale=-2:768 \
    -c:v libx264 -pix_fmt yuv420p -c:a aac clip-5s.mp4; fi
  cell seedvr2-1440p "$(pkg seedvr2 upscale)" --file video="$OUT/clip-5s.mp4" resolution=1440p aspect_ratio=16:9 seed:=$((R + 18))
  av seedvr2-1440p 1440
}
product_sdxl() { cell sdxl "$(pkg sdxl generate)" prompt="$SCENE, photo" seed:=$((R + 3)); }
product_flux_3_klein() {
  cell klein "$(pkg flux-3-klein generate_turbo)" prompt="$CHAR" prompt_upsampling:=true seed:=$((R + 4))
  K=$(output klein 'png|webp|jpg')
  refs klein-refs flux-3-klein generate_turbo $((R + 19))
}
product_qwen_image_2() {
  cell qwen "$(pkg qwen-image-2 generate)" seed:=$((R + 5)) prompt="This is an RGBA image with transparency. $OBJECT, semi-transparent with soft reflections. The image has alpha channel and the background is transparent."
  refs qwen-refs qwen-image-2 generate_turbo $((R + 20))
}
product_anima() { cell anima "$(pkg anima generate)" prompt="masterpiece, best quality, anime illustration of $CHAR" seed:=$((R + 6)); }
product_birefnet() { cell birefnet "$(pkg birefnet remove_background)" --file image="${K:-$REF}"; }
product_qwen3_5() {
  cell describer "$(pkg qwen3.5 ask)" prompt="Describe this character: appearance, clothing and pose." --file images="${K:-$REF}"
}
# Fibo's prompt upsampler (a greedy VLM) runs, as by default.
product_fibo_2() {
  cell fibo "$(pkg fibo-2 generate)" prompt="$SCENE" prompt_upsampling:=true seed:=$((R + 8))
  refs fibo-refs fibo-2 generate_turbo $((R + 21))
}
product_hidream_o1() {
  cell hidream "$(pkg hidream-o1 generate)" prompt="$CHAR, cinematic portrait" seed:=$((R + 9))
  refs hidream-refs hidream-o1 generate_turbo $((R + 22))
}
product_moss_image() { cell moss "$(pkg moss-image generate)" prompt="$OBJECT, studio shot" seed:=$((R + 10)); }
product_qwen_image() {
  cell qwen-image "$(pkg qwen-image generate)" prompt="$SCENE at dusk, wide shot" seed:=$((R + 11))
  # Two checkpoints of one release that share a component, in turn on one executor with no restart between them
  # (swap.py; TFS 0.15.0 keyed varena's weight sets by release/component, run 15744): Qwen-Image-2512 for text,
  # Qwen-Image-Edit-2511 for an edit, text again; then the Lightning LoRAs, turbo_lora for text and edit_turbo_lora for
  # an edit. No first-party SDXL LoRA exists to swap.
  local qi; qi=$(output qwen-image 'png|webp|jpg')
  cell qwen-image-edit "$(pkg qwen-image generate)" prompt="$SCENE at dusk, covered in fresh snow, wide shot" \
    pictures:="[{\"name\": \"Scene\", \"image\": \"${qi:-$REF}\"}]" seed:=$((R + 12))
  cell qwen-image-t2i "$(pkg qwen-image generate)" prompt="$OBJECT on a beach at noon, photo" seed:=$((R + 13))
  cell qwen-image-turbo "$(pkg qwen-image generate_turbo)" prompt="$CHAR on a rainy street at night, photo" seed:=$((R + 14))
  cell qwen-image-turbo-edit "$(pkg qwen-image generate_turbo)" prompt="$SCENE in bright morning sun, wide shot" \
    pictures:="[{\"name\": \"Scene\", \"image\": \"${qi:-$REF}\"}]" seed:=$((R + 15))
  refs qwen-image-refs qwen-image generate_turbo $((R + 23))
}
# FLUX.2 [dev] through fal's turbo (8 steps), text then two pictures.
product_flux_2() {
  cell flux2 "$(pkg flux-2 generate_turbo)" prompt="$SCENE, a chalkboard by the door reads \"OPEN 9-5\", photo" seed:=$((R + 24))
  refs flux2-refs flux-2 generate_turbo $((R + 25))
}
# Ideogram 4 draws lettering: the card's words are the check (a grey safety card is a warning, not a pass).
product_ideogram_4() {
  cell ideogram "$(pkg ideogram-4 generate)" prompt="$OBJECT on a shop counter beside a hand-lettered card that reads \"FRESH TODAY\", photo" \
    seed:=$((R + 26))
}
product_krea_2() { cell krea "$(pkg krea-2 generate)" prompt="$CHAR under a neon sign that reads \"NIGHT MARKET\", photo" seed:=$((R + 27)); }
# HunyuanImage 3.0's 80B experts page about 85 GB a step through a 24 GB card and wedged there (se-079): only by --only,
# on a card that holds its working set.
product_hunyuan_image_3() {
  [[ $ONLY == *",hunyuan,"* || $ONLY == *",hunyuan-image-3,"* ]] || { log "hunyuan-image-3: only by --only"; return 0; }
  cell hunyuan "$(pkg hunyuan-image-3 generate_turbo)" prompt="$SCENE, a paper banner overhead reads \"WELCOME\"" seed:=$((R + 28))
  refs hunyuan-refs hunyuan-image-3 generate_turbo $((R + 29))
}
