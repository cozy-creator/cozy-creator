#!/usr/bin/env bash
# The attention-table cell (xs-171): the table a Runtime ships, checked against the Runtime source it was built from.
#   table.sh <runtime wheel|""> <runtime version>
# The wheel's stamped commit (else the release tag v<version>) names the source; its scripts/attention_table.py check
# regenerates the table byte for byte, compares every listed kernel's build with the one measured, and fails a kernel
# its class runs that was never measured. The wheel's own table must be that source's. CPU, no rental.
set -euo pipefail
wheel=$1 version=$2 repo=cozy-creator/cozy-runtime
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
if [ -n "$wheel" ]; then
  commit=$(unzip -p "$wheel" cozy_runtime/_build_provenance.py | grep -oE '[0-9a-f]{40}' | head -1 || true)
else
  commit=$(gh api "repos/$repo/commits/v$version" --jq .sha || true)
fi
[ -n "$commit" ] || { echo "FAIL the Runtime names no source commit (an unstamped wheel)"; exit 1; }
gh api "repos/$repo/tarball/$commit" > "$tmp/src.tgz"
mkdir "$tmp/src" && tar -xzf "$tmp/src.tgz" -C "$tmp/src" --strip-components 1
tree=$tmp/src
if [ ! -f "$tree/scripts/attention_table.py" ] || [ ! -f "$tree/src/cozy_runtime/internal/attention_table.json" ]; then
  echo "n/a: Runtime $version (${commit:0:9}) has no generated attention table"; exit 0
fi
if [ -n "$wheel" ] && ! cmp -s <(unzip -p "$wheel" cozy_runtime/internal/attention_table.json) "$tree/src/cozy_runtime/internal/attention_table.json"; then
  echo "FAIL the wheel's attention_table.json is not the one in its source ${commit:0:9}"; exit 1
fi
cd "$tree" && nice -n 19 uv run -q --frozen --no-dev python scripts/attention_table.py check
