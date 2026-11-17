# h3-vae (cr-306): MiniMax H3's video VAE alone, ours against ComfyUI's, on one rental, one engine at a time. The clips are
# ref/seed.jpg in a slow push-in at 1344x768: 129 frames (ComfyUI's blog shape) and 362 (H3's 15 s). Ours: minimax-h3's
# `vae_round_trip` from a packages_refs checkout, on each arm's Runtime (`vae_encode`, `vae_decode` from its record); an arm
# after the first decodes the first arm's latents, so their decoded frames compare, and then its own once (the round trip).
# ComfyUI: its VAEEncode and VAEDecode nodes on the same clips (fixtures/h3-vae/comfy_vae.py), each `comfy.configs` entry.
# Every timed pass follows a warm-up. PSNR of each arm against the first: latents, decoded frames, the round trip.
# params:
#   tree     the packages_refs checkout whose minimax-h3 runs (pk)
#   arms     [{"name", "runtime": "rc" (the open RC's newest native wheel), https://…#sha256=<hex> or a cozy-runtime
#            dev-build run id, "tree" (optional: its own packages_refs checkout)}], in order (default one arm, "rc")
#   lane     the H3 lane (int8)
#   runs     timed passes per clip, engine and arm (2)
#   clips    frame counts ([129, 362])
#   comfy    {"commit": a ComfyUI commit, "kitchen": comfy-kitchen's version, "torch": "2.14.1", "torchvision": "0.29.1",
#            "revision": Comfy-Org/MiniMax-H3's, "vae": its file under vae/, "configs": [{"name", "fast": true}]}; {} for none
#   need_s   seconds the cap must buy (3600)
. "$(dirname "$0")/lib.sh"
FX=$(cd "$(dirname "$0")" && pwd)/fixtures/h3-vae
TREE=$(param tree pk) LANE=$(param lane int8) RUNS=$(param runs 2) CLIPS=$(param clips '[129, 362]')
ARMS=$(param arms '[{"name": "rc", "runtime": "rc"}]') COMFY=$(param comfy '{}')
[[ $RUNS =~ ^[1-9]$ && $LANE =~ ^[a-z0-9-]+$ ]] || fail "runs or lane"
command -v ffmpeg > /dev/null || fail "no ffmpeg"
mapfile -t COUNTS < <(jq -r '.[]' <<<"$CLIPS")
for n in "${COUNTS[@]}"; do
  [[ $n =~ ^[0-9]+$ ]] || fail "clips"
  ffmpeg -nostdin -v error -y -loop 1 -i "$REF/seed.jpg" -frames:v "$n" -r 24 -pix_fmt yuv420p -c:v libx264 -crf 12 \
    -vf "scale=2688:1536:force_original_aspect_ratio=increase,crop=2688:1536,zoompan=z='1+0.0012*on':x='iw/2-iw/zoom/2':y='ih/2-ih/zoom/2':d=1:s=1344x768:fps=24" \
    "$OUT/clip$n.mp4" || fail "clip $n"
  [ "$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$OUT/clip$n.mp4")" = "$n" ] ||
    fail "clip $n does not hold $n frames"
done
# Each arm's Runtime wheel, fetched before renting. An open RC's Runtime runs under a dev cozy only (rc_runtime_needs_rc_cozy):
# this session's, given with `cozy` a dev build's run id.
mapfile -t NAMES < <(jq -r '.[].name' <<<"$ARMS")
declare -A WHEEL=() PKG=()
for k in "${!NAMES[@]}"; do
  a=${NAMES[$k]} src=$(jq -r ".[$k].runtime" <<<"$ARMS") t=$(jq -r --arg t "$TREE" ".[$k].tree // \$t" <<<"$ARMS")
  [[ $a =~ ^[a-z0-9-]+$ && $t =~ ^[A-Za-z0-9_-]+$ ]] || fail "arm name $a or tree $t"
  PKG[$a]=$PACKAGES/$t/minimax-h3
  grep -q 'def vae_round_trip' "${PKG[$a]}/h3.py" 2> /dev/null || fail "arm $a: $t's minimax-h3 has no vae_round_trip"
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
  WHEEL[$a]=$f; say "arm $a: $(basename "$f"), minimax-h3 at $(git -C "${PKG[$a]}" rev-parse --short HEAD)"
  # Its Triton sets built with the Runtime the arm runs, which a candidate package's import may need (lib.sh `kernels` takes
  # the open RC's); lib.sh then builds them no more.
  d=$(cd "${PKG[$a]}" && pwd)
  t0=$SECONDS
  "$(dirname "$d")/scripts/triton.sh" "$d" "" "file://$f" > "$OUT/kernels-$a.log" 2>&1 ||
    fail "arm $a: minimax-h3's Triton sets did not build: $(tail -c 400 "$OUT/kernels-$a.log")"
  KERNELS[$d]=1; say "kernels $a: built in $((SECONDS - t0)) s"
