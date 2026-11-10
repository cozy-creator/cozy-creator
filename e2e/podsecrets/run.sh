#!/usr/bin/env bash
# xs-157: a rental pod holds no secret. The real Hub, its Docker provider renting a pod that runs
# the cozy binary under test, on this computer's Docker Engine. Needs Docker and Go.
#
#   e2e/podsecrets/run.sh <tensorhub checkout> <cozy binary> <out dir>
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
hub_src="$(cd "$1" && pwd)" cozy="$2" out="$3"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
(cd "$hub_src" && nice -n 19 go build -o "$out/tensorhub" .)
(cd "$here" && nice -n 19 go build -o "$out/podsecrets" .)
exec "$out/podsecrets" --tensorhub "$out/tensorhub" --cozy "$cozy" --out "$out/run"
