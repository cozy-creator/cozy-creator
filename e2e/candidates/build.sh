#!/usr/bin/env bash
# e2e/candidates/build.sh <cozy|cozy-before|tensorfs|runtime> <src> <out>: one candidate for this runner's OS, into <out>; prints
# its file. cozy-before (e2e/cli/selfupdate.sh, cl-978): the source built as a release of the line before the Latest, 0.24.99 with TensorFS
# 0.24.99, which runs no Runtime from 0.46: the cozy that updates itself to the Latest for a package that needs it. cozy (TensorD, the dev cozy: the RC's <x.y.z>-dev+g<sha8>, a release's <version>+dev.g<sha8>) and the TensorFS wheel come from the cozy-creator
# checkout <src>, the Runtime wheel (<version>+dev.g<sha8>) from the cozy-runtime checkout <src>.
#   REUSE=1      cozy or tensorfs as a cozy-creator merge-group or dispatched e2e run uploaded it for the same build
#                inputs: a PR's dispatch builds what its merge-queue entry reuses when the PR is up to date
# With $GITHUB_OUTPUT, `name` is the artifact's: candidate-<what>-<inputs> on Linux x64, <what>-<os>-<arch>-<inputs>
# elsewhere, <inputs> the build inputs' tree hash (the Runtime's: its commit). A release candidate is no build: e2e takes its exact version through `cozy machine update`
# (candidates/action.yml), which fetches and verifies it.
set -euo pipefail
what=$1 src=$2 out=$3
exe=''
[ "${RUNNER_OS:-}" != Windows ] || exe=.exe
mkdir -p "$out"

# reuse: the candidate an earlier cozy-creator e2e run built from the same build inputs. Only a merge group's or a
# dispatch's run counts: a pull request's runs the PR's own workflow.
reuse() {
  local run
  for run in $(gh api "repos/cozy-creator/cozy-creator/actions/artifacts?name=$name" \
    --jq '.artifacts[] | select(.expired | not) | .workflow_run.id'); do
    gh api "repos/cozy-creator/cozy-creator/actions/runs/$run" --jq '.event + " " + .path' \
      | grep -qxE '(merge_group|workflow_dispatch) \.github/workflows/e2e\.yaml' || continue
    gh run download "$run" -R cozy-creator/cozy-creator -n "$name" -D "$out" > /dev/null || continue
    echo "$what: reused $name of https://github.com/cozy-creator/cozy-creator/actions/runs/$run" >&2
    return 0
  done
  return 1
}

case $what in
  cozy | cozy-before | tensorfs) repo=cozy-creator ;;
  runtime) repo=cozy-runtime ;;
  *) echo "build.sh: no candidate $what" >&2; exit 2 ;;
esac
sha=$(git -C "$src" rev-parse HEAD)
inputs=$sha
stage_src=''
[ "$what" != cozy ] || stage_src=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
# cozy embeds the page, web/app, which imports web/player. A ref from before web has none.
web=''
[ "$what" != cozy ] || ! git -C "$src" rev-parse -q --verify HEAD:web > /dev/null || web=HEAD:web
provider=''
[ "$what" != cozy ] || ! git -C "$src" cat-file -e HEAD:scripts/prepare-securefs.py 2> /dev/null || provider=HEAD:scripts/prepare-securefs.py
if [ "$repo" = cozy-creator ]; then
  inputs=$({
    git -C "$src" rev-parse HEAD:crates HEAD:vendor HEAD:Cargo.toml HEAD:Cargo.lock \
      HEAD:rust-toolchain.toml HEAD:e2e/candidates $web $provider
    # The harness may stage code for an older product that has no helper of its own.
    if [ "$what" = cozy ]; then
      git -C "$stage_src" rev-parse HEAD:crates HEAD:vendor HEAD:Cargo.toml HEAD:Cargo.lock HEAD:rust-toolchain.toml
      sha256sum < "${BASH_SOURCE[0]}" | cut -d ' ' -f 1
    fi
  } | sha256sum | cut -c1-40)
fi
# Another OS's or CPU's build (macOS, Windows: e2e.yaml's cli-os legs build their own) is named for it, and outside
# candidate-*, the Linux set every Linux job takes whole: a macOS `cozy` merged into it replaces the Linux one.
plat=$(printf '%s-%s' "${RUNNER_OS:-Linux}" "${RUNNER_ARCH:-X64}" | tr '[:upper:]' '[:lower:]')
name=candidate-$what-$inputs
[ "$plat" = linux-x64 ] || name=$what-$plat-$inputs
if [ -n "${REUSE:-}" ]; then
  # REUSE=1 only reuses: nothing is written, and no name, when no run has these sources' candidate.
  reuse || { echo "$what: no e2e run of cozy-creator holds a build of ${sha:0:12}'s build inputs ($inputs)" >&2; exit 0; }
