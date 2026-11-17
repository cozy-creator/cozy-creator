#!/usr/bin/env bash
# local-sources.sh <package> <copy> [<packages checkout>]: the package copied to <copy> (printed), each of its
# `{ index = "tensorhub" }` sources that the checkout has a package of taking that package's folder (`{ path = … }`), as
# cozy locks a path dependency (cl-878). A gate fixture calling sdxl then calls the sdxl under test, which the Hub serves
# only after its release (child-lora-stacks: `sdxl>=0.11.0rc2`, while the Hub had 0.10.0).
set -euo pipefail
src=$1 copy=$2 pkgs=${3:-}
rm -rf "$copy"; mkdir -p "$(dirname "$copy")"; cp -r "$src" "$copy"; rm -rf "$copy/.venv"
if [ -n "$pkgs" ]; then
  for dep in $(sed -n 's/^\([A-Za-z0-9._-]*\) *= *{ *index *= *"tensorhub" *}.*$/\1/p' "$copy/pyproject.toml"); do
    at=$(grep -lx "name = \"$dep\"" "$pkgs"/*/pyproject.toml 2> /dev/null | head -n 1) || true
    [ -n "$at" ] || continue
    # Relative, as cozy sends a path dependency beside the package: an absolute one names a folder only this computer has.
    sed -i "s|^$dep *= *{ *index *= *\"tensorhub\" *}.*\$|$dep = { path = \"$(realpath --relative-to="$copy" "$(dirname "$at")")\" }|" "$copy/pyproject.toml"
  done
fi
echo "$copy"
