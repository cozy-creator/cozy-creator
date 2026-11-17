#!/usr/bin/env bash
# e2e/skew/run.sh --admin --command: OUT INSTALLED_COZY RUNTIME TENSORFS cli|imports|cpu <that suite's run.sh arguments>.
# A suite against its own isolated Hub. Signed in as that Hub's admin, it first publishes there what the suite reads
# from a Hub, as production holds it. The cli suite's shards read, at the root, from run.sh's --packages checkout,
# qwen-image-2, minimax-h3, sdxl and birefnet, and a stand-in minimax-h3-tools (hub_away locks it from the
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
shard='' packages='' published='' only=''
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
  case ${args[i]} in
    --shard) shard=${args[i + 1]%@*} ;;  # `<shard>@<n>` repeats a shard (lib.sh)
    --packages) packages=$(cd "${args[i + 1]}" && pwd) ;;
    --only) only=${args[i + 1]} ;;
    --published) published=${args[i]} ;;
  esac
done
mkdir -p "$seed"
log() { echo "$(date -u +%H:%M:%SZ) isolated Hub: $*"; }
cozy() { "$cozy_bin" "$@"; }
J() { jq -e "$@" > /dev/null; }
if [ -n "${COZY_TEST_SEED_SDK:-}" ]; then
  export E2E_SEED_COHORT=$seed/cohort.json
  cohort_args=(--cohort "$COZY_TEST_SEED_SDK" --origin "$COZY_TEST_HUB_ORIGIN")
  uv run -q --no-project "$here/seed_sdk.py" record "${cohort_args[@]}" \
    ${packages:+--packages "$packages"} --runtime-ref "${E2E_SELECTED_RUNTIME_REF:-}" \
    --packages-ref "${E2E_SELECTED_PACKAGES_REF:-}" --receipt "$E2E_SEED_COHORT" > "$seed/cohort.txt"
  cat "$seed/cohort.txt"
  [ -z "${GITHUB_STEP_SUMMARY:-}" ] || { printf '\n### Isolated seed cohort\n\n'; cat "$seed/cohort.txt"; } >> "$GITHUB_STEP_SUMMARY"
fi

