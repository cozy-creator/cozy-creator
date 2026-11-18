#!/usr/bin/env bash
# Stamps the tree as a build of its commit and prints the version: a local version, which PyPI never accepts, in
# pyproject.toml, the tensorfs and tensorfs-py manifests and their Cargo.lock rows (by text: no cargo).
#   the open RC's dev line (pyproject X.Y.Z.dev0, crates X.Y.Z-dev): X.Y.Z.dev<n>+g<sha8>, n the commit's first-parent count,
#     as the Runtime's RC builds are (cl-952), so the Runtime's floor names a build (`tensorfs>=0.27.0.dev3298`, xs-216). The
#     crates say it as semver, X.Y.Z-dev.<n>+g<sha8>.
#   a release's line (X.Y.Z): X.Y.Z+dev.g<sha8>.
# Every build of a tree stamps here: rc-dev's and dev-build's wheel, tensorfs-publish's `dev`, the e2e's candidate.
set -euo pipefail
cd "$(dirname "$0")/.."
python=$(command -v python3 || command -v python)
sha=$(git rev-parse HEAD | cut -c1-8)
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' pyproject.toml | head -1)
case $version in
  *.dev0)
    # The count needs the history: CI checks it out whole and treeless (fetch-depth 0, filter tree:0). A shallow clone fetches
    # it whole here; `--filter` would turn it into a partial clone, whose repack of local objects git can abort on (BUG in
    # pack-objects.c, e2e 44246156695).
    [ "$(git rev-parse --is-shallow-repository)" = false ] || git fetch -q --unshallow origin
    n=$(git rev-list --count --first-parent HEAD)
    wheel="${version%.dev0}.dev$n+g$sha" crate="${version%.dev0}-dev.$n+g$sha" ;;
  *+* | *dev*) echo "dev-version.sh: $version is stamped already, or no X.Y.Z or X.Y.Z.dev0" >&2; exit 1 ;;
  *) wheel="$version+dev.g$sha" crate=$wheel ;;
esac
"$python" - "$wheel" "$crate" <<'PY'
import re, sys

wheel, crate = sys.argv[1:3]
for path, version in (("pyproject.toml", wheel), ("Cargo.toml", crate), ("../tensorfs/Cargo.toml", crate)):
    text, n = re.subn(r'(?m)^version = "[^"+]+"$', f'version = "{version}"', open(path).read(), count=1)
    assert n == 1, path
    open(path, "w", newline="\n").write(text)
lock = open("../../Cargo.lock").read()
for name in ("tensorfs", "tensorfs-py"):
    lock, n = re.subn(rf'(name = "{name}"\nversion = ")[^"+]+(")', rf"\g<1>{crate}\g<2>", lock, count=1)
    assert n == 1, name
open("../../Cargo.lock", "w", newline="\n").write(lock)
PY
echo "$wheel"
