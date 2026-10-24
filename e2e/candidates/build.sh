#!/usr/bin/env bash
# e2e/candidates/build.sh <cozy|tensorfs|runtime> <src> <out>: one candidate for this runner's OS, into <out>; prints
# its file. cozy (TensorD, the dev cozy <version>+dev.g<sha8>) and the TensorFS wheel come from the cozy-creator
# checkout <src>, the Runtime wheel from the cozy-runtime checkout <src>.
#   TAKE=<spec>  a built one instead: a version (PyPI for the wheels, the v<version> release for cozy), a release tag
#                (cozy-creator's v<version> / tensorfs-v<version>, cozy-runtime's v<version>; this OS's asset, checked
#                against the release's SHA256SUMS), or a URL#sha256=<hex>
#   REUSE=1      cozy or tensorfs as a cozy-creator merge-group or dispatched e2e run uploaded it (candidate-<what>-<sha>)
#                for <src>'s commit, or for an older one on its first-parent line with the same build inputs
# With $GITHUB_OUTPUT, `name` is the artifact's: candidate-<what>-<commit>, or candidate-<what>-taken.
set -euo pipefail
what=$1 src=$2 out=$3
exe=''
[ "${RUNNER_OS:-}" != Windows ] || exe=.exe
mkdir -p "$out"
case "${RUNNER_OS:-Linux}-$(uname -m)" in
  Linux-x86_64) whl='manylinux*x86_64.whl' target=x86_64-unknown-linux-gnu ;;
  Linux-aarch64) whl='manylinux*aarch64.whl' target=aarch64-unknown-linux-gnu ;;
  macOS-arm64) whl='macosx*arm64.whl' target=aarch64-apple-darwin ;;
  macOS-x86_64) whl='macosx*x86_64.whl' target=x86_64-apple-darwin ;;
  Windows-*) whl='win_amd64.whl' target=x86_64-pc-windows-msvc ;;
