#!/bin/bash
# ggml-kernels, on the pod: stock llama.cpp <tag> (unpatched, CUDA, this card's target, no flash attention) and its
# test-backend-ops perf over our cases (shapes.py tests), console output in /w/out/llama-perf.txt.
#   pod.sh <tag> <sha256 of the tag's source archive>
set -euo pipefail
TAG=$1 SUM=$2 W=/w CUDA=/usr/local/cuda-13.0
mkdir -p $W/out && cd $W
log() { echo "$(date -u +%T) $*" | tee -a $W/out/pod.log; }
export PATH=$HOME/.local/bin:$PATH
command -v uv > /dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh > /dev/null
[ -x $W/py/bin/python ] || uv venv -q -p 3.12 $W/py
uv pip install -q -p $W/py/bin/python cmake ninja
log "card: $(nvidia-smi --query-gpu=name,memory.total,driver_version,compute_cap --format=csv,noheader); $(nproc) CPUs"
CC=$(nvidia-smi --query-gpu=compute_cap --format=csv,noheader | head -1 | tr -d .)
case $CC in 120|121) ARCH=${CC}a-real ;; *) ARCH=$CC-real ;; esac
fetch() { $W/py/bin/python -c "import sys,urllib.request,shutil; shutil.copyfileobj(urllib.request.urlopen(sys.argv[1], timeout=600), open(sys.argv[2],'wb'), 1<<20)" "$1" "$2"; }
if [ ! -e $CUDA/lib/libcublas.so ]; then
  # CUDA 13.0.2 from NVIDIA's redistributables, each held to its digest: the worker image has nvcc but no cuBLAS.
  log "install the CUDA 13.0.2 toolkit"
  redist=https://developer.download.nvidia.com/compute/cuda/redist
  fetch "$redist/redistrib_13.0.2.json" /tmp/redist.json
  mkdir -p $CUDA
  for name in cuda_nvcc cuda_crt libnvvm cuda_cudart cuda_cccl libcublas; do
    read -r path sha < <($W/py/bin/python -c "import json,sys; e=json.load(open(sys.argv[1]))[sys.argv[2]]['linux-x86_64']; print(e['relative_path'], e['sha256'])" /tmp/redist.json $name)
    fetch "$redist/$path" /tmp/$name.tar.xz && echo "$sha  /tmp/$name.tar.xz" | sha256sum -c --quiet - && tar -xJf /tmp/$name.tar.xz -C $CUDA --strip-components=1 && rm /tmp/$name.tar.xz
  done
  ln -sfn lib $CUDA/lib64
fi
if [ ! -d $W/src ]; then
  fetch "https://github.com/ggml-org/llama.cpp/archive/refs/tags/$TAG.tar.gz" $W/src.tgz
  echo "$SUM  $W/src.tgz" | sha256sum -c --quiet -
  tar xzf $W/src.tgz -C $W && mv "$W/llama.cpp-$TAG" $W/src
fi
log "build llama.cpp $TAG test-backend-ops for $ARCH"
t0=$(date +%s)
[ -x $W/build/bin/test-backend-ops ] || {
  $W/py/bin/cmake -S $W/src -B $W/build -G Ninja -DCMAKE_MAKE_PROGRAM=$W/py/bin/ninja -DCMAKE_BUILD_TYPE=Release \
    -DGGML_CUDA=ON -DGGML_CUDA_FA=OFF -DCUDAToolkit_ROOT=$CUDA -DCMAKE_CUDA_COMPILER=$CUDA/bin/nvcc \
    -DCMAKE_CUDA_ARCHITECTURES=$ARCH -DLLAMA_CURL=OFF -DLLAMA_BUILD_TESTS=ON -DLLAMA_BUILD_EXAMPLES=OFF > $W/out/build.log 2>&1
  $W/py/bin/cmake --build $W/build -j "$(nproc)" --target test-backend-ops >> $W/out/build.log 2>&1
}
log "built in $(($(date +%s) - t0)) s"
$W/py/bin/python $W/shapes.py tests > $W/tests.txt
log "test-backend-ops perf: $(wc -l < $W/tests.txt) cases"
LD_LIBRARY_PATH=$CUDA/lib:$W/build/bin $W/build/bin/test-backend-ops perf -b CUDA0 --test-file $W/tests.txt > $W/out/llama-perf.txt 2>&1
log "done: $(grep -c 'us/run' $W/out/llama-perf.txt) timed"
