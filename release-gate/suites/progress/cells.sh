# Suite `progress` (cl-921): each first-party package's tool as a box calls it, cold on a fresh machine (nothing of it installed
# or downloaded: no API warming before it, `COLD`) and then warm. check.py reads each one's event stream (<cell>.ndjson, the
# events the page draws) as the phases it went through: in order, its machine and GPU, numbers, a time left, words the page
# knows, never silent, and no warning of what the Runtime recovered from. HunyuanImage 3.0 only by --only, on 48 GB or more.
COLD=1
# The warm call asks something new (the next seed, the picture mirrored, other words), so it runs rather than answers from the
# cold one's record (`memo`).
both() {
  local name=$1 a warm=(); shift
  [ -n "${WARM_ONLY:-}" ] || cell "$name-cold" "$@"
  for a in "$@"; do
    case $a in
      seed:=*) warm+=("seed:=$(( ${a#seed:=} + 1 ))") ;;
      image=*) warm+=("image=$REF2") ;;
      video=*) warm+=("video=$clip2") ;;
      prompt=*) warm+=("$a Say it briefly.") ;;
      *) warm+=("$a") ;;
    esac
  done
  cell "$name-warm" "${warm[@]}"
}
# A box's prompt: its words and no mentions.
said() { jq -cn --arg t "$1" '{text: $t, references: []}'; }
clip=$OUT/clip-1s.mp4 clip2=$OUT/clip-1s-mirrored.mp4 REF2=$OUT/ref-mirrored.jpg
[ -f "$REF2" ] || nice -n 19 ffmpeg -v error -y -i "$REF" -vf hflip "$REF2"
[ -f "$clip" ] || nice -n 19 ffmpeg -v error -y -loop 1 -i "$REF" -t 1 -r 24 -vf scale=-2:480 -c:v libx264 -pix_fmt yuv420p "$clip"
[ -f "$clip2" ] || nice -n 19 ffmpeg -v error -y -loop 1 -i "$REF2" -t 1 -r 24 -vf scale=-2:480 -c:v libx264 -pix_fmt yuv420p "$clip2"
# Fibo first: Paul's 46780, the run that sat silent for minutes.
both fibo "$(pkg fibo-2 image)" instructions:="$(said "$SCENE")" seed:=$((R + 8))
both sdxl "$(pkg sdxl image)" instructions="$SCENE, photo" seed:=$((R + 3))
both klein "$(pkg flux-3-klein image)" instructions:="$(said "$CHAR")" seed:=$((R + 4))
both qwen "$(pkg qwen-image-2 image)" instructions:="$(said "$OBJECT on a marble plinth")" megapixels:=1 seed:=$((R + 5))
both anima "$(pkg anima image)" instructions="masterpiece, best quality, anime illustration of $CHAR" megapixels=1 seed:=$((R + 6))
both birefnet "$(pkg birefnet remove_background)" --file image="$REF"
both describer "$(pkg qwen3.5 ask)" prompt="Describe this picture." --file images="$HERE/ref/seed.jpg"
both hidream "$(pkg hidream-o1 image)" instructions:="$(said "$CHAR, cinematic portrait")" seed:=$((R + 9))
both moss "$(pkg moss-image image)" instructions="$OBJECT, studio shot" seed:=$((R + 10))
both qwen-image "$(pkg qwen-image image)" instructions:="$(said "$SCENE at dusk, wide shot")" seed:=$((R + 11))
both flux2 "$(pkg flux-2 image)" instructions:="$(said "$SCENE, photo")" seed:=$((R + 24))
both ideogram "$(pkg ideogram-4 image)" instructions="a hand-lettered card that reads \"FRESH TODAY\"" seed:=$((R + 26))
both krea "$(pkg krea-2 image)" instructions="$CHAR under a neon sign, photo" megapixels=1 seed:=$((R + 27))
both p-h3-image "$(pkg p-h3-image generate)" instructions:="$(said "$SCENE, photo")" megapixels:=1 seed:=$((R + 30))
both seedvr2 "$(pkg seedvr2 upscale)" --file video="$clip" resolution=1080p seed:=$((R + 2))
both relayvsr "$(pkg relayvsr upscale)" --file video="$clip" resolution=1080p
# H3 on the 15 s clip, as every H3 number is: its segments, frames and its result coming here.
both h3 "$(pkg minimax-h3 generate)" prompt="$SCENE, slow push-in, ambient sound" seed:=$((R + 16)) duration_s:=15
[[ $ONLY != *",hunyuan"* ]] || both hunyuan "$(pkg hunyuan-image-3 image)" instructions:="$(said "$SCENE")" seed:=$((R + 28))
COLD=
