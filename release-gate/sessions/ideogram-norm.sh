# ideogram-norm (se-162): what decoding Ideogram 4's latents with the FLUX.2 VAE's BatchNorm statistics (Diffusers' Ideogram4Pipeline,
# ComfyUI, the ideogram-4 package on rc/0.27.0) instead of Ideogram's own latent normalization (its pipeline, latent_norm.py) does to
# a picture: real pictures through the real VAE on CPU, no DiT (fixtures/upstream-parity/ideogram_norm.py). $0, no rental.
# The VAE comes from cocktailpeanut/ideogram-4-fp8, checked file for file against ideogram-ai/ideogram-4-fp8@ee79a72 (upstream-parity).
. "$(dirname "$0")/lib.sh"
FX=$(cd "$(dirname "$0")" && pwd)/fixtures/upstream-parity
W=${RUNNER_TEMP:-$OUT/..}/ideogram-norm
COPY=cocktailpeanut/ideogram-4-fp8 REV=e0a0fb243cbf458fde53e358e80e6640cc593fd0
rm -rf "$W" && mkdir -p "$W/vae" || fail "$W"
( set -e; cd "$W"
  git clone -q https://github.com/ideogram-oss/ideogram4.git ideogram4 && git -C ideogram4 checkout -q 990fe1c
  git init -q hidream && git -C hidream fetch -q --depth 1 https://github.com/HiDream-ai/HiDream-O1-Image.git 2c2d29ff729e48f33e41f49edfdbd81d5ac103b4 && git -C hidream checkout -q FETCH_HEAD
  for f in config.json diffusion_pytorch_model.safetensors; do
    curl -fsSL --retry 5 -o "vae/$f" "https://huggingface.co/$COPY/resolve/$REV/vae/$f"
  done
  echo "vae sha256 $(sha256sum vae/diffusion_pytorch_model.safetensors | cut -c1-64)"
  uv venv -q --python 3.12 py && uv pip install -q --python py/bin/python --torch-backend cpu torch diffusers==0.41.0 pillow numpy safetensors
) > "$OUT/env.log" 2>&1
# Its own status, not `|| fail`: a subshell tested by `||` ignores its `set -e`.
[ $? = 0 ] || fail "environment: $(tail -5 "$OUT/env.log")"
# The official VAE's sha256 (ideogram-ai/ideogram-4-fp8@ee79a72 lists it).
want=$(curl -fsS "https://huggingface.co/api/models/ideogram-ai/ideogram-4-fp8/revision/ee79a7237b519f1402ceacf952f30c8a31ec5073?blobs=true" |
  jq -r '.siblings[] | select(.rfilename == "vae/diffusion_pytorch_model.safetensors") | .lfs.sha256')
grep -q "vae sha256 $want" "$OUT/env.log" || fail "the VAE is not the official one: $(grep 'vae sha256' "$OUT/env.log") vs $want"
say "VAE: the official file ($want)"
for pic in "$W/ideogram4/assets/samples/collage_landscape.jpg" "$W/hidream/assets/edit/test.jpg"; do
  nice -n 19 "$W/py/bin/python" -I "$FX/ideogram_norm.py" "$W/vae" "$W/ideogram4/src" "$pic" "$OUT" > "$OUT/norm-$(basename "$pic").log" 2>&1 ||
    fail "$(basename "$pic"): $(tail -5 "$OUT/norm-$(basename "$pic").log")"
  say "$(basename "$pic"): $(tail -1 "$OUT/norm-$(basename "$pic").log")"
done
say "PASS: both decodes of each picture are in the artifact (*-norm-crops.jpg: source | Ideogram's decode | the BatchNorm decode | |diff| x4)"
