#!/usr/bin/env bash
# e2e/candidates/build.sh <cozy|tensorfs|runtime> <src> <out>: one candidate for this runner's OS, into <out>; prints
# its file. cozy (TensorD, the dev cozy <version>+dev.g<sha8>) and the TensorFS wheel come from the cozy-creator
# checkout <src>, the Runtime wheel (<version>+dev.g<sha8>) from the cozy-runtime checkout <src>.
#   REUSE=1      cozy or tensorfs as a cozy-creator merge-group or dispatched e2e run uploaded it for the same build
#                inputs: a PR's dispatch builds what its merge-queue entry reuses when the PR is up to date
# With $GITHUB_OUTPUT, `name` is the artifact's: candidate-<what>-<inputs>, <inputs> the build inputs' tree hash (the
# Runtime's: its commit). A release candidate is no build: e2e takes its exact version through `cozy machine update`
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
  cozy | tensorfs) repo=cozy-creator ;;
  runtime) repo=cozy-runtime ;;
  *) echo "build.sh: no candidate $what" >&2; exit 2 ;;
esac
sha=$(git -C "$src" rev-parse HEAD)
inputs=$sha
[ "$repo" != cozy-creator ] || inputs=$(git -C "$src" rev-parse HEAD:crates HEAD:vendor HEAD:Cargo.toml HEAD:Cargo.lock \
  HEAD:rust-toolchain.toml HEAD:e2e/candidates | sha256sum | cut -c1-40)
name=candidate-$what-$inputs
if [ -n "${REUSE:-}" ]; then
  # REUSE=1 only reuses: nothing is written, and no name, when no run has these sources' candidate.
  reuse || { echo "$what: no e2e run of cozy-creator holds a build of ${sha:0:12}'s build inputs ($inputs)" >&2; exit 0; }
elif [ "$what" = cozy ]; then
  # The dev build `cozy machine update --tensord` takes, dev-build.yaml's stamp.
  (cd "$src"
    cargo fetch --locked
    uv run -q --no-project python -c 'import re,sys; p="Cargo.toml"; s=open(p).read(); open(p,"w").write(re.sub(r"(?m)^version = \"([^\"]+)\"$", lambda m: f"version = \"{m[1]}+dev.g{sys.argv[1][:8]}\"", s, count=1))' "$sha"
    cargo update -w --offline
    COZY_COMMIT=$sha cargo build --release --locked --offline -p cozy --bin cozy
    git checkout -- Cargo.toml Cargo.lock)
  cp "$src/target/release/cozy$exe" "$out/cozy$exe"
elif [ "$what" = tensorfs ]; then
  # tfs resolved with the extension's features (tensorfs-py's default store), as maturin resolves them: TensorFS and
  # every dependency compile once for both.
  (cd "$src"
    cargo build --release --locked -p tensorfs-py -p tensorfs --bin tfs --no-default-features --features tensorfs/store,tensorfs-py/store
    mkdir -p crates/tensorfs-py/wheel-data/scripts
    cp "target/release/tfs$exe" crates/tensorfs-py/wheel-data/scripts/
    cd crates/tensorfs-py && uvx -q maturin@1.15.0 build --release --locked --out "$out")
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
  cozy) file=$out/cozy$exe; chmod +x "$file" ;;
  tensorfs) file=$(ls "$out"/tensorfs-*.whl) ;;
  runtime) file=$(ls "$out"/cozy_runtime-*.whl) ;;
esac
echo "$what: $([ "$what" != cozy ] || "$file" --version) $(basename "$file") from $repo ${sha:0:12}" >&2
[ -z "${GITHUB_OUTPUT:-}" ] || echo "name=$name" >> "$GITHUB_OUTPUT"
echo "$file"
