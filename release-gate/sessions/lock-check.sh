# lock-check (cl-1210, proof only): a packages checkout's package locked and installed on this runner by the session's cozy, as a gate's
# cells lock it before they reach a pod (`local_code::locked`): `cozy package install <dir> --on local`, and the Runtime and TensorFS the
# lock it made names. Rents nothing.
# params:
#   tree   a packages_refs checkout (`-f packages_refs=pk=<ref>`; default pk)
#   pkg    its package (default minimax-h3)
. "$(dirname "$0")/lib.sh"
TREE=$(param tree pk) PKG=$(param pkg minimax-h3)
SOURCE=$(cd "$PACKAGES/$TREE/$PKG" 2> /dev/null && pwd) || fail "no $PKG in the packages checkout $TREE"
say "cozy $(cozy --version | awk '{print $2}'); $PKG needs $(grep -o '"cozy-runtime[^"]*"' "$SOURCE/pyproject.toml"), its own lock $(grep -A1 '^name = "cozy-runtime"' "$SOURCE/uv.lock" | tail -1 | tr -d ' ')"
say "the RC's served Runtime builds: $(gh release view rc-0.27.0 -R cozy-creator/cozy-runtime --json assets -q '.assets[].name' 2> /dev/null | grep -oE 'dev[0-9]+\+g[0-9a-f]+' | sort -u | tr '\n' ' ')"
cozy package install "$SOURCE" --on local -y > "$OUT/install.txt" 2>&1; code=$?
say "package install exit $code: $(tail -c 800 "$OUT/install.txt" | tr '\n' ' ')"
# The lock it made, beside the copy (`lock_dir`): the Runtime and TensorFS it names.
for lock in $(find "$HOME/.cozy" -path '*lock*' -name lock -type f -size +0 2> /dev/null | head -5); do
  say "lock $lock: cozy-runtime $(grep -A1 '^name = "cozy-runtime"' "$lock" | tail -1 | tr -d ' '), tensorfs $(grep -A1 '^name = "tensorfs"' "$lock" | tail -1 | tr -d ' ')"
done
[ "$code" = 0 ] || fail "the lock or install failed: $(grep -m1 -E 'lock_failed|unsatisfiable|error' "$OUT/install.txt" | cut -c1-300)"
say "PASS: $PKG locked and installed on this runner"
