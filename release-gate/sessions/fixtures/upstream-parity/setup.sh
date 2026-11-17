# setup.sh <dir>: the upstream references' two environments in <dir> (venv: HiDream-O1-Image, Transformers 4.57.1 as its
# requirements.txt pins; venv5: Ideogram 4, whose code calls Transformers 5's API, on the version in <dir>/tf5), with <dir>/bin/uv
# and a Python uv manages. The same pins on the runner and on the pod.
set -euo pipefail
W=$1
cd "$W"
export UV_PYTHON_INSTALL_DIR=$W/python UV_PYTHON_PREFERENCE=only-managed UV_HTTP_TIMEOUT=300 UV_CACHE_DIR=$W/uv-cache
uv=$W/bin/uv
for v in venv venv5; do
  "$uv" venv -q --python 3.12 "$v"
  "$uv" pip install -q --python "$v/bin/python" torch==2.14.1 torchvision==0.29.1 --index-url https://download.pytorch.org/whl/cu130
done
# Diffusers for HiDream's schedulers: Tencent's 0.35.2, the newest on Transformers 4.57's huggingface_hub < 1.
"$uv" pip install -q --python venv/bin/python transformers==4.57.1 diffusers==0.35.2 accelerate einops safetensors \
  'huggingface_hub[hf_xet]<1' pillow numpy scipy tqdm
"$uv" pip install -q --python venv5/bin/python "transformers==$(cat tf5)" accelerate einops safetensors 'huggingface_hub[hf_xet]' \
  pillow numpy sentencepiece requests bitsandbytes
rm -rf "$W/uv-cache"
echo "environments ready"
