#!/usr/bin/env bash
# The type check: mypy --strict (mypy.ini) against the pinned real dependencies, once per import root, since the
# scripts reuse module names (six check.py). Errors in .mypy-baseline.txt pass; a new one fails, and so does a fixed
# one left in the baseline.
#   scripts/typecheck.sh          check
#   scripts/typecheck.sh prune    drop fixed errors from the baseline (it never grows)
#   scripts/typecheck.sh lock     re-pin typecheck-requirements.txt from typecheck-requirements.in
# A root is a file's nearest ancestor without an __init__.py, and mypy takes it as a package base (--explicit-package-bases),
# so a root under a package (suites/tensorfs/programs) isn't named into it. Its run sees what its scripts put on sys.path
# themselves, TensorFS from this tree (crates/tensorfs-py/python) and the stubs in typings/.
set -euo pipefail
cd "$(dirname "$0")/.."
top=$PWD
req=scripts/typecheck-requirements
venv=.venv-typecheck

if [ "${1:-check}" = lock ]; then
  exec uv pip compile -q --python-version 3.12 --python-platform x86_64-manylinux_2_28 --no-header --no-annotate "$req.in" -o "$req.txt"
fi
if ! cmp -s "$req.txt" "$venv/requirements.txt"; then
  uv venv -q --clear --python 3.12 "$venv"
  uv pip install -q --python "$venv/bin/python" -r "$req.txt"
  cp "$req.txt" "$venv/requirements.txt"
fi

roots() {  # "<root> <file>…": each root's own files, relative to it (a directory would take in nested roots too)
  # e2e/cpu is checked by e2e/cpu/run.sh instead: per package, in the environment its fixture builds models with.
  git ls-files -- '*.py' '*.pyi' ':!e2e/cpu' | while read -r file; do
    [ "${file%.py}" = "$file" ] || [ ! -f "${file}i" ] || continue  # a module with a stub is checked as its stub
    dir=$(dirname "$file")
    while [ -f "$dir/__init__.py" ]; do dir=$(dirname "$dir"); done
    echo "$dir ${file#"$dir"/}"
  done | LC_ALL=C sort | awk '$1 != last { if (line) print line; line = $1; last = $1 } { line = line " " $2 } END { print line }'
}

mypy_path() {
  local path=$top/crates/tensorfs-py/python:$top/typings
  case $1 in
    e2e/capabilities) path=$top/e2e:$path ;;
    release-gate) path=$top/release-gate/suites:$top/release-gate/suites/bench:$path ;;  # analyze.py's suites (its `tensorfs` is the suite), bench.py's media and comfy
    release-gate/suites/varena/core/faults) path=$top/release-gate/suites/varena/native-tensor-export:$top/release-gate/suites/varena/ledger:$path ;;
    release-gate/suites/varena/public-api) path=$top/crates/varena/examples:$path ;;
    e2e/cli/project/probe) path=$top/e2e/cli/project/probe-callee:$path ;;
  esac
  case $1 in release-gate | release-gate/*) path=$top/release-gate:$path ;; esac
  echo "$path"
}

out=$(mktemp)
trap 'rm -f "$out" "$out.root"' EXIT
# mypy exits 1 when it reports errors (the baseline judges those) and 2 when it cannot run.
while read -r root files; do
  status=0
  # shellcheck disable=SC2086  # file names hold no spaces
  (cd "$root" && MYPYPATH=$(mypy_path "$root") "$top/$venv/bin/mypy" --config-file "$top/mypy.ini" \
    --cache-dir "$top/.mypy_cache" --python-executable "$top/$venv/bin/python" --no-error-summary --no-pretty \
    --show-error-codes --explicit-package-bases $files) > "$out.root" || status=$?
  if [ "$status" -gt 1 ]; then cat "$out.root" >&2; echo "typecheck: mypy could not run in $root" >&2; exit "$status"; fi
  sed "s|^|$root/|" "$out.root" >> "$out"
done < <(roots)
"$venv/bin/python" scripts/typecheck.py "${1:-check}" < "$out"