done
TENSORD=(); [[ $(cozy --version) != *-* ]] || TENSORD=(--tensord "$(command -v cozy)")
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param need_s 3600)"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
pod "nvidia-smi --query-gpu=name,driver_version,power.limit,clocks.max.sm --format=csv,noheader" < /dev/null | tee -a "$R"

# fetch <run> <dir>: its outputs downloaded into dir, each also as dir/<its name>.
fetch() {
  mkdir -p "$2"
  cozy run watch "$1" --once -o "$2" > /dev/null 2> "$2.err"
  local line
  while IFS= read -r line; do
    case $line in latents:\ * | frames:\ * | video:\ *) cp "${line#*: }" "$2/${line%%: *}" ;; esac
  done < "$2.err"
  [ -s "$2/latents" ] && [ -s "$2/frames" ] || fail "run $1's outputs did not land in $2: $(tail -c 300 "$2.err")"
}
# stages <run json>: "<encode s> <decode s>" from its record.
stages() {
  jq -r '[.measurements, .calls[]?.measurements | .attribution.stages? // {}] as $s
    | "\(([$s[].vae_encode.total_ms // empty] | add // 0) / 1000) \(([$s[].vae_decode.total_ms // empty] | add // 0) / 1000)"' "$1"
}
TSV=$OUT/times.tsv; printf 'engine\tarm\tclip\tpass\tencode_s\tdecode_s\trun\n' > "$TSV"
first=${NAMES[0]}
for a in "${NAMES[@]}"; do
  step "update-$a" machine update "$NAME" --runtime "${WHEEL[$a]}" "${TENSORD[@]}" -y || fail "$a: the update"
  cozy machine show "$NAME" --json > "$OUT/show-$a.json" 2>&1
  say "$a: $(jq -c '{version: .status.version, runtime: .status.runtime, tensorfs: .status.tensorfs}' "$OUT/show-$a.json")"
  for n in "${COUNTS[@]}"; do
    given=(); [ "$a" = "$first" ] || given=(--file "latents=$OUT/$first-$n.out/latents")
    for i in $(seq 0 "$RUNS"); do
      c=$a-$n-$i
      step "$c" run "${PKG[$a]}/vae_round_trip" --file "video=$OUT/clip$n.mp4" "${given[@]}" --lane "$LANE" --on "$NAME" || fail "$c"
      N=$(runno "$c"); [ -n "$N" ] || fail "$c named no run"
      cozy run watch "$N" --once --json --full > "$OUT/$c.json" 2>&1
      [ "$(jq -r .status "$OUT/$c.json")" = completed ] || fail "$c: run $N $(jq -r '.reason.message // .status' "$OUT/$c.json" | cut -c1-300)"
      read -r e d < <(stages "$OUT/$c.json")
      printf 'cozy\t%s\t%s\t%s\t%s\t%s\t%s\n' "$a" "$n" "$i" "$e" "$d" "$N" >> "$TSV"
      say "$c: run $N, encode ${e}s, decode ${d}s"
    done
    fetch "$N" "$OUT/$a-$n.out"
    [ "$a" = "$first" ] && continue
    c=$a-$n-own
    step "$c" run "${PKG[$a]}/vae_round_trip" --file "video=$OUT/clip$n.mp4" --lane "$LANE" --on "$NAME" || fail "$c"
    fetch "$(runno "$c")" "$OUT/$c.out"
    PY=(uv run -q --no-project --with numpy --with safetensors python -I "$FX/psnr.py")
    for o in latents frames; do
      say "$a against $first, $n frames, $o: $("${PY[@]}" "$o" "$OUT/$first-$n.out/$o" "$OUT/$a-$n.out/$o" 2>&1 | tail -1)"
    done
    say "$a against $first, $n frames, round trip: $("${PY[@]}" frames "$OUT/$first-$n.out/frames" "$OUT/$c.out/frames" 2>&1 | tail -1)"
  done
done

