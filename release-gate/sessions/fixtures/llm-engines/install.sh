#!/bin/bash
# install.sh <what…>: on the pod. vllm, sglang, llamacpp, models. Each logs to /w/out/install-<what>.log.
set -u
export PATH=$HOME/.local/bin:/usr/local/cuda/bin:$PATH HF_XET_HIGH_PERFORMANCE=1 UV_HTTP_TIMEOUT=600
hf() { /w/client/bin/hf "$@"; }
# FlashInfer compiles kernels at first use against the venv's CUDA headers and libraries; the worker image has no -lcudart.
links() { d=/w/$1/lib/python3.12/site-packages/nvidia/cu13/lib; for l in cudart curand cublas cublasLt; do f=$(ls $d/lib$l.so.* 2> /dev/null | head -1); [ -z "$f" ] || ln -sf "$f" $d/lib$l.so; done; }
for what in "$@"; do
  t0=$(date +%s)
  case $what in
    vllm) uv venv -q --clear -p 3.12 /w/vllm && uv pip install -p /w/vllm/bin/python vllm --torch-backend=auto && links vllm ;;
    sglang) uv venv -q --clear -p 3.12 /w/sglang && uv pip install -p /w/sglang/bin/python "sglang[all]" --torch-backend=auto && links sglang ;;
    # cozy_vllm (this folder's, copied to /w/plugin): vLLM reading Cozy's lanes from the machine's TensorFS store.
    plugin) uv pip install -p /w/vllm/bin/python "tensorfs==${TENSORFS_VERSION:-0.25.1}" -e /w/plugin ;;
    llamacpp)
      mkdir -p /w/llamabin && cd /w/llamabin &&
        curl -sfL -o l.tgz https://github.com/ggml-org/llama.cpp/releases/download/b12466/llama-b12466-bin-ubuntu-cuda-12.8-x64.tar.gz &&
        curl -sfL -o c.tgz https://github.com/ggml-org/llama.cpp/releases/download/b12466/cudart-llama-b12466-bin-ubuntu-cuda-12.8-x64.tar.gz &&
        tar xzf l.tgz && tar xzf c.tgz && find /w/llamabin -name llama-server ;;
    m9b) hf download Qwen/Qwen3.5-9B --local-dir /w/models/9b ;;
    m9b-fp8) hf download RedHatAI/Qwen3.5-9B-FP8-dynamic --local-dir /w/models/9b-fp8 ;;
    m9b-w8a8) hf download RedHatAI/Qwen3.5-9B-quantized.w8a8 --local-dir /w/models/9b-w8a8 ;;
    m9b-gguf) hf download unsloth/Qwen3.5-9B-MTP-GGUF Qwen3.5-9B-Q8_0.gguf mmproj-BF16.gguf --local-dir /w/models/9b-gguf ;;
    m27b-nvfp4) hf download nvidia/Qwen3.8-27B-NVFP4 --local-dir /w/models/27b-nvfp4 ;;
    m27b-q4) hf download ggml-org/Qwen3.8-27B-GGUF Qwen3.8-27B-Q4_K_M.gguf mmproj-Qwen3.8-27B-Q8_0.gguf mtp-Qwen3.8-27B-Q8_0.gguf --local-dir /w/models/27b-q4 ;;
    m27b-fp8) hf download Qwen/Qwen3.6-27B-FP8 --local-dir /w/models/27b-fp8 ;;
    m27b-fp8q) hf download Qwen/Qwen3.8-27B-FP8 --local-dir /w/models/27b-fp8 ;;
    m27b-gguf) hf download ggml-org/Qwen3.6-27B-GGUF Qwen3.6-27B-Q8_0.gguf mmproj-Qwen3.6-27B-Q8_0.gguf mtp-Qwen3.6-27B-Q8_0.gguf --local-dir /w/models/27b-gguf ;;
    *) echo "unknown $what"; continue ;;
  esac
  echo "{\"install\":\"$what\",\"exit\":$?,\"s\":$(($(date +%s) - t0))}" >> /w/out/install.jsonl
done