# publish <dir> [cohort]: a package's own directory published at the root, from a copy (publication writes its lock), with its
# benchmarks.json (cl-1024): its own, else release-gate's seed's rows when it has any, as a promotion carries the gate's. Only cohort
# sources take the selected SDK; deliberate old-floor/negative fixtures keep their own requirements and source selection.
publish() {
  local copy=$seed/packages/${1##*/}
  rm -rf "$copy" && mkdir -p "$seed/packages" && cp -r "$1" "$copy"
  [ -f "$copy/benchmarks.json" ] \
    || jq -e --arg p "${1##*/}" '.[$p] // empty' "$here/../release-gate/benchmarks-seed.json" > "$copy/benchmarks.json" 2> /dev/null || rm -f "$copy/benchmarks.json"
  if [ "${2:-}" = cohort ]; then
    [ -n "${COZY_TEST_SEED_SDK:-}" ] || { echo "no selected SDK reached isolated package seeding" >&2; return 1; }
    uv run -q --no-project "$here/seed_sdk.py" pin --project "$copy" "${cohort_args[@]}" || return 1
  fi
  (cd "$copy" && cozy --json package publish --root -y) > "$seed/publish-${1##*/}.json" 2>&1 \
    || { echo "publishing ${1##*/} at the isolated Hub's root failed: $(tail -c 600 "$seed/publish-${1##*/}.json")" >&2; return 1; }
  if [ "${2:-}" = cohort ]; then
    uv run -q --no-project "$here/seed_sdk.py" verify --project "$copy" "${cohort_args[@]}" --receipt "$E2E_SEED_COHORT" || return 1
  fi
  log "published ${1##*/} $(sed -n 's/^version = "\(.*\)"$/\1/p' "$copy/pyproject.toml" | head -1) at the root"
}

# relay: e2e/cli/relay published as the admin's own account's package (not the root): its lock takes the stand-in
# minimax-h3-tools from the root's index (hub_published, cl-872). A refusal is the cell's to report: what it said is exported.
relay() {
  local copy=$seed/packages/e2e-relay account
  rm -rf "$copy" && mkdir -p "$seed/packages" && cp -r "$here/cli/relay" "$copy"
  account=$(cozy --json whoami | jq -r '.account // empty')
  export E2E_RELAY_PUBLISHED=$seed/publish-e2e-relay.json
  [ -n "${COZY_TEST_SEED_SDK:-}" ] || { echo "no selected SDK reached the positive relay fixture" >&2; return 1; }
  uv run -q --no-project "$here/seed_sdk.py" pin --project "$copy" "${cohort_args[@]}" || return 1
  if (cd "$copy" && cozy --json package publish -y) > "$E2E_RELAY_PUBLISHED" 2>&1; then
    uv run -q --no-project "$here/seed_sdk.py" verify --project "$copy" "${cohort_args[@]}" --receipt "$E2E_SEED_COHORT" || return 1
    export E2E_RELAY=$account/e2e-relay
    log "published $E2E_RELAY 0.1.0"
  else
    E2E_RELAY_REFUSED=$(tail -c 400 "$E2E_RELAY_PUBLISHED" | tr '\n' ' ')
    export E2E_RELAY_REFUSED
    log "publishing e2e-relay as $account was refused: $E2E_RELAY_REFUSED"
  fi
}

# Historical release selection needs code the old Runtime can execute. The existing weightless echo has that contract;
# the positive managed relay uses the hard-cut call API, which only the RC's Runtime has.
historical_relay() {
  local copy=$1 fixture=$here/../release-gate/suites/hub/pkg
  mkdir -p "$copy"
  cp "$fixture/pyproject.toml" "$copy/pyproject.toml"
  cp -r "$fixture/ci_hub_pkg" "$copy/e2e_relay"
  sed -i -e 's/^name = "ci-hub-pkg"$/name = "e2e-relay"/' -e 's/ci_hub_pkg/e2e_relay/g' \
    -e 's/"cozy-runtime[^"]*"/"cozy-runtime>=0.45"/' "$copy/pyproject.toml"
  sed -i 's/def echo(/def relay(/' "$copy/e2e_relay/__init__.py"
}

# held: the historical weightless relay published as `e2e-held` 0.1.0 and 0.2.0rc1: newer by PEP 440, a prerelease no unpinned run
# takes (cl-1109); and as `e2e-newer` 0.1.0 and 0.1.1 (held_prerelease,
# cl-952, se-128). Then floors (runtime_floor, cl-952): `e2e-floor` 0.1.0 (cozy-runtime>=0.45) and 0.2.0
# (cozy-runtime>=0.45.1); `e2e-capped` 0.1.0 and 0.2.0 (cozy-runtime>=0.45,<0.47.0.dev0, which no Runtime of this cozy's
# minor meets, cl-1055); and `e2e-lies` 0.1.0, whose cozy-runtime>=0.44 lacks the `call` it imports, which publishing refuses (what it said
# is exported).
held() {
  local copy=$seed/packages/e2e-held account name version runtime
  account=$(cozy --json whoami | jq -r '.account // empty')
  for name in e2e-held:0.1.0 e2e-held:0.2.0rc1 e2e-newer:0.1.0 e2e-newer:0.1.1 e2e-floor:0.1.0 'e2e-floor:0.2.0:>=0.45.1' \
    e2e-capped:0.1.0 'e2e-capped:0.2.0:>=0.45,<0.47.0.dev0'; do
    IFS=: read -r name version runtime <<< "$name"
    rm -rf "$copy" && mkdir -p "$seed/packages" && historical_relay "$copy"
    sed -i -e "s/^name = \"e2e-relay\"$/name = \"$name\"/" -e "s/^version = \"0.1.0\"$/version = \"$version\"/" "$copy/pyproject.toml"
    [ -z "$runtime" ] || sed -i "s/\"cozy-runtime[^\"]*\"/\"cozy-runtime$runtime\"/" "$copy/pyproject.toml"
    if ! (cd "$copy" && cozy --json package publish -y) > "$seed/publish-$name-$version.json" 2>&1; then
      E2E_HELD_REFUSED="$name $version: $(tail -c 400 "$seed/publish-$name-$version.json" | tr '\n' ' ')"
      export E2E_HELD_REFUSED
      return 0
    fi
    log "published $account/$name $version"
  done
  export E2E_HELD=$account/e2e-held E2E_NEWER=$account/e2e-newer E2E_FLOOR=$account/e2e-floor E2E_CAPPED=$account/e2e-capped
  rm -rf "$copy" && historical_relay "$copy"
  sed -i -e 's/^name = "e2e-relay"$/name = "e2e-lies"/' -e 's/"cozy-runtime[^"]*"/"cozy-runtime>=0.44"/' "$copy/pyproject.toml"
  # Keep the precise dishonest-import assertion without a managed dependency that requires the new SDK.
  sed -i 's/from cozy_runtime.author import App$/from cozy_runtime.author import App, call/' "$copy/e2e_relay/__init__.py"
  export E2E_LIES=$seed/publish-e2e-lies.json
  if (cd "$copy" && cozy --json package publish -y) > "$E2E_LIES" 2>&1; then log "published $account/e2e-lies 0.1.0, which it should have refused"
  else log "publishing e2e-lies was refused: $(tail -c 300 "$E2E_LIES" | tr '\n' ' ')"; fi
}

# final (held_prerelease, cl-1109): the hub suite's weightless echo as a tool, published as `e2e-final` 0.1.0 and then 0.2.0rc1, a
# newer prerelease, as production has minimax-h3 0.12.0 and 0.13.0rc2 (xs-203). A refusal is the cell's to report.
final() {
  local copy=$seed/packages/e2e-final account version
  account=$(cozy --json whoami | jq -r '.account // empty')
  for version in 0.1.0 0.2.0rc1; do
    rm -rf "$copy" && mkdir -p "$seed/packages" && cp -r "$here/../release-gate/suites/hub/pkg" "$copy"
    sed -i -e 's/^name = "ci-hub-pkg"$/name = "e2e-final"/' -e "s/^version = \"0.1.0\"$/version = \"$version\"/" "$copy/pyproject.toml"
    sed -i -e "s/^RELEASE = \"0.1.0\"$/RELEASE = \"$version\"/" -e 's/^@app.entrypoint(accelerator=False)$/@app.tool(label="Echo", accelerator=False)/' \
      "$copy/ci_hub_pkg/__init__.py"
    if ! (cd "$copy" && cozy --json package publish -y) > "$seed/publish-e2e-final-$version.json" 2>&1; then
      E2E_FINAL_REFUSED="$version: $(tail -c 400 "$seed/publish-e2e-final-$version.json" | tr '\n' ' ')"
      export E2E_FINAL_REFUSED
      return 0
    fi
    log "published $account/e2e-final $version"
  done
  export E2E_FINAL=$account/e2e-final
}

# rc_floor (rc_floor, xs-194): the historical weightless relay as `e2e-rcfloor` 0.1.0, whose floor only the open RC meets
# (cozy-runtime>=0.47.0.dev0,<0.48) and which names no Runtime wheel: publishing it locks the RC's newest build, whatever that
# build's author API. What it said is exported, a refusal too, for the cell to judge.
rc_floor() {
  local copy=$seed/packages/e2e-rcfloor account
  account=$(cozy --json whoami | jq -r '.account // empty')
  rm -rf "$copy" && mkdir -p "$seed/packages" && historical_relay "$copy"
  sed -i -e 's/^name = "e2e-relay"$/name = "e2e-rcfloor"/' -e 's/"cozy-runtime[^"]*"/"cozy-runtime>=0.47.0.dev0,<0.48"/' "$copy/pyproject.toml"
  export E2E_RCFLOOR_PUBLISHED=$seed/publish-e2e-rcfloor.json E2E_RCFLOOR=$account/e2e-rcfloor
  if (cd "$copy" && cozy --json package publish -y) > "$E2E_RCFLOOR_PUBLISHED" 2>&1; then log "published $E2E_RCFLOOR 0.1.0 (cozy-runtime>=0.47.0.dev0)"
  else log "publishing e2e-rcfloor was refused: $(tail -c 300 "$E2E_RCFLOOR_PUBLISHED" | tr '\n' ' ')"; fi
}

# next (self_update, cl-978): the hub suite's weightless echo as the admin's account's `e2e-next` 0.1.0, needing cozy-runtime>=0.46.0,
# which only a cozy of the Latest's line runs.
next() {
  local copy=$seed/packages/e2e-next account
  account=$(cozy --json whoami | jq -r '.account // empty')
  rm -rf "$copy" && mkdir -p "$seed/packages" && cp -r "$here/../release-gate/suites/hub/pkg" "$copy"
  sed -i -e 's/^name = "ci-hub-pkg"$/name = "e2e-next"/' -e 's/"cozy-runtime[^"]*"/"cozy-runtime>=0.46.0"/' "$copy/pyproject.toml"
  (cd "$copy" && cozy --json package publish -y) > "$seed/publish-e2e-next.json" 2>&1 \
    || { echo "publishing e2e-next failed: $(tail -c 600 "$seed/publish-e2e-next.json")" >&2; return 1; }
  export E2E_NEXT=$account/e2e-next
  log "published $E2E_NEXT 0.1.0 (cozy-runtime>=0.46.0)"
}

# final_model (final_model, cl-1109): e2e/final-model, two checkpoints of 1 MiB of random weights published as 0.1.0 and then
# 0.2.0-rc.1, a newer prerelease label (minimaxai/minimax-h3's are 1.0.0-rc.N).
final_model() {
  local page=https://github.com/cozy-creator/cozy-creator/tree/master/e2e release first='' commit
  for release in 0.1.0 0.2.0-rc.1; do
    random_weights "$seed/final-model.safetensors" 1 "final-$release"
    cozy --json model upload "$seed/final-model.safetensors" e2e/final-model --lane bf16 --mirror "$page" --on local -y --source-format as-is \
      > "$seed/upload-final-model-$release.json" 2>&1 \
      || { echo "uploading e2e/final-model $release failed: $(tail -c 600 "$seed/upload-final-model-$release.json")" >&2; return 1; }
    commit=$(python3 "$here/model-details.py" e2e/final-model | jq -r --arg first "$first" '[.checkpoints[].checkpoint | select(. != $first)][0] // empty')
    [ -n "$commit" ] || { echo "e2e/final-model has no new checkpoint after its $release upload" >&2; return 1; }
    cozy --json model publish e2e/final-model "$release" "$commit" -y > "$seed/publish-final-model-$release.json" 2>&1 \
      || { echo "publishing e2e/final-model $release failed: $(tail -c 600 "$seed/publish-final-model-$release.json")" >&2; return 1; }
    log "published e2e/final-model@$release ($commit)"
    first=$commit
  done
  rm -f "$seed/final-model.safetensors"
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

# standin_lora <file> <org/name> <page> <family> [--trigger <words>]...: a LoRA uploaded, published at 0.1.0, its family (none
# when empty) and triggers named, as an admin mirrors an author's LoRA. $FORMAT: its --source-format, else detected; $LANE its
# lane, else f16.
standin_lora() {
  local file=$1 lora=$2 page=$3 family=$4 commit
  shift 4
  cozy --json lora upload "$file" "$lora" --lane "${LANE:-f16}" --mirror "$page" --on local -y ${FORMAT:+--source-format "$FORMAT"} \
    > "$seed/upload-${lora##*/}.json" 2>&1 \
    || { echo "uploading $lora failed: $(tail -c 600 "$seed/upload-${lora##*/}.json")" >&2; return 1; }
  commit=$(python3 "$here/model-details.py" "$lora" | jq -r '.checkpoints[0].checkpoint // empty')
  [ -n "$commit" ] || { echo "$lora has no commit after its upload" >&2; return 1; }
  cozy --json lora publish "$lora" 0.1.0 "$commit" -y > "$seed/publish-${lora##*/}.json" 2>&1 \
    || { echo "publishing $lora failed: $(tail -c 600 "$seed/publish-${lora##*/}.json")" >&2; return 1; }
  [ -n "$family" ] || return 0
  cozy --json lora edit "$lora" --family "$family" "$@" > "$seed/edit-${lora##*/}.json" 2>&1 \
    || { echo "naming $lora's family failed: $(tail -c 600 "$seed/edit-${lora##*/}.json")" >&2; return 1; }
}

# random_weights <file> <MiB> [part]: one F16 tensor per 64 MiB of random finite values, written as it is made: no chunk repeats.
# With `part`, its metadata names it: a config the Runtime constructs from.
random_weights() {
  python3 -I - "$1" "$2" "${3:-}" <<'PY'
import json, os, struct, sys
path, mib, part = sys.argv[1], int(sys.argv[2]), sys.argv[3]
FINITE = bytes(b & 0xBF for b in range(256))  # a high byte without its exponent's top bit: no Inf or NaN
size, tensors = 64 << 20, -(-int(sys.argv[2]) // 64)
header, offset = {}, 0
for i in range(tensors):
    n = min(size, (mib << 20) - offset)
    header[f"w.{i}"] = {"dtype": "F16", "shape": [n // 2], "data_offsets": [offset, offset + n]}
    offset += n
if part:
    header["__metadata__"] = {"part": part}
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

# part_weights <file> <part> <seed> <n>: one F32 tensor `w` of n seeded values, its metadata `part` and `size`: a component's
# config as the Runtime reads a file kept as it is.
part_weights() {
  python3 -I - "$@" <<'PY'
import json, random, struct, sys
path, part, seed, n = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
values = random.Random(seed).random
raw = json.dumps({"__metadata__": {"part": part, "size": str(n)}, "w": {"dtype": "F32", "shape": [n], "data_offsets": [0, 4 * n]}}).encode()
raw += b" " * (-len(raw) % 8)
with open(path, "wb") as out:
    out.write(struct.pack("<Q", len(raw)) + raw + struct.pack(f"<{n}f", *(values() for _ in range(n))))
PY
}

# The web suite's jobs (cl-973) and one of its cells by hand (web-<cell>, web/<cell>.spec.js) take one seed.
key=$suite:$shard
case $shard in
  web-1 | web-2 | web-3 | web-2d | web-2d-solo | web-engines | web-solo | web-native | web-opens-busy) key=cli:web-suite ;;
  web-*) [ ! -f "$here/cli/web/${shard#web-}.spec.js" ] || key=cli:web-suite ;;
esac
case $key in
  cli:entrypoints | cli:executors | cli:projectdb-cancel | cli:local-wake | cli:containment | cli:containment-web | cli:projectvault-static | cli:projectvault-static-distros) needs='' ;;
  # forward's run that moves local's Runtime for its project (cl-1055).
  cli:selfupdate) needs='next scratch' ;;
  # llm_write's Qwen3.5 0.8B (xs-204).
  cli:runs) needs='packages birefnet qwen' ;;
  # project_runtime's probe (cl-1055).
  cli:local-code) needs='packages birefnet scratch' ;;
  # commands shows a LoRA's modules (lora show, xs-183), fits a lane on a slot's components (lane_fit, xs-191) and composes a
  # model of one component with another's (xs-190).
  cli:commands) needs='packages birefnet loras fits parts lorabase uninstall ltxlora' ;;
  cli:follow) needs='packages downloads' ;;
  cli:lines) needs='packages birefnet downloads' ;;
  cli:idle) needs='packages downloads' ;;
  cli:ondemand) needs='packages birefnet downloads calls enhancer' ;;
  # pagebox's 1.6 GB (`spare`) beside the install (cl-1007).
  cli:ondemand-share) needs='packages birefnet downloads spare' ;;
  cli:ondemand-ramp) needs='packages spare' ;;
  cli:ondemand-enhancers) needs='packages enhancer enhancers' ;;
  cli:disk) needs='packages disk' ;;
  cli:hub-grants) needs='packages grants' ;;
  # H3's Models & LoRAs, Qwen's VAE slot and heavy.py's BiRefNet; macOS and Windows run no models.
  cli:web-suite) needs='packages birefnet loras vaes'; [ "${RUNNER_OS:-Linux}" = Linux ] || needs='packages' ;;
  cli:env-cost) needs='packages birefnet' ;;
  # plan_fetch quotes a fine-tune composed of a base's parts (cl-1071).
  cli:plan) needs='packages birefnet downloads composed' ;;
  # A published tool of two models that share a VAE, which a box's machine fetches before any render (cl-1104).
  cli:prefetch) needs='packages pre' ;;
  cli:) needs='packages birefnet qwen downloads spare loras vaes fits parts composed lorabase calls uninstall ltxlora pre' ;;
  # shared_scratch's probe and its warm BiRefNet (cl-984).
  cli:projectvault) needs='packages birefnet scratch' ;;
  cli:*) needs='packages' ;;
  imports:*)
    # Production's published releases are what --published asks about, and CI reads no production (xs-169).
    [ -z "$published" ] || { echo "imports $published asks about production's releases: CI runs only on an isolated Hub; the checkout's packages run without it" >&2; exit 2; }
    needs='' ;;
  cpu:*) needs='' ;;
  *) echo "isolated.sh runs the cli, imports or cpu suite, not '$suite'" >&2; exit 2 ;;
