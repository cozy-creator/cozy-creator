# tree-siblings (cl-1211; se-153's pre-flight 44221031943: `cozy run <checkout>/qwen-image-2/image --describe` failed "only
# birefnet<=0.9.0 is available", the birefnet it needs only in the same tree): a package takes another of its tree from the Hub
# (`{ index = "tensorhub" }`) at a release no Hub has. No rental: this runner's `local`, two tiny CPU packages beside each other.
#   1. path:     `cozy run <tree>/dependent/version` locks and runs, its sibling taken by path: it answers the sibling's version;
#   2. folder:   no uv.lock is written into the dependent's folder (a committed lock pins only what a machine can fetch);
#   3. sibling:  the sibling's own change (a version and a dependency it imports) locks the dependent again: it answers with both;
#   4. checkout: the same tree as a dev home's packages checkout, the dependent by its name: the same answer.
# params: none.
. "$(dirname "$0")/lib.sh"
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build run of rc/<x> or a branch of it>"
# The packages' Runtime floor is the open RC's minor, as a first-party package's is (`>=0.48.0.dev0`).
w=$(curl -fsSL --retry 3 "https://github.com/cozy-creator/cozy-runtime/releases/download/rc-${V%%-*}/SHA256SUMS" |
  awk '$2 ~ /^cozy_runtime-.*-py3-none-any\.whl$/ { split($2, f, "-"); print f[2]; exit }')
[ -n "$w" ] || fail "cozy-runtime's rc-${V%%-*} holds no build"
FLOOR=">=$(cut -d. -f1-2 <<<"$w").0.dev0"
TREE=$OUT/tree
# package <folder> <name> <version> <dependencies> <sources> <module>: a cozy application in the tree.
package() {
  mkdir -p "$TREE/$1/${6%%.*}"
  cat > "$TREE/$1/pyproject.toml" <<EOF
[build-system]
requires = ["uv_build>=0.12.7,<0.13"]
build-backend = "uv_build"

[project]
name = "$2"
version = "$3"
requires-python = ">=3.12"
dependencies = [$4]

[project.entry-points."cozy.application"]
default = "${6%%.*}:app"

[tool.uv.build-backend]
module-name = "${6%%.*}"
module-root = ""
$5
EOF
  cat > "$TREE/$1/${6%%.*}/__init__.py"
}
package sibling e2e-tree-sibling 0.2.0rc1 "\"cozy-runtime$FLOOR\", \"msgspec>=0.19\"" "" e2e_tree_sibling <<'PY'
"""A first-party package no Hub has at this release (cl-1211): what it says is its own version."""
import importlib.metadata

import msgspec

from cozy_runtime.author import App

app = App()


class In(msgspec.Struct):
    text: str = "x"


class Said(msgspec.Struct):
    said: str


def said() -> str:
    return importlib.metadata.version("e2e-tree-sibling")


@app.entrypoint(accelerator=False)
def hello(payload: In) -> Said:
    return Said(said())
PY
package dependent e2e-tree-dependent 0.1.0 "\"cozy-runtime$FLOOR\", \"msgspec>=0.19\", \"e2e-tree-sibling>=0.2.0rc1\"" \
  "$(printf '\n[tool.uv.sources]\ne2e-tree-sibling = { index = "tensorhub" }')" e2e_tree_dependent <<'PY'
"""Takes its sibling from the Hub's index, at a release only its tree holds (cl-1211)."""
import msgspec

from cozy_runtime.author import App
from e2e_tree_sibling import said

app = App()


class In(msgspec.Struct):
    text: str = "x"


class Answer(msgspec.Struct):
    sibling: str


@app.entrypoint(accelerator=False)
def version(payload: In) -> Answer:
    return Answer(said())
PY
say "cozy $V; the tree: e2e-tree-dependent takes e2e-tree-sibling>=0.2.0rc1 from the Hub, which has none; cozy-runtime$FLOOR"
# run <name> <target>: the call in this session's project, on local, straight through cozy (`step` would take a gate's copy).
run() {
  say "run $1: cozy run $2 --on local"
  (cd "$RUN_PROJECT" && cozy run "$2" --on local) > "$OUT/$1.txt" 2>&1 || fail "$1: $(tail -c 400 "$OUT/$1.txt")"
  ANSWER=$(awk '$1 == "sibling" { $1 = ""; sub(/^ +/, ""); print; exit }' "$OUT/$1.txt")
  say "$1: sibling ${ANSWER:-?}"
}
check() { if [ "$1" = "$2" ]; then say "PASS $3"; else say "FAIL $3: $1, not $2"; RC=1; fi; }

run path "$TREE/dependent/version"
check "$ANSWER" 0.2.0rc1 "path: the dependent ran on its sibling from the tree"
[ ! -e "$TREE/dependent/uv.lock" ]; check $? 0 "folder: no uv.lock written into the dependent's folder"

# The sibling's own change: a new version that imports a dependency no lock held before.
sed -i -e 's/^version = "0.2.0rc1"/version = "0.2.0rc2"/' -e 's/^dependencies = \[/dependencies = ["six>=1.16", /' "$TREE/sibling/pyproject.toml"
sed -i 's/^    return importlib.metadata.version("e2e-tree-sibling")$/    import six\n    return f"{importlib.metadata.version(\x27e2e-tree-sibling\x27)} six {six.__version__}"/' \
  "$TREE/sibling/e2e_tree_sibling/__init__.py"
grep -q "import six" "$TREE/sibling/e2e_tree_sibling/__init__.py" || fail "the sibling wasn't changed"
run sibling "$TREE/dependent/version"
check "${ANSWER%% six *}" 0.2.0rc2 "sibling: its own change locked the dependent again ($ANSWER)"
[[ $ANSWER == *" six "* ]]; check $? 0 "sibling: the dependency it took is installed"

# The tree as a dev home's packages checkout: the dependent by its name.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$TREE" '
  function ours() { print "packages = \"" source "\""; put = 1 }
  /^packages *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
run checkout e2e-tree-dependent/version
check "${ANSWER%% six *}" 0.2.0rc2 "checkout: the dev home's checkout ran it by name ($ANSWER)"
cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
[ "$RC" = 0 ] && say "PASS: a package's first-party sibling, which no Hub has, came from its tree by path"
