#!/bin/bash
# ggml-ingest, on the pod: stock llama.cpp <tag>'s llama-quantize (CPU, unpatched) and its converter, the Hugging Face
# checkpoint <hf>, its bf16 GGUF and, from it, llama-quantize's file of each type (no importance matrix): <TYPE> to
# /w/<TYPE>.gguf, `pure:<TYPE>` with --pure (every tensor that type) to /w/pure-<TYPE>.gguf. Everything under /w.
#   pod.sh <tag> <sha256 of the tag's source archive> <hf repo> <type>...
set -euo pipefail
TAG=$1 SUM=$2 HF=$3 W=/w
shift 3
mkdir -p $W/out && cd $W
log() { echo "$(date -u +%T) $*" | tee -a $W/out/pod.log; }
export PATH=$HOME/.local/bin:$PATH HF_XET_HIGH_PERFORMANCE=1
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
# Each phase has its deadline and logs its time: a stall costs one deadline, and the 5-minute report names the phase.
# SCALE (the session's `scale`, default 1) multiplies every deadline, for a checkpoint several times the 9B's size.
phase() {
  local name=$1 secs=$(($2 * ${SCALE:-1})) t0=$(date +%s) code=0; shift 2
  log "$name (deadline $secs s)"
  timeout "$secs" bash -c "$*" || code=$?
  [ $code = 0 ] || { log "FAILED $name after $(($(date +%s) - t0)) s (exit $code)"; exit 1; }
  log "$name took $(($(date +%s) - t0)) s"
}
log "pod: $(nproc) CPUs, $(free -g | awk '/Mem:/{print $2}') GiB RAM, $(df -BG --output=avail $W | tail -1 | tr -d ' ') free"
[ -d $W/src ] || phase "fetch llama.cpp $TAG" 900 "$(declare -f get); PY0=$PY0; get https://github.com/ggml-org/llama.cpp/archive/refs/tags/$TAG.tar.gz $W/src.tgz &&
  echo '$SUM  $W/src.tgz' | sha256sum -c --quiet - && tar xzf $W/src.tgz -C $W && mv $W/llama.cpp-$TAG $W/src"
[ -x $W/py/bin/python ] || uv venv -q -p 3.12 $W/py
phase "converter's requirements" 900 "uv pip install -q -p $W/py/bin/python -r $W/src/requirements/requirements-convert_hf_to_gguf.txt cmake ninja 'huggingface_hub[hf_xet]'"
[ -x $W/build/bin/llama-quantize ] || phase "build llama-quantize ($TAG, CPU)" 1800 "$W/py/bin/cmake -S $W/src -B $W/build -G Ninja -DCMAKE_MAKE_PROGRAM=$W/py/bin/ninja -DCMAKE_BUILD_TYPE=Release \
    -DGGML_CUDA=OFF -DLLAMA_CURL=OFF -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF > $W/out/build.log 2>&1 &&
  $W/py/bin/cmake --build $W/build -j $(nproc) --target llama-quantize >> $W/out/build.log 2>&1"
phase "download $HF" 1800 "$W/py/bin/hf download $HF --local-dir $W/hf > $W/out/download.log 2>&1"
phase "convert to a bf16 GGUF" 1800 "PYTHONPATH=$W/src/gguf-py $W/py/bin/python $W/src/convert_hf_to_gguf.py $W/hf --outtype bf16 --outfile $W/bf16.gguf > $W/out/convert.log 2>&1"
for type in "$@"; do
  case $type in
    pure:*) phase "llama-quantize --pure ${type#pure:}" 1800 "$W/build/bin/llama-quantize --pure $W/bf16.gguf $W/pure-${type#pure:}.gguf ${type#pure:} > $W/out/quantize-$type.log 2>&1" ;;
    *) phase "llama-quantize $type" 1800 "$W/build/bin/llama-quantize $W/bf16.gguf $W/$type.gguf $type > $W/out/quantize-$type.log 2>&1" ;;
  esac
done
rm -f $W/bf16.gguf
log "done"
