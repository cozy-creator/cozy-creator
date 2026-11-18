# tree-siblings (cl-1211; se-153's pre-flight 44221031943: `cozy run <checkout>/qwen-image-2/image --describe` failed "only
# birefnet<=0.9.0 is available", the birefnet it needs only in the same tree): a package takes another of its tree from the Hub
# (`{ index = "tensorhub" }`) at a release no Hub has. No rental: this runner's `local`, two tiny CPU packages beside each other.
#   1. path:     `cozy run <tree>/dependent/version` locks and runs, its sibling taken by path: it answers the sibling's version;
#   2. folder:   no uv.lock is written into the dependent's folder (a committed lock pins only what a machine can fetch);
#   3. sibling:  the sibling's own change (a version and a dependency it imports) locks the dependent again: it answers with both;
#   4. checkout: the same tree as a dev home's packages checkout, the dependent by its name: the same answer.
#   5. floor:    its lock takes the RC's Runtime build, whose version names its commit (`0.48.0.dev2316+ge1ab5640`), and `local` has no
#      Runtime of its own, so its install asks uv for that build's series at its newest patch. cl-1219 (se-155's pre-flight
#      44236236787: every source install warned "Failed to parse: `cozy-runtime>=0.48.0.dev2316+ge1ab5640,==0.48.*`", PEP 440
#      taking no commit after `>=`, and ran on the lock's unasked): no run warns of its lock's Runtime, and it runs on that build.
#   6. patch:    what that requirement asks (cl-795: a patch is fixes): a package alone in its folder, its lock held to the oldest
#      TensorFS of its series, runs on the series' newest; its lock, written into its folder, still names the oldest. A lock on the
#      open RC's TensorFS build (xs-216), a series with no release yet, has nothing older to be held to.
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
import importlib.metadata

import msgspec

from cozy_runtime.author import App
from e2e_tree_sibling import said

app = App()


class In(msgspec.Struct):
    text: str = "x"


class Answer(msgspec.Struct):
    sibling: str
    ran_on: str


@app.entrypoint(accelerator=False)
def version(payload: In) -> Answer:
    return Answer(said(), importlib.metadata.version("cozy-runtime"))
PY
ALONE=$OUT/alone
TREE=$ALONE package patch e2e-lock-patch 0.1.0 "\"cozy-runtime$FLOOR\", \"msgspec>=0.19\"" "" e2e_lock_patch <<'PY'
"""Answers the TensorFS its environment holds (cl-1219)."""
import importlib.metadata

import msgspec

from cozy_runtime.author import App

app = App()


class In(msgspec.Struct):
    text: str = "x"


class Held(msgspec.Struct):
    tensorfs: str


@app.entrypoint(accelerator=False)
def held(payload: In) -> Held:
    return Held(importlib.metadata.version("tensorfs"))
PY
say "cozy $V; the tree: e2e-tree-dependent takes e2e-tree-sibling>=0.2.0rc1 from the Hub, which has none; cozy-runtime$FLOOR"
# run <name> <target>: the call in this session's project, on local, straight through cozy (`step` would take a gate's copy).
run() {
  say "run $1: cozy run $2 --on local"
  (cd "$RUN_PROJECT" && cozy run "$2" --on local) > "$OUT/$1.txt" 2>&1 || fail "$1: $(tail -c 400 "$OUT/$1.txt")"
  ANSWER=$(awk '$1 == "sibling" { $1 = ""; sub(/^ +/, ""); print; exit }' "$OUT/$1.txt")
  RAN_ON=$(awk '$1 == "ran_on" { print $2; exit }' "$OUT/$1.txt")
  HELD=$(awk '$1 == "tensorfs" { print $2; exit }' "$OUT/$1.txt")
  say "$1: ${ANSWER:+sibling $ANSWER, on Runtime ${RAN_ON:-?}}${HELD:+on TensorFS $HELD}"
  # Its lock's pair asked of uv at its newest patch (cl-1219): a requirement uv can't read, or one that doesn't install, is a warning.
  local said
  said=$(grep -m1 -o "Failed to parse: .*" "$OUT/$1.txt" || grep -m1 -o "no newer patch of its lock's .*" "$OUT/$1.txt")
  [ -z "$said" ]; check $? 0 "floor ($1): no warning of its lock's Runtime${said:+ ($said)}"
}
check() { if [ "$1" = "$2" ]; then say "PASS $3"; else say "FAIL $3: $1, not $2"; RC=1; fi; }

