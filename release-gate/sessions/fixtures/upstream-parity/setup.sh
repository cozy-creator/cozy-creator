#!/bin/bash
# upstream-parity's references on the rental, as root in /root/up: two Python environments (a: Diffusers 0.41 and
# Transformers 5.20.2 for Qwen Image 1 and Qwen3; b: Diffusers 0.43 and Transformers 5.19 for PAN2 and BiRefNet, the
# versions each package locks), then each group's weights from Hugging Face at its pinned revision, in the order the
# session runs them. A group's `<group>.fetched` (or `.failed`) marks it.
set -uo pipefail
cd /root/up
export HF_HOME=/root/hf PATH=$HOME/.local/bin:$PATH
command -v uv > /dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh > uv.log 2>&1
command -v uv > /dev/null || { echo "no uv"; exit 1; }
common=(torch torchvision accelerate safetensors pillow numpy "huggingface_hub[hf_xet]")
env_a() { uv venv -q --python 3.12 a && uv pip install -q --python a/bin/python "${common[@]}" "diffusers==0.41.0" "transformers==5.20.2" peft; }
env_b() { uv venv -q --python 3.12 b && uv pip install -q --python b/bin/python "${common[@]}" "diffusers==0.43.0" "transformers==5.19.0" timm einops kornia opencv-python-headless requests; }
(env_a > a.log 2>&1 && touch a.ok || touch a.failed) &
(env_b > b.log 2>&1 && touch b.ok || touch b.failed) &
wait
for g in qwen-image:a birefnet:b qwen3:a pan2:b; do
  name=${g%%:*} env=${g#*:}
  [ -f "$env.ok" ] || { touch "$name.failed"; continue; }
  if "$env/bin/python" upstream.py "$name" out fx --fetch > "fetch-$name.log" 2>&1; then touch "$name.fetched"; else touch "$name.failed"; fi
done
