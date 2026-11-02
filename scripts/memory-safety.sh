#!/usr/bin/env bash
# The memory-safety guard (xs-067, cl-722): unsafe only in unsafe-boundaries.txt's files, no known advisory in
# Cargo.lock. `build` runs it when a lockfile, a manifest, an `allow(unsafe_code)` or this guard's own files change.
set -euo pipefail
cd "$(dirname "$0")/.."
export LC_ALL=C

git grep -nE '\bunsafe ?(\{|fn |impl |extern |\()' -- '*.rs' ':!vendor/' | grep -vE '^[^:]+:[0-9]+:\s*//' | cut -d: -f1 | sort -u \
  | diff -u <(grep -v '^#' unsafe-boundaries.txt | sort) -

version=0.20.2
dir=$(mktemp -d)
curl -sSfL "https://github.com/EmbarkStudios/cargo-deny/releases/download/$version/cargo-deny-$version-x86_64-unknown-linux-musl.tar.gz" \
  | tar -xz -C "$dir" --strip-components=1
"$dir/cargo-deny" --locked check advisories
