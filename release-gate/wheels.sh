#!/bin/bash
# wheels.sh <runtime_wheel> <tensorfs_wheel>: release-gate.yml's candidate wheels as gate.sh's arguments, a word a line (none
# for one not given). A wheel is a URL#sha256 the gate fetches and checks, an artifact of this run or of another run of this repo
# (e2e's candidate-runtime-*, dev-build's tensorfs-wheel-*): artifact:<run id>/<name>, or for the Runtime a cozy-runtime
# dev-build run id (its release files, kernels built: cozy-runtime-<sha>). An artifact lands in ./wheels-<flag>. Stdout is the
# arguments alone: what fails says so on stderr.
set -euo pipefail
for w in "runtime-wheel|$1|cozy_runtime" "tensorfs-wheel|$2|tensorfs"; do
  IFS="|" read -r flag value dist <<< "$w"
  [[ $flag != runtime-wheel || ! $value =~ ^[0-9]+$ ]] || value=dev-build:$value
  case $value in
    '') ;;
    dev-build:*)
      gh run download "${value#dev-build:}" -R cozy-creator/cozy-runtime -p 'cozy-runtime-*' -D "wheels-$flag" >&2
      f=$(find "wheels-$flag" -type f -name "$dist-*manylinux*x86_64*.whl" -print -quit)
      [ -n "$f" ] || { echo "::error::cozy-runtime dev-build ${value#dev-build:} has no x86_64 wheel" >&2; exit 1; }
      printf '%s\n' "--$flag" "$PWD/$f" ;;
    artifact:*)
      name=${value#artifact:} run=$GITHUB_RUN_ID
      [[ $name != */* ]] || { run=${name%%/*}; name=${name#*/}; }
      gh run download "$run" -R "$GITHUB_REPOSITORY" -n "$name" -D "wheels-$flag" >&2
      # One of the two globs matches: ls exits 2 for the other, which pipefail would make the script's.
      f=$(ls "wheels-$flag/$dist"-*manylinux*x86_64*.whl "wheels-$flag/$dist"-*-py3-none-any.whl 2>/dev/null | head -1 || true)
      [ -n "$f" ] || { echo "::error::no $dist x86_64 wheel in artifact $value" >&2; exit 1; }
      printf '%s\n' "--$flag" "$PWD/$f" ;;
    *) printf '%s\n' "--$flag" "$value" ;;
  esac
done