if [ "$(jq length <<<"$COMFY")" != 0 ]; then
  # ComfyUI alone on the GPU: a restart ends Cozy's executors.
  step free machine update "$NAME" --gpu-memory-limit off -y || say "freeing the GPU: $(tail -2 "$OUT/free.txt")"
  for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; sleep 15; done
  C=$(jq -r .commit <<<"$COMFY") KV=$(jq -r .kitchen <<<"$COMFY") TV=$(jq -r '.torch // "2.14.1"' <<<"$COMFY")
  VV=$(jq -r '.torchvision // "0.29.1"' <<<"$COMFY") REV=$(jq -r .revision <<<"$COMFY") VAE=$(jq -r .vae <<<"$COMFY")
  [[ $C =~ ^[0-9a-f]{40}$ && $REV =~ ^[0-9a-f]{40}$ && $KV =~ ^[0-9.]+$ && $VAE =~ ^[a-z0-9_.]+$ ]] || fail "comfy params"
  for n in "${COUNTS[@]}"; do T=600 pod "mkdir -p /root/h3vae && cat > /root/h3vae/clip$n.mp4" < "$OUT/clip$n.mp4" || fail "clip $n to the pod"; done
  pod "cat > /root/h3vae/comfy_vae.py" < "$FX/comfy_vae.py" || fail "comfy_vae.py to the pod"
  # Its environment and weights, built on the pod while nothing of Cozy's runs.
  T=2400 pod "set -e; cd /root/h3vae; command -v uv > /dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh > /dev/null; export PATH=\$HOME/.local/bin:\$PATH
    git init -q ComfyUI && git -C ComfyUI fetch -q --depth 1 https://github.com/Comfy-Org/ComfyUI.git $C && git -C ComfyUI checkout -q FETCH_HEAD
    uv venv -q -p 3.12 env
    uv pip install -q -p env torch==$TV torchvision==$VV --index-url https://download.pytorch.org/whl/cu130
    uv pip install -q -p env -r ComfyUI/requirements.txt comfy-kitchen==$KV
    curl -fsSL --retry 5 -o vae.safetensors https://huggingface.co/Comfy-Org/MiniMax-H3/resolve/$REV/vae/$VAE
    env/bin/python -c 'import comfy_kitchen, torch; print(\"kitchen\", comfy_kitchen.__version__, \"torch\", torch.__version__)'" \
    < /dev/null > "$OUT/comfy-setup.log" 2>&1 || fail "ComfyUI's setup: $(tail -5 "$OUT/comfy-setup.log")"
  say "ComfyUI $C: $(tail -1 "$OUT/comfy-setup.log")"
  clips=$(printf ' /root/h3vae/clip%s.mp4' "${COUNTS[@]}")
  mapfile -t CONFIGS < <(jq -c '.configs[]' <<<"$COMFY")
  for cfg in "${CONFIGS[@]}"; do
    cn=$(jq -r .name <<<"$cfg") fast=$(jq -r 'if .fast then 1 else 0 end' <<<"$cfg")
    [[ $cn =~ ^[a-z0-9-]+$ ]] || fail "comfy config name $cn"
    T=3600 pod "cd /root/h3vae && env/bin/python comfy_vae.py /root/h3vae/ComfyUI /root/h3vae/vae.safetensors $RUNS $fast$clips" \
      < /dev/null > "$OUT/comfy-$cn.log" 2>&1 || fail "ComfyUI $cn: $(tail -5 "$OUT/comfy-$cn.log")"
    say "ComfyUI $cn: $(grep -m1 '"facts"' "$OUT/comfy-$cn.log")"
    grep '^{"clip"' "$OUT/comfy-$cn.log" | jq -r --arg c "$cn" '["comfyui", $c, (.frames | tostring), (.pass | tostring), (.encode_s | tostring), (.decode_s | tostring), "-"] | @tsv' >> "$TSV"
  done
fi

# The median of each engine, arm and clip's timed passes (pass 0 is the warm-up).
say "$(awk -F'\t' 'NR > 1 && $4 > 0 { k = $1 " " $2 " " $3; e[k] = e[k] " " $5; d[k] = d[k] " " $6 }
  function med(s,   a, n) { n = split(s, a, " "); asort(a); return n % 2 ? a[(n + 1) / 2] : (a[n / 2] + a[n / 2 + 1]) / 2 }
  END { for (k in e) printf "\n  %s frames: encode %.2f s, decode %.2f s", k, med(e[k]), med(d[k]) }' "$TSV" | sort)"
say "PASS: times.tsv holds every pass"