esac

# hub_dependencies [package dir...]: the checkout's packages that those packages' `tensorhub` sources name (every
# package's when none is named), by directory.
hub_dependencies() {
  python3 -I - "$packages" "$@" <<'PY'
import pathlib, re, sys, tomllib
root, chosen = pathlib.Path(sys.argv[1]), set(sys.argv[2:])
def norm(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()
projects = {path.parent.name: tomllib.loads(path.read_text()) for path in sorted(root.glob("*/pyproject.toml"))}
folders = {norm(project["project"]["name"]): folder for folder, project in projects.items()}
named = set()
for folder, project in projects.items():
    for name, source in project.get("tool", {}).get("uv", {}).get("sources", {}).items():
        alternatives = source if isinstance(source, list) else [source]
        if (not chosen or folder in chosen) and any(isinstance(a, dict) and a.get("index") == "tensorhub" for a in alternatives):
            named.add(norm(name))
print(" ".join(sorted(folders.get(name, name) for name in named)))
PY
}

# CPU's author/binder cells use the already-installed candidate and authenticated fixture.
# Its ordinary executor cells remain unchanged and use the package checkout and its locks.
if [ "$suite" = cpu ]; then
  project=$seed/project
  mkdir -p "$project"
  # A package's `tensorhub` sources (orbit's minimax-h3) lock against this Hub's root index, where each is published from
  # the checkout as production's root holds it, read under the Hub's own CA: Calcifer's uv takes the system's bundle once
  # restarted. One refused is the upload and lock cells' to find.
  dependencies=''
  [ -z "$packages" ] || dependencies=$(hub_dependencies ${only//,/ })
  if [ -n "$dependencies" ]; then
    for dependency in $dependencies; do
      publish "$packages/$dependency" cohort || { echo "$dependency could not be seeded from the selected cohort" >&2; exit 1; }
    done
    sudo cp "${COZY_TEST_HUB_CA:?}" /usr/local/share/ca-certificates/cozy-test-hub.crt && sudo update-ca-certificates > "$seed/ca.log" 2>&1
    systemctl --user set-environment SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
    systemctl --user restart cozy-calcifer > "$seed/calcifer-restart.log" 2>&1 || true
  fi
  # The binder installs on the candidates' Runtime and TensorFS, as every RC machine runs them with the RC's cozy (cl-952):
  # with no pair of its own, a machine runs a lock's pair only in its own TensorFS series.
  update=()
  [ -z "$runtime" ] || update+=(--runtime "$runtime")
  [ -z "$tensorfs" ] || update+=(--tensorfs "$tensorfs")
  [ ${#update[@]} -eq 0 ] || cozy --json machine update local "${update[@]}" -y > "$seed/machine-update.json" 2>&1 \
    || { echo "the cpu suite's machine refused the candidates: $(tail -c 600 "$seed/machine-update.json")" >&2; exit 1; }
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
    publish "$staged/$dependency" cohort
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
# The packages checkout, for a cell that runs one from source (account-rent's render time from source, cl-1024).
[ -z "$packages" ] || export E2E_PACKAGES=$packages

if [[ " $needs " == *" next "* ]]; then
  next
  needs=${needs/next/} && needs=${needs# }
fi
# The probe alone (selfupdate's forward, cl-1055), without the first-party packages.
if [ "$needs" = scratch ]; then
  publish "$here/cli/scratch"
  needs=''
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
  # Weightless (App only) and on a stable floor, as on the RC: published for every platform, so a consumer's universal lock
  # resolves it on each (hub_away's Windows-only dependency). A cohort pin would lock it to this runner alone.
  publish "$tools"
  for p in qwen-image-2 minimax-h3 sdxl birefnet; do publish "$packages/$p" cohort; done
  [[ " $needs " != *" scratch "* ]] || publish "$here/cli/scratch"
  # MOSS Image, which takes words only: a mention it can't use is crossed out (cl-944).
  case $key in cli: | cli:web-suite) publish "$packages/moss-image" cohort ;; esac
  # Every other first-party package, as production's catalog holds them: what the palette lists (cl-896). Orbit's lock takes minimax-h3
  # from the root index, under the Hub's own CA, as relay's does. One refused is the cell's to find missing.
  if [[ " $needs " == *" catalog "* ]]; then
    sudo cp "${COZY_TEST_HUB_CA:?}" /usr/local/share/ca-certificates/cozy-test-hub.crt && sudo update-ca-certificates > "$seed/ca.log" 2>&1
    systemctl --user set-environment SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
    systemctl --user restart cozy-calcifer > "$seed/calcifer-restart.log" 2>&1 || true
    for dir in "$packages"/*/; do
      dir=${dir%/} && case ${dir##*/} in qwen-image-2 | minimax-h3 | sdxl | birefnet) continue ;; esac
      grep -q '^\[project.entry-points."cozy.application"\]' "$dir/pyproject.toml" 2> /dev/null || continue
      publish "$dir" cohort || { echo "${dir##*/} could not be seeded from the selected cohort" >&2; exit 1; }
    done
  fi
  case $shard in
    local-code | '')
      # Its lock reads the Hub's index, under the Hub's own CA: Calcifer's uv takes the system's bundle once restarted.
      sudo cp "${COZY_TEST_HUB_CA:?}" /usr/local/share/ca-certificates/cozy-test-hub.crt && sudo update-ca-certificates > "$seed/ca.log" 2>&1
      systemctl --user set-environment SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
      systemctl --user restart cozy-calcifer > "$seed/calcifer-restart.log" 2>&1 || true
      relay
      held
      final
      rc_floor ;;
  esac
  if [[ " $needs " == *" birefnet "* || " $needs " == *" qwen "* || " $needs " == *" downloads "* || " $needs " == *" loras "* || " $needs " == *" parts "* \
    || " $needs " == *" disk "* || " $needs " == *" lorabase "* || " $needs " == *" ltxlora "* || " $needs " == *" enhancer "* ]]; then
    update=()
    [ -z "$runtime" ] || update+=(--runtime "$runtime")
    [ -z "$tensorfs" ] || update+=(--tensorfs "$tensorfs")
    [ ${#update[@]} -eq 0 ] || cozy --json machine update local "${update[@]}" -y > "$seed/machine-update.json" 2>&1 \
      || { echo "the seeding machine refused the candidates: $(tail -c 600 "$seed/machine-update.json")" >&2; exit 1; }
  fi
  if [[ " $needs " == *" birefnet "* ]]; then
    mirror hf://ZhengPeng7/BiRefNet@e2bf8e4460fc8fa32bba5ea4d94b3233d367b0e4 zhengpeng7/birefnet fp16 0.1.0 https://huggingface.co/ZhengPeng7/BiRefNet
  fi
  case $shard in local-code | '') final_model ;; esac
  # The real Qwen3.5 0.8B, which an LLM tool runs on the CPU (xs-204); it also stands in for the downloads' 1.6 GB.
  if [[ " $needs " == *" qwen "* ]]; then
    mirror hf://Qwen/Qwen3.5-0.8B@2fc06364715b967f1860aea9cf38778875588b17 qwen/qwen3.5-0.8b bf16 0.1.0 https://huggingface.co/Qwen/Qwen3.5-0.8B
  fi
  if [[ " $needs " == *" downloads "* ]]; then
    if [[ " $needs " != *" qwen "* ]]; then
      random_weights "$seed/qwen3.5-0.8b.safetensors" 1640
      mirror "$seed/qwen3.5-0.8b.safetensors" qwen/qwen3.5-0.8b bf16 0.1.0 https://huggingface.co/Qwen/Qwen3.5-0.8B --source-format as-is
    fi
    random_weights "$seed/fontbox.safetensors" 115
    mirror "$seed/fontbox.safetensors" lilylilith/fontbox bf16 0.1.0 https://huggingface.co/lilylilith/fontbox --source-format as-is
    rm -f "$seed"/*.safetensors
  fi
  if [[ " $needs " == *" disk "* ]]; then
    # What disk_full (cli/disk.sh, tfs-150) cycles through a 2.5 GiB disk: six models of 384 MiB of random weights, none sharing a chunk.
    for i in 1 2 3 4 5 6; do
      random_weights "$seed/disk.safetensors" 384 "disk-$i"
      mirror "$seed/disk.safetensors" "e2e/disk-$i" bf16 0.1.0 https://github.com/cozy-creator/cozy-creator/tree/master/e2e --source-format as-is
      rm -f "$seed/disk.safetensors"
    done
    # And one bigger than the disk can make free, refused with the sizes.
    random_weights "$seed/disk.safetensors" 2048 disk-big
    mirror "$seed/disk.safetensors" e2e/disk-big bf16 0.1.0 https://github.com/cozy-creator/cozy-creator/tree/master/e2e --source-format as-is
    rm -f "$seed/disk.safetensors"
    # And disk_run_object's (cl-1076): its asset is a file the package that runs it carries too (cli/tokenizer), as H3 holds
    # the Qwen3-VL processor files minimax-h3 carries.
    random_weights "$seed/disk.safetensors" 384 disk-tokenizer
    mirror "$seed/disk.safetensors" e2e/disk-tokenizer bf16 0.1.0 https://github.com/cozy-creator/cozy-creator/tree/master/e2e --source-format as-is \
      --add "processor/vocab.json=$here/cli/tokenizer/e2e_tokenizer/processor/vocab.json"
    rm -f "$seed/disk.safetensors"
  fi
  # hub_grants' model (xs-210): a 1 MiB weight and 320 assets of 4 KiB, so its download is hundreds of objects and about 2 MB.
  if [[ " $needs " == *" grants "* ]]; then
    random_weights "$seed/grants.safetensors" 1
    mkdir -p "$seed/grants" && adds=()
    for i in $(seq 320); do
      head -c 4096 /dev/urandom > "$seed/grants/a$i.bin" && adds+=(--add "assets/a$i.bin=$seed/grants/a$i.bin")
    done
    mirror "$seed/grants.safetensors" e2e/grants bf16 0.1.0 https://github.com/cozy-creator/cozy-creator/tree/master/e2e \
      --source-format as-is "${adds[@]}"
    rm -rf "$seed/grants" "$seed/grants.safetensors"
    export E2E_GRANTS=e2e/grants
  fi
  if [[ " $needs " == *" calls "* ]]; then
    # ondemand_calls' orchestrator (cl-1077): the model of the function it calls and of one it never calls, 32 MiB each; and
    # ondemand_adapted's other model, bigger than its base, so its root's prefetch (smallest first) can't land it before its call;
    # and ondemand_order's two, the earlier run's bigger, so the later run's call is wanted while it still downloads; and
    # ondemand_needs' two (cl-1102): the one its call loads, and the one it has on demand and never reads.
    page=https://github.com/cozy-creator/cozy-creator/tree/master/e2e
    for name in used:32 unused:32 other:256 early:256 late:128 needed:192 extra:256; do
      random_weights "$seed/calls.safetensors" "${name#*:}" "calls-${name%:*}"
      name=${name%:*}
      mirror "$seed/calls.safetensors" "e2e/calls-$name" bf16 0.1.0 "$page" --source-format as-is
      rm -f "$seed/calls.safetensors"
    done
    # ondemand_rules' four (cl-1103): the two `pick` needs by rule, and the two of the functions `route` calls by rule.
    for name in worded plain left right; do
      random_weights "$seed/calls.safetensors" 32 "rules-$name"
      mirror "$seed/calls.safetensors" "e2e/rules-$name" bf16 0.1.0 "$page" --source-format as-is
      rm -f "$seed/calls.safetensors"
    done
    # ondemand_progress' (tfs-300): read on demand as its call starts, 512 MiB, about 17 s over the 30 MB/s link.
    random_weights "$seed/calls.safetensors" 512 ondemand-read
    mirror "$seed/calls.safetensors" e2e/ondemand-read bf16 0.1.0 "$page" --source-format as-is
    rm -f "$seed/calls.safetensors"
    # And ondemand_adapted's, as lora_reads' (tfs-300): e2e/calls-base, a 16 MiB text encoder (e2e/calls-te's) with a 256 MiB
    # transformer put on it, one linear `proj` [8192, 8192]; and e2e/calls-lora, a rank-4 LoRA of `proj` in PEFT's names.
    part_weights "$seed/calls-te.safetensors" text_encoder 8 4194304
    python3 -I - "$seed" <<'PYCALLS'
import json, os, random, struct, sys
def write(path, tensors):
    header, at = {}, 0
    for key, (dtype, shape, _) in tensors.items():
        n = shape[0] * shape[1] * (4 if dtype == "F32" else 2)
        header[key] = {"dtype": dtype, "shape": shape, "data_offsets": [at, at + n]}
        at += n
    text = json.dumps(header).encode()
    text += b" " * (-len(text) % 8)
    with open(path, "wb") as out:
        out.write(struct.pack("<Q", len(text)) + text)
        for key, (dtype, shape, fill) in tensors.items():
            fill(out, shape[0] * shape[1])
# F32 of random mantissas, each high byte's exponent held near 2^-7: finite and small.
SMALL = bytes((b & 0x80) | 0x3C for b in range(256))
def big(out, n):
    for _ in range(0, n, 1 << 20):
        chunk = bytearray(os.urandom(4 * min(1 << 20, n)))
        chunk[3::4] = bytes(chunk[3::4]).translate(SMALL)
        out.write(chunk)
        n -= 1 << 20
def lora(seed):
    values = random.Random(seed)
    return lambda out, n: out.write(struct.pack(f"<{n}e", *(values.uniform(-0.01, 0.01) for _ in range(n))))
write(f"{sys.argv[1]}/calls-proj.safetensors", {"proj.weight": ("F32", [8192, 8192], big)})
write(f"{sys.argv[1]}/calls-lora.safetensors", {"proj.lora_A.weight": ("F16", [4, 8192], lora(6)), "proj.lora_B.weight": ("F16", [8192, 4], lora(7))})
PYCALLS
    # One refused is the cell's to report (E2E_CALLS_SEED), never the shard's.
    if ! mirror "$seed/calls-te.safetensors:text_encoder" e2e/calls-te f32 0.1.0 "$page" --source-format as-is 2> "$seed/calls-base.err" \
      || ! mirror "$seed/calls-proj.safetensors" e2e/calls-base f32 0.1.0 "$page" --source-format as-is --base e2e/calls-te@0.1.0:transformer 2>> "$seed/calls-base.err"; then
      E2E_CALLS_SEED="e2e/calls-base: $(tail -c 400 "$seed/calls-base.err" | tr '\n' ' ')"
    elif ! standin_lora "$seed/calls-lora.safetensors:transformer" e2e/calls-lora "$page" '' 2> "$seed/calls-lora.err"; then
      E2E_CALLS_SEED="e2e/calls-lora: $(tail -c 400 "$seed/calls-lora.err" | tr '\n' ' ')"
    fi
    export E2E_CALLS_SEED=${E2E_CALLS_SEED:-}
    log "published e2e/calls-used, -unused, -other, -early, -late, -needed, -extra, -base and e2e/calls-lora${E2E_CALLS_SEED:+ (not all: $E2E_CALLS_SEED)}"
    rm -f "$seed"/calls-*.safetensors
  fi
  if [[ " $needs " == *" enhancer "* ]]; then
    # project/enhance's (cl-1101): each request's own model (16 MiB) and two prompt enhancers (96 MiB each), none sharing a chunk.
    # `plain`'s are ondemand_behind's; ondemand_enhancers runs all three.
    requests=plain
    [[ " $needs " != *" enhancers "* ]] || requests='words edit plain'
    for request in $requests; do
      for slot in model:16 expander:96 rewriter:96; do
        random_weights "$seed/enhance.safetensors" "${slot#*:}" "enhance-$request-${slot%:*}"
        mirror "$seed/enhance.safetensors" "e2e/enhance-$request-${slot%:*}" bf16 0.1.0 https://github.com/cozy-creator/cozy-creator/tree/master/e2e --source-format as-is
        rm -f "$seed/enhance.safetensors"
      done
    done
  fi
  if [[ " $needs " == *" uninstall "* ]]; then
    # uninstall_on's model (cl-1076): 32 MiB no other cell reads, so removing it takes nothing from them.
    random_weights "$seed/uninstall.safetensors" 32 uninstall
    mirror "$seed/uninstall.safetensors" e2e/uninstall bf16 0.1.0 https://github.com/cozy-creator/cozy-creator/tree/master/e2e --source-format as-is
    rm -f "$seed/uninstall.safetensors"
  fi
  if [[ " $needs " == *" spare "* ]]; then
    # ondemand_share's weights (cl-1007): more than the 60 Mbit link moves while its install runs.
    random_weights "$seed/pagebox.safetensors" 1600
    mirror "$seed/pagebox.safetensors" lilylilith/pagebox bf16 0.1.0 https://huggingface.co/lilylilith/pagebox --source-format as-is
    rm -f "$seed"/*.safetensors
  fi
  if [[ " $needs " == *" loras "* ]]; then
    # What H3's Models & LoRAs lists (cl-934): its default model, a stand-in its family names, and LoRAs of that family.
    h3=$(sed -n 's/^_MODEL = "\(.*\)"$/\1/p' "$packages/minimax-h3/h3.py")
    [ -n "$h3" ] || { echo "minimax-h3/h3.py names no _MODEL" >&2; exit 1; }
    random_weights "$seed/minimax-h3.safetensors" 1
    mirror "$seed/minimax-h3.safetensors" "${h3%@*}" bf16 "${h3#*@}" https://huggingface.co/MiniMaxAI/MiniMax-H3 --source-format as-is
    cozy --json model edit "${h3%@*}" --family minimax-h3 > "$seed/edit-minimax-h3.json" 2>&1 \
      || { echo "naming ${h3%@*}'s family failed: $(tail -c 600 "$seed/edit-minimax-h3.json")" >&2; exit 1; }
    # Two fine-tunes of its family for its one Base Model to choose between (cl-960).
    for tune in sexgod1979/pinkcherry tenstrip/eros-max; do
      mirror "$seed/minimax-h3.safetensors" "$tune" bf16 0.1.0 "https://huggingface.co/$tune" --source-format as-is
      cozy --json model edit "$tune" --family minimax-h3 > "$seed/edit-${tune##*/}.json" 2>&1 \
        || { echo "naming $tune's family failed: $(tail -c 600 "$seed/edit-${tune##*/}.json")" >&2; exit 1; }
    done
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
    # Three of its family: an edit LoRA, one whose card note is long (loraNotes.ts), which the picker wraps, and 360-orbit, whose
    # trigger is a whole prompt, commas and all: each --trigger is one entry, kept whole (cl-971).
    orbit='One frozen instant. Only the camera moves. In a continuous 360 orbit. Preserve every person and object in exactly the same world position, orientation, shape and pose throughout the shot. Airborne objects remain suspended at the captured height and angle: no wobbling, shaking, spinning, drifting, falling or continued action. Keep faces, hands, clothing, liquids and the background motionless while retaining their natural appearance. Camera parallax is the only source of apparent movement. No cuts, zoom, morphing or added objects.'
    for lora in alissonerdx/minimax-h3-vfx-edit nrdx/bfs-body-swap-selective-lora pablodawson/360-orbit; do
      case $lora in
        nrdx/*) page=https://huggingface.co/Alissonerdx/Minimax-H3-ComfyUI triggers=(--trigger bfs_swap --trigger 'head swap') ;;
        pablodawson/*) page=https://huggingface.co/pablodawson/MiniMax-H3-360-Orbit-LoRA triggers=(--trigger "$orbit") ;;
        *) page=https://huggingface.co/Alissonerdx/Minimax-H3-ComfyUI triggers=(--trigger vfx_edit) ;;
      esac
      standin_lora "$seed/vfx-edit.safetensors" "$lora" "$page" minimax-h3 "${triggers[@]}" || exit 1
    done
    log "published ${h3%@*}@${h3#*@} (a stand-in) and three LoRAs of family minimax-h3"
    rm -f "$seed"/*.safetensors
  fi
  if [[ " $needs " == *" vaes "* ]]; then
    # What Qwen Image's VAE slot lists (se-131): its default and a VAE of its family, tagged `vae` as the Hub's is,
    # a model of its `vae` alone (xs-190).
    random_weights "$seed/qwen-image-2.1.safetensors" 1
    mirror "$seed/qwen-image-2.1.safetensors" qwen/qwen-image-2.1 bf16 0.1.0 https://huggingface.co/Qwen/Qwen-Image-2.1 --source-format as-is
    mirror "$seed/qwen-image-2.1.safetensors:vae" madebyollin/texture-fix-vae bf16 0.3.0 \
      https://huggingface.co/madebyollin/texture-fix-vae-for-qwen-image-2.1 --source-format as-is
    for model in qwen/qwen-image-2.1 madebyollin/texture-fix-vae; do
      tags=(); [ "$model" = qwen/qwen-image-2.1 ] || tags=(--tag vae)
      cozy --json model edit "$model" --family qwen-image-2 "${tags[@]}" > "$seed/edit-${model##*/}.json" 2>&1 \
        || { echo "naming $model's family failed: $(tail -c 600 "$seed/edit-${model##*/}.json")" >&2; exit 1; }
    done
    log "published qwen/qwen-image-2.1@0.1.0 and madebyollin/texture-fix-vae@0.3.0 (stand-ins) of family qwen-image-2"
    rm -f "$seed"/*.safetensors
  fi
  if [[ " $needs " == *" parts "* ]]; then
    # What component_models (cli/commands.sh, xs-190) composes: e2e/parts, a transformer and a VAE (a transformer alone, the VAE
    # put on it), and e2e/parts-vae, a model of a `vae` alone.
    part_weights "$seed/dit.safetensors" dit 1 1048576
    part_weights "$seed/vae-a.safetensors" vae-a 2 65536
    part_weights "$seed/vae-b.safetensors" vae-b 3 65536
    page=https://github.com/cozy-creator/cozy-creator/tree/master/e2e
    mirror "$seed/dit.safetensors:transformer" e2e/parts-dit f32 0.1.0 "$page" --source-format as-is
    mirror "$seed/vae-a.safetensors" e2e/parts f32 0.1.0 "$page" --source-format as-is --base e2e/parts-dit@0.1.0:vae
    mirror "$seed/vae-b.safetensors:vae" e2e/parts-vae f32 0.1.0 "$page" --source-format as-is
    # e2e/parts@0.2.0: 0.1.0 cut by reference to its vae, a part an unversioned pick takes only for a slot that reads it (xs-192).
    mirror "e2e/parts@0.1.0:vae" e2e/parts f32 0.2.0 "$page"
    log "published e2e/parts@0.1.0 (transformer, vae), its part 0.2.0 (vae) and e2e/parts-vae@0.1.0 (vae)"
    rm -f "$seed"/*.safetensors
  fi
  if [[ " $needs " == *" composed "* ]]; then
    # What plan_fetch (cli/plan.sh, cl-1071) quotes and fetches: e2e/composed-base, a 48 MiB transformer and a 16 MiB VAE;
    # e2e/composed-tune, its own 48 MiB transformer on the base's VAE (the base's parts shared, as noctaluna/noct-q is composed of
    # Qwen-Image-2.1's); and e2e/composed-extra, 32 MiB a slot fetches only on demand. One family, so the tune takes the base's slot.
    page=https://github.com/cozy-creator/cozy-creator/tree/master/e2e
    random_weights "$seed/composed-dit.safetensors" 48 composed-dit
    random_weights "$seed/composed-vae.safetensors" 16 composed-vae
    random_weights "$seed/composed-tune.safetensors" 48 composed-tune
    random_weights "$seed/composed-extra.safetensors" 32 composed-extra
    mirror "$seed/composed-dit.safetensors:transformer" e2e/composed-dit bf16 0.1.0 "$page" --source-format as-is
    mirror "$seed/composed-vae.safetensors" e2e/composed-base bf16 0.1.0 "$page" --source-format as-is --base e2e/composed-dit@0.1.0:vae
    mirror "$seed/composed-tune.safetensors" e2e/composed-tune bf16 0.1.0 "$page" --source-format as-is --base e2e/composed-base@0.1.0:transformer
    mirror "$seed/composed-extra.safetensors" e2e/composed-extra bf16 0.1.0 "$page" --source-format as-is
    for model in e2e/composed-base e2e/composed-tune; do
      cozy --json model edit "$model" --family e2e-composed > "$seed/edit-${model#e2e/}.json" 2>&1 \
        || { echo "naming $model's family failed: $(tail -c 600 "$seed/edit-${model#e2e/}.json")" >&2; exit 1; }
    done
    log "published e2e/composed-base@0.1.0 (transformer, vae), e2e/composed-tune@0.1.0 (its transformer, the base's vae), e2e/composed-extra@0.1.0"
    rm -f "$seed"/composed-*.safetensors
  fi
  if [[ " $needs " == *" pre "* ]]; then
    # What the prefetch cells' boxes read (cli/prefetch.sh, cl-1104): e2e/pre-a, a 384 MiB transformer and a 320 MiB VAE; e2e/pre-b,
    # its own 384 MiB transformer on e2e/pre-a's VAE, as two fine-tunes of one base share its parts (one family, so either takes
    # the tool's slot); e2e/pre-ask, 256 MiB a request asks for while a prefetch goes; e2e/pre-req, 256 MiB a request makes room for
    # on a short disk; and their tool, published, so its machine installs it with no run. Each a few seconds through the cells' 30 MB/s
    # link.
    page=https://github.com/cozy-creator/cozy-creator/tree/master/e2e
    random_weights "$seed/pre-dit.safetensors" 384 pre-dit
    random_weights "$seed/pre-vae.safetensors" 320 pre-vae
    random_weights "$seed/pre-b.safetensors" 384 pre-b
    random_weights "$seed/pre-ask.safetensors" 256 pre-ask
    random_weights "$seed/pre-req.safetensors" 256 pre-req
    mirror "$seed/pre-dit.safetensors:transformer" e2e/pre-dit bf16 0.1.0 "$page" --source-format as-is
    mirror "$seed/pre-vae.safetensors" e2e/pre-a bf16 0.1.0 "$page" --source-format as-is --base e2e/pre-dit@0.1.0:vae
    mirror "$seed/pre-b.safetensors" e2e/pre-b bf16 0.1.0 "$page" --source-format as-is --base e2e/pre-a@0.1.0:transformer
    mirror "$seed/pre-ask.safetensors" e2e/pre-ask bf16 0.1.0 "$page" --source-format as-is
    mirror "$seed/pre-req.safetensors" e2e/pre-req bf16 0.1.0 "$page" --source-format as-is
    for model in e2e/pre-a e2e/pre-b; do
      cozy --json model edit "$model" --family e2e-pre > "$seed/edit-${model#e2e/}.json" 2>&1 \
        || { echo "naming $model's family failed: $(tail -c 600 "$seed/edit-${model#e2e/}.json")" >&2; exit 1; }
    done
    publish "$here/cli/project/prefetched"
    # prefetch_disk's slow install: its lock pins the wheel its server makes the same each time, served fast while it publishes.
    python3 "$here/cli/project/slowinstall/wheel.py" "$seed/slow-wheel" > "$seed/slow-wheel.log" 2>&1 &
    slow_wheel=$!
    for _ in $(seq 100); do curl -fsI http://127.0.0.1:47104/e2e_slow_wheel-0.0.0-py3-none-any.whl > /dev/null 2>&1 && break; sleep 0.2; done
    publish "$here/cli/project/slowinstall" || log "e2e-slow-install wasn't published: prefetch_disk's install line is not judged"
    kill "$slow_wheel" 2> /dev/null || true
    rm -rf "$seed/slow-wheel"
    log "published e2e/pre-a@0.1.0 (transformer, vae), e2e/pre-b@0.1.0 (its transformer, e2e/pre-a's vae), e2e/pre-ask@0.1.0, e2e/pre-req@0.1.0, e2e-prefetched and e2e-slow-install"
    rm -f "$seed"/pre-*.safetensors
  fi
  if [[ " $needs " == *" fits "* ]]; then
    # lane_fit (xs-191): fitting/vae is fitting/whole (an 8 MiB `model`) with a 1 MiB `vae` added, as
    # madebyollin/texture-fix-vae is Qwen-Image-2.1 with its VAE replaced.
    random_weights "$seed/fit-whole.safetensors" 8
    random_weights "$seed/fit-vae.safetensors" 1
    mirror "$seed/fit-whole.safetensors" fitting/whole bf16 0.1.0 https://huggingface.co/fitting/whole --source-format as-is
    mirror "$seed/fit-vae.safetensors" fitting/vae bf16 0.1.0 https://huggingface.co/fitting/vae --source-format as-is \
      --base fitting/whole@0.1.0:vae
    export E2E_FIT=fitting/vae
    rm -f "$seed"/*.safetensors
  fi
  if [[ " $needs " == *" lorabase "* ]]; then
    # lora_reads (cli/commands.sh, tfs-300): e2e/lora-base, a 16 MiB text encoder (e2e/lora-te's) with a 256 KiB transformer put on
    # it, one linear `proj`; and e2e/lora-proj, a rank-4 LoRA of `proj` in PEFT's names.
    page=https://github.com/cozy-creator/cozy-creator/tree/master/e2e
    part_weights "$seed/te.safetensors" text_encoder 4 4194304
    python3 -I - "$seed" <<'PY'
import json, random, struct, sys
def write(path, tensors):
    header, data = {}, b""
    for key, (dtype, shape, seed) in tensors.items():
        values, n = random.Random(seed), shape[0] * shape[1]
        raw = struct.pack(f"<{n}{'f' if dtype == 'F32' else 'e'}", *(values.uniform(-0.1, 0.1) for _ in range(n)))
        header[key] = {"dtype": dtype, "shape": shape, "data_offsets": [len(data), len(data) + len(raw)]}
        data += raw
    text = json.dumps(header).encode()
    text += b" " * (-len(text) % 8)
    open(path, "wb").write(struct.pack("<Q", len(text)) + text + data)
write(f"{sys.argv[1]}/proj.safetensors", {"proj.weight": ("F32", [256, 256], 5)})
write(f"{sys.argv[1]}/proj-lora.safetensors", {"proj.lora_A.weight": ("F16", [4, 256], 6), "proj.lora_B.weight": ("F16", [256, 4], 7)})
PY
    mirror "$seed/te.safetensors:text_encoder" e2e/lora-te f32 0.1.0 "$page" --source-format as-is
    mirror "$seed/proj.safetensors" e2e/lora-base f32 0.1.0 "$page" --source-format as-is --base e2e/lora-te@0.1.0:transformer
    standin_lora "$seed/proj-lora.safetensors:transformer" e2e/lora-proj "$page" '' || exit 1
    log "published e2e/lora-base@0.1.0 (text_encoder, transformer) and e2e/lora-proj@0.1.0 (a LoRA of its proj)"
    rm -f "$seed"/*.safetensors
  fi
  if [[ " $needs " == *" ltxlora "* ]]; then
    # ltx_lora (cli/commands.sh): Lightricks' LTX-2.3 Motion Track Control IC-LoRA (327 MB, ungated) as Lightricks publishes it,
    # LTX's native names under diffusion_model., uploaded from Hugging Face by the candidate machine. commands.sh says what the
    # upload made of it, so a failed one doesn't end the shard.
    LANE=bf16 standin_lora "hf://Lightricks/LTX-2.3-22b-IC-LoRA-Motion-Track-Control@022bf183b6da565fd2e2800e135ba0b46db57d32/ltx-2.3-22b-ic-lora-motion-track-control-ref0.5.safetensors" \
      lightricks/ltx-2.3-motion-track-control https://huggingface.co/Lightricks/LTX-2.3-22b-IC-LoRA-Motion-Track-Control '' \
      && log "published lightricks/ltx-2.3-motion-track-control@0.1.0 (an LTX-2.3 IC-LoRA)" \
      || log "lightricks/ltx-2.3-motion-track-control: $(tail -c 400 "$seed/upload-ltx-2.3-motion-track-control.json" 2> /dev/null)"
    # ltx_msr: LiconStudio's Multiple-Subject Reference V2 for LTX-2.5 (2.2 GB, Apache-2.0, ungated), the same way.
    LANE=bf16 standin_lora "hf://LiconStudio/LTX-2.5-Multiple-Subject-Reference@aa19b48f845995525d8ace60cdc89b842e951ce6/LTX-2.5-Licon-MSR-V2.safetensors" \
      liconstudio/licon-msr-v2 https://huggingface.co/LiconStudio/LTX-2.5-Multiple-Subject-Reference '' \
      && log "published liconstudio/licon-msr-v2@0.1.0 (an LTX-2.5 reference LoRA with slot embeddings)" \
      || log "liconstudio/licon-msr-v2: $(tail -c 400 "$seed/upload-licon-msr-v2.json" 2> /dev/null)"
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
