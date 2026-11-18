# h3-upscaler (cr-327): H3's latent upscaler (lbh-123-ai/h3-latent-upscaler) on one rental. Its int8 lane is written
# privately from the bf16 commit by the tree's recipe (`derive`), on the Runtime of the first arm that runs it. One clip
# (ref/seed.jpg in a slow push-in at 1344x768, `frames` frames) is encoded once (`vae_round_trip`, the first arm), and
# each arm upscales those latents to `to` (`latent_upscale`, the record's `upscale_latent`), `runs` timed passes after a
# warm-up, then decodes its last upscale (`vae_round_trip` with its latents): every arm's latents and decoded frames
# against the first arm's (PSNR), and frames 72 and 288 side by side in $OUT/frames. With `full`, one real `upscale` of
# the clip at `to` on the first arm: its stages, the upscaler's share of the request. params:
#   tree     the packages_refs checkout whose minimax-h3 runs (pk)
#   bf16     the upscaler's bf16 commit (lbh-123-ai/h3-latent-upscaler@sha256:…)
#   int8     its int8 commit, or "derive" (default): written on the pod by the tree's recipe
#   arms     [{"name", "runtime": "rc" (the open RC's native wheel), https://…#sha256=<hex> or a cozy-runtime dev-build
#            run id, "upscaler": "bf16" or "int8", "tree" (optional)}], in order
#   frames   the clip's frames (362: H3's 15 s)
#   to       1080p, 1440p (default) or 2160p
#   runs     timed passes per arm (2)
#   full     true: the real upscale too
#   need_s   seconds the cap must buy (3600)
. "$(dirname "$0")/lib.sh"
FX=$(cd "$(dirname "$0")" && pwd)/fixtures/h3-vae
TREE=$(param tree pk) RUNS=$(param runs 2) FRAMES=$(param frames 362) TO=$(param to 1440p) FULL=$(param full false)
BF16=$(param bf16) INT8=$(param int8 derive) ARMS=$(param arms '[]')
[[ $RUNS =~ ^[1-9]$ && $FRAMES =~ ^[0-9]+$ && $TO =~ ^(1080|1440|2160)p$ ]] || fail "runs, frames or to"
[[ $BF16 == lbh-123-ai/h3-latent-upscaler@sha256:* ]] || fail "bf16: the upscaler's bf16 commit"
[[ $INT8 == derive || $INT8 == lbh-123-ai/h3-latent-upscaler@sha256:* ]] || fail "int8: derive or a commit"
[ "$(jq length <<<"$ARMS")" -gt 0 ] || fail "no arms"
command -v ffmpeg > /dev/null || fail "no ffmpeg"
CLIP=$OUT/clip.mp4
ffmpeg -nostdin -v error -y -loop 1 -i "$REF/seed.jpg" -frames:v "$FRAMES" -r 24 -pix_fmt yuv420p -c:v libx264 -crf 12 \
  -vf "scale=2688:1536:force_original_aspect_ratio=increase,crop=2688:1536,zoompan=z='1+0.0012*on':x='iw/2-iw/zoom/2':y='ih/2-ih/zoom/2':d=1:s=1344x768:fps=24" \
  "$CLIP" || fail "clip"
