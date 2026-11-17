#!/bin/bash
# The upstream side's environment and weights on the pod (se-156), run detached as root from /root/parity as soon as SSH
# answers. The weights start first (Hugging Face's CLI needs no torch); the environment then takes the pod's own CUDA torch:
# the image's Python, else a TensorD generation's env once our first run installed one, else PyPI's. Progress in setup.log;
# markers: env.ok, dl-video.ok, dl-image.ok (or *.fail).
set -u
cd /root/parity || exit 1
log() { echo "$(date -u +%T) $*" >> setup.log; }
log "host: $(nproc) cpus; $(free -g | awk '/^Mem:/{print $2 " GiB RAM, " $7 " available"}'); $(df -BG --output=avail /root | tail -1 | tr -d ' ') free on /root"
nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader >> setup.log 2>&1
command -v uv > /dev/null || { python3 -m pip install -q uv >> setup.log 2>&1 || curl -LsSf https://astral.sh/uv/install.sh | sh >> setup.log 2>&1; }
export PATH=$HOME/.local/bin:$HOME/.cargo/bin:$PATH
DEPS=(diffusers==0.43.0 transformers==5.19.0 accelerate==1.15.0 'huggingface_hub[hf_xet]>=1.32' safetensors av pillow numpy)
uv venv -q --python 3.12 /root/up >> setup.log 2>&1 && uv pip install -q --python /root/up/bin/python "${DEPS[@]}" >> setup.log 2>&1 ||
  { log "the environment's packages failed"; touch env.fail dl-video.fail dl-image.fail; exit 1; }

export HF_XET_HIGH_PERFORMANCE=1
hf() { /root/up/bin/hf download "$@" >> download.log 2>&1; }
(
  t=$SECONDS
  hf MiniMaxAI/MiniMax-H3 --include model_index.json modular_model_index.json 'scheduler/*' 'audio_scheduler/*' 'processor/*' \
    'tokenizer/*' 'text_encoder/*' 'transformer/*' 'transformer_ref/*' 'vae/*' 'audio_vae/*' &&
    hf alibaba-pai/MiniMax-H3-Acc-LoRAs --include minimax_h3_pdd.py MiniMax-H3-FL2VA-Acc-8Step.safetensors \
      MiniMax-H3-Ref2VA-Acc-8Step.safetensors && { log "video weights in $((SECONDS - t)) s"; touch dl-video.ok; } ||
    { log "video weights failed"; touch dl-video.fail; }
  t=$SECONDS
  hf PrunaAI/P-H3-Image --include modular_model_index.json 'scheduler/*' 'audio_scheduler/*' 'processor/*' 'tokenizer/*' \
    'text_encoder/*' 'text_connector/*' 'transformer/*' 'vae/*' &&
    hf MiniMaxAI/MiniMax-H3-Image --include model_index.json modular_model_index.json 'scheduler/*' 'processor/*' 'tokenizer/*' \
      'text_encoder/*.json' 'transformer_diffusers/*' 'vae_diffusers/*' && { log "image weights in $((SECONDS - t)) s"; touch dl-image.ok; } ||
    { log "image weights failed"; touch dl-image.fail; }
  log "$(df -BG --output=avail /root | tail -1 | tr -d ' ') free on /root after the downloads"
) &

# A Python 3.12 whose torch sees the card, waited for up to 40 min (our first run's install makes one).
TORCH=''
for _ in $(seq 80); do
  for py in python3.12 python3 /usr/local/bin/python3 /opt/*/bin/python3 /var/lib/cozy/tensord/generations/*/env/bin/python; do
    [ -x "$(command -v "$py" 2> /dev/null)" ] || continue
    "$py" -c 'import sys, torch; assert sys.version_info[:2] == (3, 12) and torch.cuda.is_available()' > /dev/null 2>&1 &&
      { TORCH=$(command -v "$py"); break 2; }
  done
  sleep 30
done
if [ -n "$TORCH" ]; then
  site=$("$TORCH" -c 'import os, torch; print(os.path.dirname(os.path.dirname(torch.__file__)))')
  # After the venv's own packages, so diffusers 0.43 and transformers 5.19 win over a generation's.
  echo "import sys; sys.path.append('$site')" > "$(/root/up/bin/python -c 'import sysconfig; print(sysconfig.get_paths()["purelib"])')/zz_torch.pth"
  log "torch from $TORCH ($site)"
else
  log "no CUDA torch on the pod: PyPI's"
  uv pip install -q --python /root/up/bin/python torch >> setup.log 2>&1 || { log "torch failed"; touch env.fail; exit 1; }
fi
/root/up/bin/python -c 'import torch, diffusers, transformers; print("env", torch.__version__, torch.version.cuda, diffusers.__version__, transformers.__version__, torch.cuda.get_device_name(0))' >> setup.log 2>&1 \
  && touch env.ok || { log "the environment's check failed"; touch env.fail; }
wait
