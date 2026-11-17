#!/bin/bash
# ggml-ingest, on the pod: stock llama.cpp <tag>'s llama-quantize (CPU, unpatched) and its converter, the Hugging Face
# checkpoint <hf>, its bf16 GGUF and, from it, llama-quantize's Q8_0 and Q4_K_M (no importance matrix). Everything under /w.
#   pod.sh <tag> <sha256 of the tag's source archive> <hf repo>
set -euo pipefail
TAG=$1 SUM=$2 HF=$3 W=/w
mkdir -p $W/out && cd $W
log() { echo "$(date -u +%T) $*" | tee -a $W/out/pod.log; }
export PATH=$HOME/.local/bin:$PATH HF_XET_HIGH_PERFORMANCE=1
command -v uv > /dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh > /dev/null
if [ ! -d $W/src ]; then
  curl -fsSL --retry 5 -o $W/src.tgz "https://github.com/ggml-org/llama.cpp/archive/refs/tags/$TAG.tar.gz"
  echo "$SUM  $W/src.tgz" | sha256sum -c --quiet -
  tar xzf $W/src.tgz -C $W && mv "$W/llama.cpp-$TAG" $W/src
fi
[ -x $W/py/bin/python ] || uv venv -q -p 3.12 $W/py
uv pip install -q -p $W/py/bin/python -r $W/src/requirements/requirements-convert_hf_to_gguf.txt cmake ninja "huggingface_hub[hf_xet]"
log "build llama-quantize ($TAG, CPU)"
t0=$(date +%s)
[ -x $W/build/bin/llama-quantize ] || {
  $W/py/bin/cmake -S $W/src -B $W/build -G Ninja -DCMAKE_MAKE_PROGRAM=$W/py/bin/ninja -DCMAKE_BUILD_TYPE=Release \
    -DGGML_CUDA=OFF -DLLAMA_CURL=OFF -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF > $W/out/build.log 2>&1
  $W/py/bin/cmake --build $W/build -j "$(nproc)" --target llama-quantize >> $W/out/build.log 2>&1
}
log "built in $(($(date +%s) - t0)) s"
log "download $HF"
$W/py/bin/hf download "$HF" --local-dir $W/hf > $W/out/download.log 2>&1
log "convert to a bf16 GGUF"
PYTHONPATH=$W/src/gguf-py $W/py/bin/python $W/src/convert_hf_to_gguf.py $W/hf --outtype bf16 --outfile $W/bf16.gguf \
  > $W/out/convert.log 2>&1
for type in Q8_0 Q4_K_M; do
  log "llama-quantize $type"
  $W/build/bin/llama-quantize $W/bf16.gguf $W/$type.gguf $type > $W/out/quantize-$type.log 2>&1
done
rm -f $W/bf16.gguf
log "done"