# Each arm's Runtime wheel and the Triton sets its tree builds with it, before renting (h3-vae.sh's way).
mapfile -t NAMES < <(jq -r '.[].name' <<<"$ARMS")
declare -A WHEEL=() PKG=() UP=()
for k in "${!NAMES[@]}"; do
  a=${NAMES[$k]} src=$(jq -r ".[$k].runtime" <<<"$ARMS") t=$(jq -r --arg t "$TREE" ".[$k].tree // \$t" <<<"$ARMS")
  UP[$a]=$(jq -r ".[$k].upscaler" <<<"$ARMS")
  [[ $a =~ ^[a-z0-9-]+$ && $t =~ ^[A-Za-z0-9_-]+$ && ${UP[$a]} =~ ^(bf16|int8)$ ]] || fail "arm $a: name, tree $t or upscaler"
  PKG[$a]=$PACKAGES/$t/minimax-h3
  grep -q 'def latent_upscale' "${PKG[$a]}/h3.py" 2> /dev/null || fail "arm $a: $t's minimax-h3 has no latent_upscale"
  if [ "$src" = rc ]; then
    base=https://github.com/cozy-creator/cozy-runtime/releases/download/rc-$(cozy --version | awk '{print $2}' | cut -d- -f1)
    line=$(curl -fsSL "$base/SHA256SUMS" | awk '$2 ~ /manylinux_2_28_x86_64[.]whl$/ {print; exit}')
    [ -n "$line" ] || fail "arm $a: no native wheel at $base"
    src="$base/${line##* }#sha256=${line%% *}"
  fi
  if [[ $src == https://* ]]; then
    d=$OUT/wheels/$a; mkdir -p "$d"; f=$d/$(basename "${src%%#*}" | sed 's/%2B/+/g')
    curl -fsSL --retry 5 -o "$f" "${src%%#*}" && [ "$(sha256sum "$f" | cut -c1-64)" = "${src##*sha256=}" ] || fail "arm $a: $src"
  else
    f=$(wheel_from "$OUT/wheels/$a" "$src") && [ -n "$f" ] || fail "arm $a: no native wheel in dev-build $src"
  fi
  WHEEL[$a]=$f; say "arm $a: $(basename "$f"), upscaler ${UP[$a]}, minimax-h3 at $(git -C "${PKG[$a]}" rev-parse --short HEAD)"
  d=$(cd "${PKG[$a]}" && pwd)
  "$(dirname "$d")/scripts/triton.sh" "$d" "" "file://$f" > "$OUT/kernels-$a.log" 2>&1 ||
    fail "arm $a: minimax-h3's Triton sets did not build: $(tail -c 400 "$OUT/kernels-$a.log")"
  KERNELS[$d]=1
done
TENSORD=(); [[ $(cozy --version) != *-* ]] || TENSORD=(--tensord "$(command -v cozy)")
quote "$(param need_s 3600)"
rent
say "pod: $(cozy machine show "$NAME" --json | jq -c '{v: .status.version, gpu: .status.gpus[0].name}')"

# fetch <run> <dir>: its outputs downloaded into dir, each also as dir/<its name>.
fetch() {
  mkdir -p "$2"
  cozy run watch "$1" --once -o "$2" > /dev/null 2> "$2.err"
  local line
  while IFS= read -r line; do
    case $line in latents:\ * | frames:\ * | video:\ *) cp "${line#*: }" "$2/${line%%: *}" ;; esac
  done < "$2.err"
  [ -s "$2/latents" ] || fail "run $1's outputs did not land in $2: $(tail -c 300 "$2.err")"
}
# completed <step>: its run's number, its record in $OUT/<step>.json; else why not, and 1.
completed() {
  local n; n=$(runno "$1"); [ -n "$n" ] || { echo "$1 named no run"; return 1; }
  cozy run watch "$n" --once --json --full > "$OUT/$1.json" 2>&1
  [ "$(jq -r .status "$OUT/$1.json")" = completed ] ||
    { echo "$1: run $n $(jq -r '.reason.message // .status' "$OUT/$1.json" | cut -c1-300)"; return 1; }
  echo "$n"
}
# stage <record> <name>: the stage's seconds.
stage() {
  jq -r --arg s "$2" '[.measurements, .calls[]?.measurements | .attribution.stages? // {}] as $x
    | ([$x[][$s].total_ms // empty] | add // 0) / 1000' "$1"
}
PY=(uv run -q --no-project --with numpy --with safetensors python -I "$FX/psnr.py")
TSV=$OUT/times.tsv; printf 'arm\tpass\tupscale_latent_s\trun\n' > "$TSV"
first=${NAMES[0]}
for a in "${NAMES[@]}"; do
  step "update-$a" machine update "$NAME" --runtime "${WHEEL[$a]}" "${TENSORD[@]}" -y || fail "$a: the update"
  say "$a: $(cozy machine show "$NAME" --json | jq -c '{runtime: .status.runtime, tensorfs: .status.tensorfs}')"
  if [ "${UP[$a]}" = int8 ] && [ "$INT8" = derive ]; then
    step derive model quantize "$BF16" --lane int8 --package "${PKG[$a]}" --on "$NAME" || fail "derive: $(tail -c 400 "$OUT/derive.txt")"
    c=$(made derive); [ -n "$c" ] || fail "derive made no commit: $(tail -c 300 "$OUT/derive.txt")"
    INT8=lbh-123-ai/h3-latent-upscaler@$c
    say "INT8 COMMIT $INT8 (private)"
  fi
  if [ "$a" = "$first" ]; then
    step encode run "${PKG[$a]}/vae_round_trip" --file "video=$CLIP" --on "$NAME" || fail "encode"
    N=$(completed encode) || fail "$N"
    fetch "$N" "$OUT/clip.out"
    LAT=$OUT/clip.out/latents
    say "clip latents: $(jq -c '.calls[]?.measurements.attribution.stages? // empty' "$OUT/encode.json" | head -1 | cut -c1-200)"
  fi
  model=$BF16; [ "${UP[$a]}" = int8 ] && model=$INT8
  for i in $(seq 0 "$RUNS"); do
    c=up-$a-$i
    step "$c" run "${PKG[$a]}/latent_upscale" --file "latents=$LAT" "resolution=$TO" "model.latent_upscaler=$model" --on "$NAME" ||
      fail "$c: $(tail -c 300 "$OUT/$c.txt")"
    N=$(completed "$c") || fail "$N"
    s=$(stage "$OUT/$c.json" upscale_latent)
    printf '%s\t%s\t%s\t%s\n' "$a" "$i" "$s" "$N" >> "$TSV"
    say "$c: run $N, upscale_latent ${s}s, plan $(jq -c '[.calls[]?.measurements.plan_reports? // empty] | .[0] // {} | with_entries(select(.key | test("conv")))' "$OUT/$c.json" 2>/dev/null | cut -c1-300)"
  done
  fetch "$N" "$OUT/$a.up"
  step "decode-$a" run "${PKG[$a]}/vae_round_trip" --file "video=$CLIP" --file "latents=$OUT/$a.up/latents" --on "$NAME" ||
    fail "decode $a"
  N=$(completed "decode-$a") || fail "$N"
  fetch "$N" "$OUT/$a.decode"
  [ "$a" = "$first" ] && continue
  say "$a against $first, upscaled latents: $("${PY[@]}" latents "$OUT/$first.up/latents" "$OUT/$a.up/latents" 2>&1 | tail -1)"
  say "$a against $first, decoded frames: $("${PY[@]}" frames "$OUT/$first.decode/frames" "$OUT/$a.decode/frames" 2>&1 | tail -1)"
done
mkdir -p "$OUT/frames"
for f in 72 288; do
  inputs=() chain='' labels='' i=0
  for a in "${NAMES[@]}"; do
    inputs+=(-i "$OUT/$a.decode/video"); chain+="[$i:v]select=eq(n\\,$f),scale=1280:720,setsar=1[v$i];"; labels+="[v$i]"; i=$((i + 1))
  done
  ffmpeg -v error -y "${inputs[@]}" -filter_complex "${chain}${labels}hstack=inputs=$i" -frames:v 1 -q:v 3 "$OUT/frames/arms-$f.jpg" ||
    say "frames $f failed"
done
if [ "$FULL" = true ]; then
  step "update-full" machine update "$NAME" --runtime "${WHEEL[$first]}" "${TENSORD[@]}" -y || fail "full: the update"
  if step full run "${PKG[$first]}/upscale" --file "video=$CLIP" "to=$TO" --on "$NAME"; then
    N=$(completed full) || say "$N"
    say "full upscale, run $N: $(jq -c '[.measurements, .calls[]?.measurements | .attribution.stages? // empty] | add' "$OUT/full.json" | cut -c1-500)"
  else
    say "full upscale failed: $(tail -c 400 "$OUT/full.txt")"
  fi
fi
say "$(awk -F'\t' 'NR > 1 && $2 > 0 { t[$1] = t[$1] " " $3 }
  function med(s,   v, n) { n = split(s, v, " "); asort(v); return n % 2 ? v[(n + 1) / 2] : (v[n / 2] + v[n / 2 + 1]) / 2 }
  END { for (a in t) printf "\n  %s: upscale_latent %.2f s", a, med(t[a]) }' "$TSV" | sort)"
say "PASS: times.tsv holds every pass; $OUT/frames the arms side by side"
