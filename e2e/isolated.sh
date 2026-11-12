#!/usr/bin/env bash
# e2e/skew/run.sh --admin --command: OUT INSTALLED_COZY RUNTIME TENSORFS cli|imports|cpu <that suite's run.sh arguments>.
# A suite against its own isolated Hub. Signed in as that Hub's admin, it first publishes there what the suite reads
# from a Hub, as production holds it. The cli suite's shards read, at the root, from run.sh's --packages checkout,
# qwen-image-2, minimax-h3, sdxl and birefnet (web-palette: every first-party package), and a stand-in minimax-h3-tools (hub_away locks it from the
# root index, hub_dependency calls its echo); then the models their cells download: BiRefNet's weights from the Hugging Face
# revision production mirrors, and random weights for queued_download's two. macOS and Windows run no models and need no
# weights: their shards take the packages alone. Imports stages Hub-dependent locks against its own published dependencies.
# Then this computer's cozy is removed, and the suite starts as on a fresh runner, signed out, in
# this process's place: nothing of this script is left for the suite's own removal of cozy to find.
set -euo pipefail
[ "$#" -ge 5 ] || { echo "usage: isolated.sh OUT INSTALLED_COZY RUNTIME TENSORFS cli|imports|cpu <run.sh arguments>" >&2; exit 2; }
[ "${GITHUB_ACTIONS:-}" = true ] || { echo "isolated.sh removes this computer's cozy: a fresh GitHub runner only" >&2; exit 2; }
seed=$1 cozy_bin=$2 runtime=$3 tensorfs=$4 suite=$5
shift 5
here=$(cd "$(dirname "$0")" && pwd) start=$PWD
. "$here/test-hub.sh"
require_ci_test_hub
# The SDK has its own endpoint setting; COZY_HUB_URL alone covers only Cozy's clients.
export TENSORFS_ENDPOINT="${COZY_TEST_HUB_ORIGIN:?}"
shard='' packages='' published=''
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
  case ${args[i]} in
    --shard) shard=${args[i + 1]} ;;
    --packages) packages=$(cd "${args[i + 1]}" && pwd) ;;
    --published | --pins) published=${args[i]} ;;
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

# relay: e2e/cli/relay published as the admin's own account's package (not the root): its lock takes the stand-in
# minimax-h3-tools from the root's index (hub_published, cl-872). A refusal is the cell's to report: what it said is exported.
relay() {
  local copy=$seed/packages/e2e-relay account
  rm -rf "$copy" && mkdir -p "$seed/packages" && cp -r "$here/cli/relay" "$copy"
  account=$(cozy --json whoami | jq -r '.account // empty')
  export E2E_RELAY_PUBLISHED=$seed/publish-e2e-relay.json
  if (cd "$copy" && cozy --json package publish -y) > "$E2E_RELAY_PUBLISHED" 2>&1; then
    export E2E_RELAY=$account/e2e-relay
    log "published $E2E_RELAY 0.1.0"
  else
    E2E_RELAY_REFUSED=$(tail -c 400 "$E2E_RELAY_PUBLISHED" | tr '\n' ' ')
    export E2E_RELAY_REFUSED
    log "publishing e2e-relay as $account was refused: $E2E_RELAY_REFUSED"
  fi
}