esac
sum() { if command -v sha256sum > /dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

# take <repo> <spec> <wheel name | cozy>: a built candidate for this OS.
take() {
  local repo=$1 spec=$2 name=$3 dir file want
  dir=$(mktemp -d)
  if [[ $spec == *://* ]]; then
    file=$dir/$(basename "${spec%%#*}"); curl -fsSL -o "$file" "${spec%%#*}"
    want=${spec##*#sha256=}; [ "$want" = "$spec" ] || [ "$(sum "$file")" = "$want" ] || { echo "$file: not sha256 $want" >&2; return 1; }
  elif [[ $spec == v* || $spec == tensorfs-v* || $name == cozy ]]; then
    [[ $spec == v* || $spec == tensorfs-v* ]] || spec=v$spec
    if [ "$name" = cozy ]; then
      gh release download "$spec" -R "cozy-creator/$repo" -D "$dir" -p "cozy-$target.*" -p SHA256SUMS -p '*.sha256' || true
    else
      gh release download "$spec" -R "cozy-creator/$repo" -D "$dir" -p "$name-*-$whl" -p "$name-*-py3-none-any.whl" -p SHA256SUMS || true
    fi
    # This OS's archive, or wheel: abi3 first, then CPython 3.12's, then the pure one.
    file=''
    for f in "$dir"/cozy-"$target".tar.gz "$dir"/cozy-"$target".zip "$dir"/$name-*-abi3-$whl "$dir"/$name-*-cp312-cp312-$whl "$dir"/$name-*-py3-none-any.whl; do
      [ -e "$f" ] && { file=$f; break; }
    done
    [ -n "$file" ] || { echo "release $spec of cozy-creator/$repo has no $name for $target" >&2; return 1; }
    want=$( (cat "$dir"/SHA256SUMS "$file.sha256" 2>/dev/null || true) | grep -F "$(basename "$file")" | head -1 | cut -d' ' -f1)
    [ -z "$want" ] || [ "$(sum "$file")" = "$want" ] || { echo "$file: not its SHA256SUMS $want" >&2; return 1; }
  else
    uvx -q --from pip pip download -q --no-deps --only-binary :all: "${name//_/-}==$spec" -d "$dir"
    file=$(ls "$dir"/*.whl | head -1)
  fi
  if [ "$name" = cozy ] && [[ $file != *.whl ]]; then
    # dist's archive: one top directory holding cozy (cozy.exe on Windows).
    uv run -q --no-project python - "$file" "$out/cozy$exe" <<'PY'
import sys, tarfile, zipfile
src, dst = sys.argv[1:]
if src.endswith(".zip"):
    z = zipfile.ZipFile(src); data = z.read(next(n for n in z.namelist() if n.split("/")[-1] in ("cozy", "cozy.exe")))
else:
    t = tarfile.open(src); data = t.extractfile(next(m for m in t.getmembers() if m.name.split("/")[-1] == "cozy")).read()
open(dst, "wb").write(data)
PY
    chmod +x "$out/cozy$exe"; file=$out/cozy$exe
  else
    cp "$file" "$out/"; file=$out/$(basename "$file")
  fi
  echo "$file"
}

# reuse: the candidate an earlier cozy-creator e2e run built from these sources, walking back from <src>'s commit to
# the last one that touched a build input. Only a merge group's or a dispatch's run counts: a pull request's runs the
# PR's own workflow.
reuse() {
  local last sha run
  last=$(git -C "$src" rev-list -n1 HEAD -- crates Cargo.toml Cargo.lock rust-toolchain.toml e2e/candidates)
  for sha in $(git -C "$src" rev-list --first-parent -n 30 HEAD); do
    for run in $(gh api "repos/cozy-creator/cozy-creator/actions/artifacts?name=candidate-$what-$sha" \
      --jq '.artifacts[] | select(.expired | not) | .workflow_run.id'); do
      gh api "repos/cozy-creator/cozy-creator/actions/runs/$run" --jq '.event + " " + .path' \
        | grep -qxE '(merge_group|workflow_dispatch) \.github/workflows/e2e\.yaml' || continue
      gh run download "$run" -R cozy-creator/cozy-creator -n "candidate-$what-$sha" -D "$out" > /dev/null || continue
      echo "$what: reused candidate-$what-$sha of https://github.com/cozy-creator/cozy-creator/actions/runs/$run" >&2
      return 0
    done
    [ "$sha" != "$last" ] || break
  done
  return 1
}

name=candidate-$what-taken
case $what in
  cozy) repo=cozy-creator wheel=cozy ;;
  tensorfs) repo=cozy-creator wheel=tensorfs ;;
  runtime) repo=cozy-runtime wheel=cozy_runtime ;;
  *) echo "build.sh: no candidate $what" >&2; exit 2 ;;
esac
if [ -n "${TAKE:-}" ]; then
  file=$(take "$repo" "$TAKE" "$wheel")
  echo "$what: $(basename "$file"), taken ($TAKE)" >&2
else
  sha=$(git -C "$src" rev-parse HEAD)
  name=candidate-$what-$sha
  if [ -n "${REUSE:-}" ]; then
    # REUSE=1 only reuses: nothing is written, and no name, when no run has these sources' candidate.
    reuse || { echo "$what: no e2e run of cozy-creator holds a build of ${sha:0:12}'s sources" >&2; exit 0; }
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
    (cd "$src"
      uv run -q --no-project python scripts/stamp-build-provenance.py "$sha"
      uv build -q --wheel --out-dir "$out")
  fi
  case $what in
    cozy) file=$out/cozy$exe; chmod +x "$file" ;;
    tensorfs) file=$(ls "$out"/tensorfs-*.whl) ;;
    runtime) file=$(ls "$out"/cozy_runtime-*.whl) ;;
  esac
  echo "$what: $([ "$what" != cozy ] || "$file" --version) $(basename "$file") from $repo ${sha:0:12}" >&2
fi
[ -z "${GITHUB_OUTPUT:-}" ] || echo "name=$name" >> "$GITHUB_OUTPUT"
echo "$file"
