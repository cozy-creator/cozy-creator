#!/usr/bin/env bash
# e2e/capabilities/run.sh: a run capability reaches what its runs need and nothing else (cl-675, AuthKit's workload
# grant). It runs no run: main.go signs, redeems and reads as Calcifer and TensorD do, with this computer's signed-in cozy.
#   e2e/capabilities/run.sh --out DIR --commit ORG/NAME@sha256:<hex> [--hub ORIGIN] [--rental ID --rental-state live|ended]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
. "$here/../test-hub.sh"
target=${COZY_HUB_URL:-}
args=("$@")
for ((i=0; i<${#args[@]}; i++)); do
  case ${args[$i]} in
    --hub) target=${args[$((i+1))]:-};;
    --hub=*) target=${args[$i]#--hub=};;
  esac
done
[ -n "$target" ] || { echo "capabilities: --hub or COZY_HUB_URL is required" >&2; exit 2; }
export COZY_HUB_URL=$target
require_ci_test_hub "$target" || exit 2
if [ -n "${COZY_TEST_HUB_CA:-}" ]; then
  require_test_hub "$target" || exit 2
  export SSL_CERT_FILE=$COZY_TEST_HUB_CA
fi
cozy --json whoami > /dev/null
cd "$here" && GOWORK=off go run . "$@"
