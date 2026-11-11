#!/usr/bin/env bash
# interfaces.sh --runtime <wheel file|version> [--tensorfs <wheel file|version>] <dir>...: run by a cut before it rents
# anything, and by gate.sh for its own packages (TensorFS: the Runtime's own requirement from PyPI unless named). A dir is a
# package, or a checkout of packages. Each package from source is described as TensorD describes it (the package contract
# included), its interface written as TensorD writes it, and read back by the reader an executor starts on, all on that
# Runtime. No GPU, no pod, none of a package's dependencies, seconds a package.
# Describing alone passed Runtime 0.40.0, whose interface for minimax-h3 no executor could read.
# Exit 0: every package read back. Exit 1: names each that didn't.
set -uo pipefail
RUNTIME='' TENSORFS='' PACKAGES=()
while [ $# -gt 0 ]; do
  case $1 in
    --runtime) RUNTIME=$2; shift 2 ;;
    --tensorfs) TENSORFS=$2; shift 2 ;;
    *) [ -d "$1" ] || { echo "no directory $1" >&2; exit 2; }; PACKAGES+=("${1%/}"); shift ;;
  esac
done
[ -n "$RUNTIME" ] && [ ${#PACKAGES[@]} -gt 0 ] || { sed -n '2,8s/^# \{0,1\}//p' "$0" >&2; exit 2; }
case $RUNTIME in
  *.whl) WHEEL=$(realpath "$RUNTIME") ;;
  *) WHEEL=https://github.com/cozy-creator/cozy-runtime/releases/download/v$RUNTIME/cozy_runtime-$RUNTIME-py3-none-any.whl
     curl -fsIL "$WHEEL" > /dev/null || WHEEL=cozy-runtime==$RUNTIME ;;
esac
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
PY=$W/venv/bin/python
uv venv -q --python 3.12 "$W/venv" || exit 2
# The Runtime's reader and describer, and the TensorFS its author module imports (`tensorfs.executor`, Runtime 0.44): Torch
# and a package's own dependencies are never imported here.
uv pip install -q --python "$PY" --no-deps "$WHEEL" msgspec numpy packaging pillow || exit 2
[ -n "$TENSORFS" ] || TENSORFS=$("$PY" -I -c 'import importlib.metadata as m; print(next(r.split(";")[0] for r in m.requires("cozy-runtime") if r.startswith("tensorfs")))') || exit 2
case $TENSORFS in
  *.whl) TENSORFS=$(realpath "$TENSORFS") ;;
  [0-9]*) TENSORFS=tensorfs==$TENSORFS ;;
esac
uv pip install -q --python "$PY" --no-deps "$TENSORFS" || exit 2
# TensorD's own (`ask_runtime`, crates/tensord/src/published/environment.rs).
BOOTSTRAP="import pathlib,runpy,sys;\
root=pathlib.Path(sys.executable).absolute().parent.parent;\
sys.path[:0]=[str(p) for p in (*(root/'lib').glob('python*/site-packages'),root/'Lib'/'site-packages') if p.is_dir()];\
sys.argv=['cozy-runtime','machine-describe'];runpy.run_module('cozy_runtime.cli.main',run_name='__main__')"
echo "Runtime $("$PY" -I -c 'import importlib.metadata as m; print(m.version("cozy-runtime"), "/ TensorFS", m.version("tensorfs"))')"
FAILED=() COUNT=0
for project in $(for d in "${PACKAGES[@]}"; do if [ -f "$d/pyproject.toml" ]; then echo "$d"; else ls -d "$d"/*/; fi; done); do
  project=${project%/}; name=${project#"$(dirname "$(dirname "$project")")/"}
  [ -f "$project/pyproject.toml" ] || continue
  COUNT=$((COUNT + 1))
  jq -n --arg project "$(realpath "$project")" --arg python "$PY" '{kind: "describe", project: $project, environment_python: $python}' \
    | "$PY" -I -S -c "$BOOTSTRAP" > "$W/reply.json" 2> "$W/err"
  if ! jq -e '.kind == "described"' "$W/reply.json" > /dev/null 2>&1; then
    echo "FAIL $name: described: $(jq -r '"\(.code): \(.detail)"' "$W/reply.json" 2>/dev/null || tail -1 "$W/err")"
    FAILED+=("$name"); continue
  fi
  # As TensorD's `interface_file`: serde_json's sorted keys, no whitespace.
  jq -cS .interface "$W/reply.json" > "$W/package-interface.json"
  if ! "$W/venv/bin/cozy-runtime" --package-interface "$W/package-interface.json" describe > /dev/null 2> "$W/err"; then
    echo "FAIL $name: read back: $(sed -n 1p "$W/err")"
    FAILED+=("$name"); continue
  fi
  echo "ok   $name: $(jq '.interface.entrypoints | length' "$W/reply.json") functions"
done
[ "$COUNT" -gt 0 ] || { echo "no package under ${PACKAGES[*]}"; exit 2; }
[ ${#FAILED[@]} = 0 ] || { echo "FAILED ${#FAILED[@]} of $COUNT: ${FAILED[*]}"; exit 1; }
echo "PASS: $COUNT packages read back"