run path "$TREE/dependent/version"
check "$ANSWER" 0.2.0rc1 "path: the dependent ran on its sibling from the tree"
[[ $RAN_ON == "$(cut -d. -f1-2 <<<"$w")".0.dev*+g* ]]
check $? 0 "floor: it runs on the RC build its lock took, named with its commit (${RAN_ON:-?})"
[ ! -e "$TREE/dependent/uv.lock" ]; check $? 0 "folder: no uv.lock written into the dependent's folder"

# The sibling's own change: a new version that imports a dependency no lock held before.
sed -i -e 's/^version = "0.2.0rc1"/version = "0.2.0rc2"/' -e 's/^dependencies = \[/dependencies = ["six>=1.16", /' "$TREE/sibling/pyproject.toml"
sed -i 's/^    return importlib.metadata.version("e2e-tree-sibling")$/    import six\n    return f"{importlib.metadata.version(\x27e2e-tree-sibling\x27)} six {six.__version__}"/' \
  "$TREE/sibling/e2e_tree_sibling/__init__.py"
grep -q "import six" "$TREE/sibling/e2e_tree_sibling/__init__.py" || fail "the sibling wasn't changed"
run sibling "$TREE/dependent/version"
check "${ANSWER%% six *}" 0.2.0rc2 "sibling: its own change locked the dependent again ($ANSWER)"
[[ $ANSWER == *" six "* ]]; check $? 0 "sibling: the dependency it took is installed"

# The lock's pair at its series' newest patch: a fresh lock's TensorFS is the newest, so the lock is held to the series' oldest.
run alone "$ALONE/patch/held"
NEWEST=$HELD
# locked <distribution>: the version the package's own uv.lock names.
locked() { awk -v row="name = \"$1\"" '$0 == row { getline; gsub(/^version = "|"$/, ""); print; exit }' "$ALONE/patch/uv.lock" 2> /dev/null; }
OLDEST=$(curl -fsSL --retry 3 https://pypi.org/pypi/tensorfs/json | jq -r '.releases | to_entries[] | select(.value | length > 0 and (all(.yanked) | not)) | .key' |
  grep -xE "$(cut -d. -f1-2 <<<"$NEWEST" | sed 's/\./\\./')\.[0-9]+" | sort -V | head -1)
if [[ $NEWEST == *.dev* ]]; then
  # Its lock takes the open RC's TensorFS beside the RC's Runtime, a build of the next minor (xs-216): no release of it to hold the lock to.
  say "patch: TensorFS $NEWEST is the open RC's build, of a series with no release yet, so no lock is older"
elif [ -z "$NEWEST" ] || [ -z "$OLDEST" ]; then
  say "FAIL patch: its environment's TensorFS (${NEWEST:-none}) or PyPI's releases of its series weren't read"; RC=1
elif [ "$OLDEST" = "$NEWEST" ]; then
  say "patch: TensorFS $NEWEST is its series' only release, so no lock is older"
else
  printf '\n[tool.uv]\nconstraint-dependencies = ["tensorfs==%s"]\n' "$OLDEST" >> "$ALONE/patch/pyproject.toml"
  run patch "$ALONE/patch/held"
  check "$(locked tensorfs)" "$OLDEST" "patch: its lock names the series' oldest TensorFS"
  check "$HELD" "$NEWEST" "patch: it runs on the series' newest TensorFS, over its lock's $OLDEST"
fi

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
[ "$RC" = 0 ] && say "PASS: a package's first-party sibling, which no Hub has, came from its tree by path, its lock's Runtime asked of uv without a warning"
