#!/usr/bin/env bash
# web/app/src/api/calcifer.ts is api's types as ts-rs declares them (crates/api/examples/ts.rs): this fails when the file
# holds anything else. `scripts/web-types.sh --write` writes it. ts-rs is api's `ts` feature, which only this enables.
set -euo pipefail
cd "$(dirname "$0")/.."
file=web/app/src/api/calcifer.ts
fresh=$(cargo run --quiet --locked -p api --features ts --example ts)
if [ "${1:-}" = --write ]; then
  printf '%s\n' "$fresh" >"$file"
  echo "web-types: wrote $file"
  exit
fi
if ! diff -u "$file" <(printf '%s\n' "$fresh") >&2; then
  echo "web-types: $file is not what api declares: scripts/web-types.sh --write, and commit it" >&2
  exit 1
fi
echo "web-types: $file is api's $(grep -c '^export ' "$file") types"