# held: e2e-relay's code published again as `e2e-held` 0.1.0 and 0.2.0rc1, as a package built for the open RC's Runtime is
# published (cl-922): newer by PEP 440, for machines on the RC only; and as `e2e-newer` 0.1.0 and 0.1.1 (held_prerelease,
# cl-952, se-128). Then floors (runtime_floor, cl-952): `e2e-floor` 0.1.0 (cozy-runtime>=0.45) and 0.2.0
# (cozy-runtime>=0.45.1,<0.46); and `e2e-lies` 0.1.0, whose cozy-runtime>=0.44 lacks the `call` it imports, which publishing
# refuses (what it said is exported).
held() {
  local copy=$seed/packages/e2e-held account name version runtime
  account=$(cozy --json whoami | jq -r '.account // empty')
  for name in e2e-held:0.1.0 e2e-held:0.2.0rc1 e2e-newer:0.1.0 e2e-newer:0.1.1 e2e-floor:0.1.0 'e2e-floor:0.2.0:>=0.45.1,<0.46'; do
    IFS=: read -r name version runtime <<< "$name"
    rm -rf "$copy" && mkdir -p "$seed/packages" && cp -r "$here/cli/relay" "$copy"
    sed -i -e "s/^name = \"e2e-relay\"$/name = \"$name\"/" -e "s/^version = \"0.1.0\"$/version = \"$version\"/" "$copy/pyproject.toml"
    [ -z "$runtime" ] || sed -i "s/\"cozy-runtime>=0.45\"/\"cozy-runtime$runtime\"/" "$copy/pyproject.toml"
    if ! (cd "$copy" && cozy --json package publish -y) > "$seed/publish-$name-$version.json" 2>&1; then
      E2E_HELD_REFUSED="$name $version: $(tail -c 400 "$seed/publish-$name-$version.json" | tr '\n' ' ')"
      export E2E_HELD_REFUSED
      return 0
    fi
    log "published $account/$name $version"
  done
  export E2E_HELD=$account/e2e-held E2E_NEWER=$account/e2e-newer E2E_FLOOR=$account/e2e-floor
  rm -rf "$copy" && cp -r "$here/cli/relay" "$copy"
  sed -i -e 's/^name = "e2e-relay"$/name = "e2e-lies"/' -e 's/"cozy-runtime>=0.45"/"cozy-runtime>=0.44"/' "$copy/pyproject.toml"
  export E2E_LIES=$seed/publish-e2e-lies.json
  if (cd "$copy" && cozy --json package publish -y) > "$E2E_LIES" 2>&1; then log "published $account/e2e-lies 0.1.0, which it should have refused"
  else log "publishing e2e-lies was refused: $(tail -c 300 "$E2E_LIES" | tr '\n' ' ')"; fi
}

# mirror <source> <org/name> <lane> <release> <page> [upload flags...]: a model uploaded by this computer's machine and
# published, as an admin mirrors an author's model.
mirror() {
  local name=${2//\//-} commit
  cozy --json model upload "$1" "$2" --lane "$3" --mirror "$5" --on local -y "${@:6}" > "$seed/upload-$name.json" 2>&1 \
    || { echo "uploading $2 failed: $(tail -c 600 "$seed/upload-$name.json")" >&2; return 1; }
  commit=$(python3 "$here/model-details.py" "$2" | jq -r '.checkpoints[0].checkpoint // empty')
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

case $suite:$shard in
  cli:entrypoints | cli:executors | cli:projectdb-cancel | cli:local-wake) needs='' ;;
  cli:runs | cli:local-code) needs='packages birefnet' ;;
  cli:commands) needs='packages birefnet slots' ;;
  # web-localfit's heavy tool paints its edit base with BiRefNet (a take is one since cl-909); only Linux's web-core runs it.
  cli:web-core) needs='packages'; [ "${RUNNER_OS:-Linux}" != Linux ] || needs='packages birefnet' ;;
  cli:follow) needs='packages downloads' ;;
  cli:lines) needs='packages birefnet downloads' ;;
  cli:web-palette) needs='packages catalog' ;;
  cli:web-models) needs='packages loras' ;;
  cli:) needs='packages birefnet downloads slots' ;;
  cli:*) needs='packages' ;;
  imports:*)
    # Production's published releases are what --published and --pins ask about, and CI reads no production (xs-169).
    [ -z "$published" ] || { echo "imports $published asks about production's releases: CI runs only on an isolated Hub; the checkout's packages run without it" >&2; exit 2; }
    needs='' ;;
  cpu:*) needs='' ;;
  *) echo "isolated.sh runs the cli, imports or cpu suite, not '$suite'" >&2; exit 2 ;;
esac

# CPU's author/binder cells use the already-installed candidate and authenticated fixture.
# Its ordinary executor cells remain unchanged and use the package checkout and its locks.
if [ "$suite" = cpu ]; then
  project=$seed/project
  mkdir -p "$project"
  (cd "$project" && cozy --json new --unencrypted > /dev/null)
  log "the cpu suite starts against its owned Hub with the installed candidate"
  exec bash "$here/cpu/run.sh" "$@" --cozy "$cozy_bin" --project "$project"
fi

