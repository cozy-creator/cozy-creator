#!/bin/bash
# ggml-decode, on the pod: stock llama.cpp <tag>'s CUDA 12.8 release build (unpatched, a measuring harness only) serving
# <hf repo>'s <gguf> one row at a time, on the GPU this session's runs used. For each mode, plain and MTP (`draft-mtp`, 2
# drafts, as xs-212 measured), one 512-token greedy decode of the same prompt as the `decode` drive's, never stopping,
# after a 64-token warm-up; one JSON line each, from the server's own timings.
#   llama.sh <tag> <hf repo> <gguf file>
set -euo pipefail
TAG=$1 HF=$2 FILE=$3 W=/w/llama
mkdir -p $W /w/out && cd $W
log() { echo "$(date -u +%T) $*" | tee -a /w/out/llama.log >&2; }
export PATH=$HOME/.local/bin:$PATH HF_XET_HIGH_PERFORMANCE=1
# The worker image has no curl: Python fetches (its own, until the venv's).
PY0=$(command -v python3 || echo /opt/cozy/python/bin/python)
get() { $PY0 -c "import sys,urllib.request,shutil; shutil.copyfileobj(urllib.request.urlopen(sys.argv[1], timeout=600), open(sys.argv[2],'wb'), 1<<20)" "$1" "$2"; }
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
t0=$(date +%s)
if [ -z "$(find $W/bin -name llama-server 2> /dev/null)" ]; then
  log "fetch llama.cpp $TAG (CUDA 12.8 release build)"
  base=https://github.com/ggml-org/llama.cpp/releases/download/$TAG
  timeout 900 bash -c "$(declare -f get); PY0=$PY0; get $base/llama-$TAG-bin-ubuntu-cuda-12.8-x64.tar.gz $W/l.tgz && get $base/cudart-llama-$TAG-bin-ubuntu-cuda-12.8-x64.tar.gz $W/c.tgz"
  mkdir -p $W/bin && tar xzf $W/l.tgz -C $W/bin && tar xzf $W/c.tgz -C $W/bin && rm -f $W/l.tgz $W/c.tgz
fi
[ -x $W/py/bin/hf ] || { uv venv -q -p 3.12 $W/py && uv pip install -q -p $W/py/bin/python "huggingface_hub[hf_xet]"; }
if [ ! -s "$W/m/$FILE" ]; then
  log "download $HF $FILE"
  timeout 1800 $W/py/bin/hf download "$HF" "$FILE" --local-dir $W/m > /w/out/llama-download.log 2>&1
fi
log "ready in $(($(date +%s) - t0)) s; GPU memory in use before the server: $(nvidia-smi --query-gpu=memory.used --format=csv,noheader | head -1)"
L=$(find $W/bin -name llama-server | head -1)
export LD_LIBRARY_PATH=$(dirname "$L"):$(dirname "$(find $W/bin -name 'libcudart.so*' | head -1)"):${LD_LIBRARY_PATH:-}
for mode in plain mtp; do
  extra=()
  [ $mode = mtp ] && extra=(--spec-type draft-mtp --spec-draft-n-max 2)
  setsid "$L" -m "$W/m/$FILE" -ngl 999 -c 8192 -np 1 -fa on --cache-ram 0 --port 8090 "${extra[@]}" > /w/out/llama-$mode.server.log 2>&1 &
  srv=$!
  code=0
  $W/py/bin/python - "$mode" "$TAG" "$FILE" <<'EOF' || code=$?
import json, sys, time, urllib.request
mode, tag, gguf = sys.argv[1:]
url = "http://127.0.0.1:8090"
for _ in range(600):
    try:
        if urllib.request.urlopen(url + "/health", timeout=5).status == 200:
            break
    except Exception:
        time.sleep(1)
else:
    print(json.dumps({"mode": mode, "error": "server never healthy"})); sys.exit(1)
prompt = ("<|im_start|>user\nWrite a long, detailed story about a lighthouse keeper who finds a message in a bottle. Keep "
          "writing until you are stopped.<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n")
def complete(n):
    body = {"prompt": prompt, "n_predict": n, "temperature": 0, "ignore_eos": True, "cache_prompt": False}
    req = urllib.request.Request(url + "/completion", json.dumps(body).encode(), {"Content-Type": "application/json"})
    return json.load(urllib.request.urlopen(req, timeout=600))
complete(64)
t = complete(512)["timings"]
print(json.dumps({"engine": f"llama.cpp {tag}", "gguf": gguf, "mode": mode, "tokens": t.get("predicted_n"),
                  "tok_s": round(t.get("predicted_per_second", 0), 1), "prompt_tokens": t.get("prompt_n"),
                  "draft_n": t.get("draft_n"), "draft_accepted": t.get("draft_n_accepted")}))
EOF
  kill -- -$srv 2> /dev/null || true
  for _ in $(seq 1 30); do kill -0 $srv 2> /dev/null || break; sleep 1; done
  kill -9 -- -$srv 2> /dev/null || true
  [ $code = 0 ] || { tail -n 30 /w/out/llama-$mode.server.log >&2; exit 1; }
done
