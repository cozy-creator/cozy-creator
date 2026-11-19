#!/bin/bash
# ggml-kernels, on the pod: stock llama.cpp <tag> (unpatched, CUDA, this card's target, no flash attention) and its
# test-backend-ops perf over our cases (shapes.py tests), console output in /w/out/llama-perf.txt.
#   pod.sh <tag> <sha256 of the tag's source archive>
# The session's runner builds it first with BUILD_ONLY=1, ARCH (no GPU there), W and CUDA under its own folder.
set -euo pipefail
TAG=$1 SUM=$2 W=${W:-/w} CUDA=${CUDA:-/usr/local/cuda-13.0}
mkdir -p $W/out && cd $W
log() { echo "$(date -u +%T) $*" | tee -a $W/out/pod.log; }
export PATH=$HOME/.local/bin:$PATH
# The worker image has no curl: Python fetches (its own, until the venv's).
PY0=$(command -v python3 || echo /opt/cozy/python/bin/python)
get() { $PY0 -c "import sys,urllib.request,shutil; shutil.copyfileobj(urllib.request.urlopen(sys.argv[1], timeout=600), open(sys.argv[2],'wb'), 1<<20)" "$1" "$2"; }
# uv: the machine's own, else its release binary (no installer: that needs curl too).
if ! command -v uv > /dev/null; then
  found=$(ls /opt/cozy/bin/uv /opt/cozy/*/bin/uv "$HOME"/.cozy/bin/uv 2> /dev/null | head -1 || true)
  mkdir -p "$HOME/.local/bin"
  if [ -n "$found" ]; then
    ln -sf "$found" "$HOME/.local/bin/uv"
  else
    get https://github.com/astral-sh/uv/releases/download/0.9.5/uv-x86_64-unknown-linux-gnu.tar.gz /tmp/uv.tgz
    $PY0 -c "import sys,tarfile; t=tarfile.open(sys.argv[1]); m=t.getmember('uv-x86_64-unknown-linux-gnu/uv'); m.name='uv'; t.extract(m, sys.argv[2])" /tmp/uv.tgz "$HOME/.local/bin"
  fi
fi
[ -x $W/py/bin/python ] || uv venv -q -p 3.12 $W/py
uv pip install -q -p $W/py/bin/python cmake ninja
if [ -z "${ARCH:-}" ]; then
  log "card: $(nvidia-smi --query-gpu=name,memory.total,driver_version,compute_cap --format=csv,noheader); $(nproc) CPUs"
  CC=$(nvidia-smi --query-gpu=compute_cap --format=csv,noheader | head -1 | tr -d .)
  case $CC in 120|121) ARCH=${CC}a-real ;; *) ARCH=$CC-real ;; esac
else
  log "building for $ARCH, no GPU asked; $(nproc) CPUs"
fi
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
# The redistributables' libraries by path at link time: nothing puts them on ld's path, and without it the executable can't
# resolve libggml-cuda's libcudart and libcublas symbols (44297995601). Where no driver is installed (the runner), the
# toolkit's stub stands in for libcuda.so.1, for the link only.
link=$CUDA/lib
if ! ldconfig -p 2> /dev/null | grep -q 'libcuda\.so\.1 ' && [ -e $CUDA/lib/stubs/libcuda.so ]; then
  mkdir -p $W/stub && ln -sf $CUDA/lib/stubs/libcuda.so $W/stub/libcuda.so.1 && link=$link:$W/stub
fi
export LD_LIBRARY_PATH=$link:${LD_LIBRARY_PATH:-}
[ -x $W/build/bin/test-backend-ops ] || {
  $W/py/bin/cmake -S $W/src -B $W/build -G Ninja -DCMAKE_MAKE_PROGRAM=$W/py/bin/ninja -DCMAKE_BUILD_TYPE=Release \
    -DGGML_CUDA=ON -DGGML_CUDA_FA=OFF -DCUDAToolkit_ROOT=$CUDA -DCMAKE_CUDA_COMPILER=$CUDA/bin/nvcc \
    -DCMAKE_EXE_LINKER_FLAGS="-Wl,-rpath-link,$link" -DCMAKE_SHARED_LINKER_FLAGS="-Wl,-rpath-link,$link" \
    -DCMAKE_CUDA_ARCHITECTURES=$ARCH -DLLAMA_CURL=OFF -DLLAMA_BUILD_TESTS=ON -DLLAMA_BUILD_EXAMPLES=OFF > $W/out/build.log 2>&1
  # A deadline, so a stalled build costs 45 minutes and says so, not the session's whole cap.
  timeout 2700 $W/py/bin/cmake --build $W/build -j "$(nproc)" --target test-backend-ops >> $W/out/build.log 2>&1 ||
    { log "FAILED build after $(($(date +%s) - t0)) s"; exit 1; }
}
log "built in $(($(date +%s) - t0)) s"
[ -z "${BUILD_ONLY:-}" ] || exit 0
$W/py/bin/python $W/shapes.py tests > $W/tests.txt
log "test-backend-ops perf: $(wc -l < $W/tests.txt) cases"
t0=$(date +%s)
# An hour, and what it timed by then stands (an A40 took over 30 min: 44438719553); the cases it didn't reach read unmeasured.
LD_LIBRARY_PATH=$CUDA/lib:$W/build/bin timeout 3600 $W/build/bin/test-backend-ops perf -b CUDA0 --test-file $W/tests.txt > $W/out/llama-perf.txt 2>&1 ||
  { log "test-backend-ops ended after $(($(date +%s) - t0)) s with $(grep -c 'us/run' $W/out/llama-perf.txt) timed"
    grep -q 'us/run' $W/out/llama-perf.txt || exit 1; }
log "done: $(grep -c 'us/run' $W/out/llama-perf.txt) timed in $(($(date +%s) - t0)) s"