if [ "$suite" = imports ]; then
  [ -n "$packages" ] || { echo "imports needs the cohort packages checkout" >&2; exit 2; }
  staged=$seed/import-packages report=$seed/imports-fixture.json
  python3 "$here/imports/fixture.py" prepare --source "$packages" --staged "$staged" --report "$report"
  mkdir -p "$seed/project"
  (cd "$seed/project" && cozy --json new --unencrypted > /dev/null)
  # Calcifer's private lock operation and its uv must trust the fixture during regeneration too.
  sudo cp "${COZY_TEST_HUB_CA:?}" /usr/local/share/ca-certificates/cozy-test-hub.crt
  sudo update-ca-certificates > "$seed/imports-ca.log" 2>&1
  export SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
  systemctl --user set-environment SSL_CERT_FILE="$SSL_CERT_FILE"
  systemctl --user restart cozy-calcifer > "$seed/imports-calcifer-restart.log" 2>&1
  while IFS= read -r dependency; do
    publish "$staged/$dependency"
  done < <(jq -r '.dependencies[].directory' "$report")
  while IFS= read -r package; do
    python3 "$here/package-lock.py" --cozy "$cozy_bin" --root "$staged/$package" > "$seed/imports-lock-$package.json" \
      2> "$seed/imports-lock-$package.err"
  done < <(jq -r '.relock[].directory' "$report")
  python3 "$here/imports/fixture.py" verify --report "$report" --origin "${COZY_TEST_HUB_ORIGIN:?}"
  COZY_IMPORTS_REQUIRED=$(jq -r '[.relock[].name] | join(" ")' "$report")
  export COZY_IMPORTS_REQUIRED
  for ((i = 0; i < ${#args[@]}; i++)); do
    [ "${args[i]}" != --packages ] || args[i + 1]=$staged
  done
  packages=$staged
  log "imports locks verified against the owned Hub; authoritative checkout unchanged"
fi

if [ -n "$needs" ]; then
  [ -n "$packages" ] || { echo "the isolated Hub's first-party packages come from run.sh's --packages checkout" >&2; exit 2; }
  mkdir -p "$seed/project"
  cd "$seed/project" && cozy --json new --unencrypted > /dev/null
  # A stand-in for the root index's minimax-h3-tools, which the packages no longer hold (combined into minimax-h3).
  tools=$seed/minimax-h3-tools
  rm -rf "$tools" && cp -r "$here/../release-gate/suites/hub/pkg" "$tools"
  # Its module by its name, as a package's is: local code imports it (hub_dependency, cl-871). sed without -i: BSD's
  # (macOS) takes a suffix there.
  mv "$tools/ci_hub_pkg" "$tools/minimax_h3_tools"
  for f in pyproject.toml package.toml; do
    sed -e 's/^name = "ci-hub-pkg"/name = "minimax-h3-tools"/' -e 's/ci_hub_pkg/minimax_h3_tools/g' "$tools/$f" > "$tools/$f.next" && mv "$tools/$f.next" "$tools/$f"
  done
  publish "$tools"
  for p in qwen-image-2 minimax-h3 sdxl birefnet; do publish "$packages/$p"; done
  # Every other first-party package, as production's catalog holds them: what the palette lists (cl-896). Orbit's lock takes minimax-h3
  # from the root index, under the Hub's own CA, as relay's does. One refused is the cell's to find missing.
  if [[ " $needs " == *" catalog "* ]]; then
    sudo cp "${COZY_TEST_HUB_CA:?}" /usr/local/share/ca-certificates/cozy-test-hub.crt && sudo update-ca-certificates > "$seed/ca.log" 2>&1
    systemctl --user set-environment SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
    systemctl --user restart cozy-calcifer > "$seed/calcifer-restart.log" 2>&1 || true
    for dir in "$packages"/*/; do
      dir=${dir%/} && case ${dir##*/} in qwen-image-2 | minimax-h3 | sdxl | birefnet) continue ;; esac
      grep -q '^\[project.entry-points."cozy.application"\]' "$dir/pyproject.toml" 2> /dev/null || continue
      publish "$dir" || log "${dir##*/} not published: $(tail -c 300 "$seed/publish-${dir##*/}.json" | tr '\n' ' ')"
    done
    # A release locked to the Runtime before this one, as flux-3-klein 0.7.0 is on production (run 46790): the palette leaves it out.
    old=$seed/e2e-runtime-044
    rm -rf "$old" && cp -r "$here/cli/project/boxed" "$old"
    sed -e 's/^name = "e2e-boxed"/name = "e2e-runtime-044"/' -e 's/"cozy-runtime\[media\]>=[^"]*"/"cozy-runtime[media]>=0.44.0,<0.45"/' "$old/pyproject.toml" > "$old/pyproject.next" && mv "$old/pyproject.next" "$old/pyproject.toml"
    publish "$old" || log "e2e-runtime-044 not published: $(tail -c 300 "$seed/publish-e2e-runtime-044.json" | tr '\n' ' ')"
  fi
  case $shard in
    local-code | '')
      # Its lock reads the Hub's index, under the Hub's own CA: Calcifer's uv takes the system's bundle once restarted.
      sudo cp "${COZY_TEST_HUB_CA:?}" /usr/local/share/ca-certificates/cozy-test-hub.crt && sudo update-ca-certificates > "$seed/ca.log" 2>&1
      systemctl --user set-environment SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
      systemctl --user restart cozy-calcifer > "$seed/calcifer-restart.log" 2>&1 || true
      relay
      held ;;
  esac
  if [[ " $needs " == *" birefnet "* || " $needs " == *" downloads "* || " $needs " == *" loras "* ]]; then
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
  if [[ " $needs " == *" loras "* ]]; then
    # What H3's Models & LoRAs lists (web-models, cl-934): its default model, a stand-in its family names, and LoRAs of that family.
    h3=$(sed -n 's/^_MODEL = "\(.*\)"$/\1/p' "$packages/minimax-h3/h3.py")
    [ -n "$h3" ] || { echo "minimax-h3/h3.py names no _MODEL" >&2; exit 1; }
    random_weights "$seed/minimax-h3.safetensors" 1
    mirror "$seed/minimax-h3.safetensors" "${h3%@*}" bf16 "${h3#*@}" https://huggingface.co/MiniMaxAI/MiniMax-H3 --source-format as-is
    cozy --json model edit "${h3%@*}" --family minimax-h3 > "$seed/edit-minimax-h3.json" 2>&1 \
      || { echo "naming ${h3%@*}'s family failed: $(tail -c 600 "$seed/edit-minimax-h3.json")" >&2; exit 1; }
    # An H3 LoRA as musubi-tuner writes it (e2e/release/synthetic.sh's), one rank of one block.
    python3 -I - "$seed/vfx-edit.safetensors" <<'PYH3'
import json, struct, sys
stem, rank = "lora_unet_blocks_0_attn_qkv_proj", 1
shapes = {f"{stem}.lora_down.weight": [rank, 5376], f"{stem}.lora_up.weight": [3 * 7168, rank]}
header, data, at = {}, b"", 0
for key, shape in shapes.items():
    size = 2 * shape[0] * shape[1]
    header[key] = {"dtype": "F16", "shape": shape, "data_offsets": [at, at + size]}
    data, at = data + b"\x00\x3c" * (size // 2), at + size
header[f"{stem}.alpha"] = {"dtype": "F32", "shape": [], "data_offsets": [at, at + 4]}
data += struct.pack("<f", 1.0)
header["__metadata__"] = {"ss_network_module": "networks.lora_minimax_h3", "ss_minimax_h3_task": "ref2va", "ss_minimax_h3_base_family": "ref2va"}
raw = json.dumps(header).encode()
raw += b" " * (-len(raw) % 8)
open(sys.argv[1], "wb").write(struct.pack("<Q", len(raw)) + raw + data)
PYH3
    # Two of its family: an edit LoRA, and one whose card note is long (loraNotes.ts), which the picker wraps.
    for lora in alissonerdx/minimax-h3-vfx-edit nrdx/bfs-body-swap-selective-lora; do
      cozy --json lora upload "$seed/vfx-edit.safetensors" "$lora" --lane f16 --mirror https://huggingface.co/Alissonerdx/Minimax-H3-ComfyUI --on local -y \
        > "$seed/upload-${lora##*/}.json" 2>&1 || { echo "uploading $lora failed: $(tail -c 600 "$seed/upload-${lora##*/}.json")" >&2; exit 1; }
      commit=$(python3 "$here/model-details.py" "$lora" | jq -r '.checkpoints[0].checkpoint // empty')
      [ -n "$commit" ] || { echo "$lora has no commit after its upload" >&2; exit 1; }
      cozy --json lora publish "$lora" 0.1.0 "$commit" -y > "$seed/publish-${lora##*/}.json" 2>&1 \
        || { echo "publishing $lora failed: $(tail -c 600 "$seed/publish-${lora##*/}.json")" >&2; exit 1; }
      cozy --json lora edit "$lora" --family minimax-h3 > "$seed/edit-${lora##*/}.json" 2>&1 \
        || { echo "naming $lora's family failed: $(tail -c 600 "$seed/edit-${lora##*/}.json")" >&2; exit 1; }
    done
    log "published ${h3%@*}@${h3#*@} (a stand-in) and two LoRAs of family minimax-h3"
    rm -f "$seed"/*.safetensors
  fi
  if [[ " $needs " == *" slots "* ]]; then
    # What slot_choices (cl-943) asks about: Klein's package, and stand-ins of the models its slots and Qwen Image's and H3's judge.
    publish "$packages/flux.3-klein"
    # Written by TensorFS's own writer: the candidate's when there is one.
    case $tensorfs in '') with=tensorfs ;; [0-9]*) with=tensorfs==$tensorfs ;; *) with=$tensorfs ;; esac
    uv run -q --no-project --python 3.12 --with "$with" python "$here/cli/slot_models.py" "$seed/slots" > "$seed/slots.txt"
    while read -r -u 3 file model release page; do
      name=${model//\//-}-$release
      cozy --json model upload "$file" "$model" --lane bf16 --mirror "$page" --on local -y > "$seed/upload-$name.json" 2>&1 \
        || { echo "uploading $model $release failed: $(tail -c 600 "$seed/upload-$name.json")" >&2; exit 1; }
      commit=$(python3 "$here/model-details.py" "$model" | jq -r '[.checkpoints[] | select((.releases // []) | length == 0)][0].checkpoint // empty')
      [ -n "$commit" ] || { echo "$model has no unreleased checkpoint after its upload" >&2; exit 1; }
      cozy --json model publish "$model" "$release" "$commit" -y > "$seed/publish-$name.json" 2>&1 \
        || { echo "publishing $model $release failed: $(tail -c 600 "$seed/publish-$name.json")" >&2; exit 1; }
    done 3< "$seed/slots.txt"
    log "published $(wc -l < "$seed/slots.txt") stand-ins for slot_choices"
    rm -rf "$seed/slots"
  fi
  cd "$start"
fi

# rental_end (cli/rental.sh) signs in again as this account, with the code this development Hub logs.
E2E_HUB_EMAIL=$(cozy --json whoami 2> /dev/null | jq -r '.email // empty' || true)
E2E_HUB_LOG=$(cd "$seed/.." && pwd)/hub.log
export E2E_HUB_EMAIL E2E_HUB_LOG
# This computer as a fresh runner: signed out, Calcifer (and on Linux its machine) ended as each OS's service manager
# ends it, ~/.cozy and the TensorFS store the uploads filled gone.
cozy --json logout > "$seed/logout.json" 2>&1 || true
case ${RUNNER_OS:-Linux} in
  Linux)
    systemctl --user disable --now cozy-calcifer > "$seed/reset.log" 2>&1 || true
    pkill -f "^$HOME/.cozy/" || true
    for _ in $(seq 30); do pgrep -f "^$HOME/.cozy/" > /dev/null || break; sleep 1; done
    # The machine's transient scope (its executors) outlives Calcifer: on a runner every user scope is cozy's.
    for unit in $(systemctl --user list-units --type=scope --no-legend --plain 2> /dev/null | awk '$1 ~ /^run-/ {print $1}'); do
      systemctl --user stop "$unit" >> "$seed/reset.log" 2>&1 || true
    done
    rm -rf "$HOME/.cozy" "$HOME/.config/cozy" "${TENSORFS_HOME:-$HOME/.cache/tensorfs}"
    # uv (Calcifer's locks, the machine's installs) trusts the Hub's CA as it trusts tensorhub.com's: from the system's
    # bundle, which systemd's user manager hands Calcifer.
    sudo cp "${COZY_TEST_HUB_CA:?}" /usr/local/share/ca-certificates/cozy-test-hub.crt && sudo update-ca-certificates > "$seed/ca.log" 2>&1
    export SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
    systemctl --user set-environment SSL_CERT_FILE="$SSL_CERT_FILE"
    ps -eo pid,ppid,pcpu,rss,etime,args --sort=-pcpu | head -25 > "$seed/ps-after-reset.txt" ;;
  macOS)
    launchctl bootout "gui/$(id -u)/com.cozy-creator.calcifer" > "$seed/reset.log" 2>&1 || true
    pkill -f "^$HOME/.cozy/" || true
    for _ in $(seq 30); do pgrep -f "^$HOME/.cozy/" > /dev/null || break; sleep 1; done
    rm -rf "$HOME/.cozy" "$HOME/.config/cozy" "$HOME"/Library/LaunchAgents/com.cozy-creator.*.plist ;;
  Windows)
    powershell -NoProfile -c 'Remove-ItemProperty HKCU:\Software\Microsoft\Windows\CurrentVersion\Run cozy-calcifer -ErrorAction SilentlyContinue; Get-Process cozy -ErrorAction SilentlyContinue | Stop-Process -Force; Start-Sleep 2; Remove-Item -Recurse -Force ~\.cozy, $env:LOCALAPPDATA\cozy -ErrorAction SilentlyContinue' > "$seed/reset.log" 2>&1 || true ;;
esac
log "seeded (${needs:-nothing}), this computer's cozy removed; the $suite suite${shard:+ $shard} starts"
unset CLI M R
exec bash "$here/$suite/run.sh" "${args[@]}"
