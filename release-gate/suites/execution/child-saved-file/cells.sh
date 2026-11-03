# A parent gives its child a file it saved (cr-251), and nothing else opens: the child reads the very bytes, and a file of
# another attempt, an ungranted ref, a lying digest, a claimed tree and a symlink are refused before any child runs. CPU,
# under a minute. Needs a Runtime and a TensorD with cr-251.
cell child-saved-file "$HERE/suites/execution/child-saved-file/main"
if [ -f child-saved-file.json ]; then
  got=$(run_get "$OUT" child-saved-file '.result | [(.saved_picture == .seen_picture.digest), (.seen_picture.width == 64),
    (.saved_blob == .seen_blob.digest), .other_attempt, .ungranted, .lying_digest, .claimed_tree, .symlink] | map(tostring) | join(" ")')
  want="true true true foreign_asset child.asset_ungranted child.asset_ungranted child.asset_ungranted child.asset_ungranted"
  [ "$got" = "$want" ] && expect child-saved-file PASS "the child read both files; refused: $got" \
    || expect child-saved-file FAIL "got [$got], want [$want]"
fi