elif [ "$what" = cozy ]; then
  # The dev build `cozy machine update --tensord` takes, dev-build.yaml's stamp.
  (cd "$src"
    # To stderr: this script's stdout is its file, and callers drop it. A page that doesn't build must say why.
    [ ! -f web/build.sh ] || bash web/build.sh >&2
    if [ -f scripts/prepare-securefs.py ]; then
      target=$(rustc -vV | sed -n 's/^host: //p')
      archive=$(uv run -q --no-project python scripts/prepare-securefs.py --target "$target" --out-dir "$PWD/target/vault-helper")
      case "$target" in
        *apple-darwin) ;; # Native hdiutil/APFS; no third-party helper is embedded.
        *) export COZY_SECUREFS_ARCHIVE="$archive" COZY_SECUREFS_REQUIRED=1 ;;
      esac
    fi
    cargo fetch --locked
    # A release's version gets +dev.g<sha8>; the RC's dev version (0.26.0-dev) gets +g<sha8> from crates/api/build.rs (cl-976).
    uv run -q --no-project python -c 'import re,sys; p="Cargo.toml"; s=open(p).read(); open(p,"w").write(re.sub(r"(?m)^version = \"([^\"]+)\"$", lambda m: m[0] if "-" in m[1] else f"version = \"{m[1]}+dev.g{sys.argv[1][:8]}\"", s, count=1))' "$sha"
    cargo update -w --offline
    COZY_COMMIT=$sha cargo build --release --locked --offline -p cozy --bin cozy
    git checkout -- Cargo.toml Cargo.lock)
  (cd "$stage_src"
    COZY_COMMIT=$(git rev-parse HEAD) cargo build --release --locked -p api --example stage-code)
  # cozy is one file (00e565d72): varena is the TensorFS wheel's.
  cp "$src/target/release/cozy$exe" "$out/cozy$exe"
  cp "$stage_src/target/release/examples/stage-code$exe" "$out/stage-code$exe"
elif [ "$what" = cozy-before ]; then
  # A release's version, no +dev: it updates itself. Its TensorFS crate is the line's, whose protocol is what it runs. No page.
  (cd "$src"
    sed -i '0,/^version = "[^"]*"$/s//version = "0.24.99"/' Cargo.toml crates/tensorfs/Cargo.toml
    cargo update -w
    COZY_COMMIT=$sha cargo build --release -p cozy --bin cozy
    git checkout -- Cargo.toml Cargo.lock crates/tensorfs/Cargo.toml)
  cp "$src/target/release/cozy$exe" "$out/cozy$exe"
elif [ "$what" = tensorfs ]; then
  # Staged as the release wheel is (backend/stage_cli.py, as dev-build): `tfs` with the extension's features, so TensorFS
  # and every dependency compile once for both, and on Linux the Torch allocator shims, without which the candidate runs
  # no GPU work ("this TensorFS wheel has no Torch allocator shim": cli-release, e2e 42749286657).
  (cd "$src/crates/tensorfs-py"
    uv run -q --no-project --python 3.11 python backend/stage_cli.py --release > /dev/null
    uvx -q maturin@1.15.0 build --release --locked --out "$out")
else
  # Its own version, <version>+dev.g<sha8> as the dev cozy's, so the imports suite tells it from PyPI's.
  (cd "$src"
    # Artifact construction needs only its build backend. Do not resolve a serving
    # find-links/source override when the matched TensorFS candidate is built beside it.
    uv --no-config run -q --no-project python scripts/stamp-build-provenance.py "$sha"
    uv --no-config version -q --frozen "$(uv --no-config version --short)+dev.g${sha:0:8}"
    uv --no-config build -q --wheel --out-dir "$out"
    git checkout -- pyproject.toml)
fi
case $what in
  cozy | cozy-before) file=$out/cozy$exe; chmod +x "$file"; version=$("$file" --version) ;;
  tensorfs) file=$(ls "$out"/tensorfs-*.whl) ;;
  runtime) file=$(ls "$out"/cozy_runtime-*.whl) ;;
esac
echo "$what: ${version:-} $(basename "$file") from $repo ${sha:0:12}" >&2
[ -z "${GITHUB_OUTPUT:-}" ] || echo "name=$name" >> "$GITHUB_OUTPUT"
echo "$file"
