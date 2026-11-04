#!/usr/bin/env bash
# Builds the page Calcifer embeds (web/app, crates/calcifer/build.rs) into web/app/dist. Needs Node and the pnpm web/app's
# packageManager pins. Every build of cozy that ships the page runs this; a plain `cargo build` compiles without it.
set -euo pipefail
cd "$(dirname "$0")/app"
# macOS's and Windows' disks ignore case: ui/Notes.tsx beside ui/notes.ts built here and failed there.
same=$(find src -type f | sed -E 's/\.[^./]+$//' | sort -u | tr 'A-Z' 'a-z' | sort | uniq -d)
[ -z "$same" ] || { echo "web/build.sh: these differ only by case, which macOS and Windows can't tell apart: $same" >&2; exit 1; }
pnpm install --frozen-lockfile --ignore-scripts
pnpm run build
test -f dist/index.html
