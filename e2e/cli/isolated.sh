#!/usr/bin/env bash
# e2e/skew/run.sh --admin --command: OUT INSTALLED_COZY RUNTIME TENSORFS <e2e/cli/run.sh arguments>.
# The cli suite against its own isolated Hub. Signed in as that Hub's admin, it first publishes there what the shard reads
# from a Hub, as production holds it: at the root, from run.sh's --packages checkout, the toolbox's defaults (qwen-image-2,
# minimax-h3) and birefnet, and a stand-in minimax-h3-tools (hub_away locks it from the root index); then the models the
# shard's cells download: BiRefNet's weights from the Hugging Face revision production mirrors, and random weights for
# queued_download's two. Then this computer's cozy is removed, and run.sh starts as on a fresh runner, signed out.
set -euo pipefail
[ "$#" -ge 4 ] || { echo "usage: isolated.sh OUT INSTALLED_COZY RUNTIME TENSORFS <run.sh arguments>" >&2; exit 2; }
seed=$1 cozy_bin=$2 runtime=$3 tensorfs=$4
shift 4
here=$(cd "$(dirname "$0")" && pwd) start=$PWD
shard='' packages=''
args=("$@")
for ((i = 0; i + 1 < ${#args[@]}; i++)); do
  case ${args[i]} in
    --shard) shard=${args[i + 1]} ;;
    --packages) packages=$(cd "${args[i + 1]}" && pwd) ;;
  esac
done
mkdir -p "$seed"
log() { echo "$(date -u +%H:%M:%SZ) isolated Hub: $*"; }
cozy() { "$cozy_bin" "$@"; }
J() { jq -e "$@" > /dev/null; }

# publish <dir>: a package's own directory published at the root, from a copy (publication writes its lock).
publish() {
  local copy=$seed/packages/${1##*/}
  rm -rf "$copy" && mkdir -p "$seed/packages" && cp -r "$1" "$copy"
  (cd "$copy" && cozy --json package publish --root -y) > "$seed/publish-${1##*/}.json" 2>&1 \
    || { echo "publishing ${1##*/} at the isolated Hub's root failed: $(tail -c 600 "$seed/publish-${1##*/}.json")" >&2; return 1; }
  log "published ${1##*/} $(sed -n 's/^version = "\(.*\)"$/\1/p' "$copy/pyproject.toml" | head -1) at the root"
}

# mirror <source> <org/name> <lane> <release> <page> [upload flags...]: a model uploaded by this computer's machine and
# published, as an admin mirrors an author's model.
mirror() {
  local name=${2//\//-} commit
  cozy --json model upload "$1" "$2" --lane "$3" --mirror "$5" --on local -y "${@:6}" > "$seed/upload-$name.json" 2>&1 \
    || { echo "uploading $2 failed: $(tail -c 600 "$seed/upload-$name.json")" >&2; return 1; }
  commit=$(python3 "$here/../model-details.py" "$2" | jq -r '.commits[0].commit // empty')
  [ -n "$commit" ] || { echo "$2 has no commit after its upload" >&2; return 1; }
  cozy --json model publish "$2" "$4" "$commit" -y > "$seed/publish-$name.json" 2>&1 \
    || { echo "publishing $2 $4 failed: $(tail -c 600 "$seed/publish-$name.json")" >&2; return 1; }
  log "published $2@$4 ($3, $commit)"
}

# random_weights <file> <MiB>: one F16 tensor per 64 MiB of random finite values, written as it is made: no chunk repeats.
random_weights() {
  python3 -I - "$1" "$2" <<'PY'
import json, os, struct, sys
path, mib = sys.argv[1], int(sys.argv[2])
FINITE = bytes(b & 0xBF for b in range(256))  # a high byte without its exponent's top bit: no Inf or NaN
size, tensors = 64 << 20, -(-int(sys.argv[2]) // 64)
header, offset = {}, 0
for i in range(tensors):
    n = min(size, (mib << 20) - offset)
    header[f"w.{i}"] = {"dtype": "F16", "shape": [n // 2], "data_offsets": [offset, offset + n]}
    offset += n
raw = json.dumps(header).encode()
raw += b" " * (-len(raw) % 8)
with open(path, "wb") as out:
    out.write(struct.pack("<Q", len(raw)) + raw)
    left = offset
    while left:
        chunk = bytearray(os.urandom(min(left, 8 << 20)))
        chunk[1::2] = chunk[1::2].translate(FINITE)
        out.write(chunk)
        left -= len(chunk)
PY
}

case $shard in
  entrypoints | executors | projectdb-cancel) needs='' ;;
  runs | commands) needs='packages birefnet' ;;
  follow) needs='packages downloads' ;;
  '') needs='packages birefnet downloads' ;;
  *) needs='packages' ;;
esac

if [ -n "$needs" ]; then
  [ -n "$packages" ] || { echo "the isolated Hub's first-party packages come from run.sh's --packages checkout" >&2; exit 2; }
  mkdir -p "$seed/project"
  cd "$seed/project" && cozy --json new > /dev/null
  # A stand-in for the root index's minimax-h3-tools, which the packages no longer hold (combined into minimax-h3).
  tools=$seed/minimax-h3-tools
  rm -rf "$tools" && cp -r "$here/../../release-gate/suites/hub/pkg" "$tools"
  sed -i 's/^name = "ci-hub-pkg"/name = "minimax-h3-tools"/' "$tools/pyproject.toml"
  publish "$tools"
  for p in qwen-image-2 minimax-h3 birefnet; do publish "$packages/$p"; done
  if [[ " $needs " == *" birefnet "* || " $needs " == *" downloads "* ]]; then
    update=()
    [ -z "$runtime" ] || update+=(--runtime "$runtime")
    [ -z "$tensorfs" ] || update+=(--tensorfs "$tensorfs")
    [ ${#update[@]} -eq 0 ] || cozy --json machine update local "${update[@]}" -y > "$seed/machine-update.json" 2>&1 \
      || { echo "the seeding machine refused the candidates: $(tail -c 600 "$seed/machine-update.json")" >&2; exit 1; }
  fi
  if [[ " $needs " == *" birefnet "* ]]; then
    mirror hf://ZhengPeng7/BiRefNet@e2bf8e4460fc8fa32bba5ea4d94b3233d367b0e4 zhengpeng7/birefnet fp16 0.1.0 https://huggingface.co/ZhengPeng7/BiRefNet
  fi
  if [[ " $needs " == *" downloads "* ]]; then
    random_weights "$seed/qwen3.5-0.8b.safetensors" 1640
    mirror "$seed/qwen3.5-0.8b.safetensors" qwen/qwen3.5-0.8b bf16 0.1.0 https://huggingface.co/Qwen/Qwen3.5-0.8B --source-format as-is
    random_weights "$seed/fontbox.safetensors" 115
    mirror "$seed/fontbox.safetensors" lilylilith/fontbox bf16 0.1.0 https://huggingface.co/lilylilith/fontbox --source-format as-is
    rm -f "$seed"/*.safetensors
  fi
  cd "$start"
fi

# This computer as a fresh runner: signed out, Calcifer and its machine ended, ~/.cozy gone.
cozy --json logout > "$seed/logout.json" 2>&1 || true
systemctl --user disable --now cozy-calcifer > "$seed/reset.log" 2>&1 || true
pkill -f "^$HOME/.cozy/" || true
for _ in $(seq 30); do pgrep -f "^$HOME/.cozy/" > /dev/null || break; sleep 1; done
rm -rf "$HOME/.cozy" "$HOME/.config/cozy"
log "seeded ($needs), this computer's cozy removed; the cli suite${shard:+ $shard} starts"
unset CLI M R
bash "$here/run.sh" "$@"
