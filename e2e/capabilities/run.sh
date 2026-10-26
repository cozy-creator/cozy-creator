#!/usr/bin/env bash
# e2e/capabilities/run.sh: a run capability reaches what its runs need and nothing else (cl-675, AuthKit's workload
# grant). It runs no run: main.go signs, redeems and reads as Calcifer and TensorD do, with this computer's signed-in cozy.
#   e2e/capabilities/run.sh --out DIR --commit ORG/NAME@sha256:<hex> [--hub ORIGIN] [--rental ID --rental-state live|ended]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
COZY_HUB_URL=${COZY_HUB_URL:-https://tensorhub.com} cozy --json whoami > /dev/null
cd "$here" && GOWORK=off go run . "$@"
