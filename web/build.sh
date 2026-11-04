#!/usr/bin/env bash
# Builds the page Calcifer embeds (web/app, crates/calcifer/build.rs) into web/app/dist. Needs Node and the pnpm web/app's
# packageManager pins. Every build of cozy that ships the page runs this; a plain `cargo build` compiles without it.
set -euo pipefail
cd "$(dirname "$0")/app"
pnpm install --frozen-lockfile --ignore-scripts
pnpm run build
test -f dist/index.html
