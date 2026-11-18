#!/usr/bin/env bash
# release-gate/errexit-lint.sh (cl-1204, build.yaml): `set -e` in release-gate/ and e2e/ shell only as a script's first command. Bash
# ignores it in a subshell, group or function whose status is tested (`|| fail`, `&&`, `if`, `!`): sessions/h3-vae.sh's
# `( set -e; …; uv pip install …; … ) || fail` ran on past a failed install. Such a block ends each step itself (`|| exit`,
# `|| return 1`). A new shell's own first command (`sh -c 'set -e; …'`) and heredoc bodies are not this shell's.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2
# shellcheck disable=SC2016  # an awk program
bad=$(git ls-files -z 'release-gate/*.sh' 'e2e/*.sh' | xargs -0 awk '
  FNR == 1 { first = 1; doc = "" }
  doc != "" { if ($0 ~ "^\t*" doc "$") doc = ""; next }
  /^[[:space:]]*(#|$)/ { next }
  /(^|[;&|({[:space:]])set[[:space:]]+(-[[:alnum:]]*e|-o[[:space:]]+errexit)/ && !/sh -c .set -e/ && !(first && /^set /) {
    print FILENAME ":" FNR ": " $0
  }
  { first = 0 }
  match($0, /<<-?[[:space:]]*[\047"]?[A-Za-z_][A-Za-z0-9_]*/) && !/<<</ {
    doc = substr($0, RSTART, RLENGTH); sub(/^<<-?[[:space:]]*[\047"]?/, "", doc)
  }')
[ -z "$bad" ] || { printf '::error::set -e past a script'"'"'s first command (release-gate/errexit-lint.sh):\n%s\n' "$bad"; exit 1; }
echo "errexit: set only as a script's first command"
