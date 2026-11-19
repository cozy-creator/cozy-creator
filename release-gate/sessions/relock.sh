# relock (cr-346, se-178): a package's `scripts/lock.sh` run on this runner, whose cozy is the dispatched build, so the RC whose
# Runtime the lock takes is that build's own, not a guess from some other cozy. $0: nothing quoted or rented. The lock lands in
# $OUT/<package>.uv.lock, with lock.sh's output in lock.txt and the checkout's diff in diff.txt.
# params:
#   package  the package directory in packages_refs `pk` (required)
#   args     lock.sh's uv lock arguments (default --upgrade-package cozy-runtime --upgrade-package tensorfs)
. "$(dirname "$0")/lib.sh"
P=$(param package '') ARGS=$(param args '--upgrade-package cozy-runtime --upgrade-package tensorfs')
[[ $P =~ ^[a-z0-9._-]+$ ]] && [ -d "$PACKAGES/pk/$P" ] || fail "package '$P': no such directory in packages_refs pk"
say "cozy $(cozy --version); packages at $(git -C "$PACKAGES/pk" rev-parse --short HEAD)"
# shellcheck disable=SC2086  # ARGS is uv's words
(cd "$PACKAGES/pk" && nice -n 19 scripts/lock.sh "$P" $ARGS) > "$OUT/lock.txt" 2>&1 || fail "lock.sh: $(tail -c 400 "$OUT/lock.txt")"
cp "$PACKAGES/pk/$P/uv.lock" "$OUT/$P.uv.lock"
git -C "$PACKAGES/pk" diff > "$OUT/diff.txt"
tail -4 "$OUT/lock.txt" | sed 's/^/  /' | tee -a "$R"
git -C "$PACKAGES/pk" diff --stat | sed 's/^/  /' | tee -a "$R"
