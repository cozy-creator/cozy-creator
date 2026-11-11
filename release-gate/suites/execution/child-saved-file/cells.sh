# A parent gives its child files it saved, a labeled copy of one too, and nothing else opens (cr-251, cr-256): the child
# reads exactly the parent's bytes; a file of another attempt and an ungranted file are refused. Forged child-call frames
# (a lying digest, a claimed tree manifest, a spool symlink) are the Runtime's own exerciser, `bench.child_wire` (drive/).
cell child-saved-file "$HERE/suites/execution/child-saved-file/main"
if [ -f child-saved-file.json ]; then
  got=$(run_get "$OUT" child-saved-file '.result | [(.saved_picture == .seen_picture.digest), (.seen_picture.width == 64),
    (.saved_labeled == .seen_labeled.digest), (.seen_labeled.width == 32), (.saved_blob == .seen_blob.digest),
    (.other_attempt != ""), (.ungranted != "")] | map(tostring) | join(" ")')
  codes=$(run_get "$OUT" child-saved-file '.result | "other attempt \(.other_attempt), ungranted \(.ungranted), earlier \(.earlier_attempt)"')
  [ "$got" = "true true true true true true true" ] && expect child-saved-file PASS "the child read all three files; refused: $codes" \
    || expect child-saved-file FAIL "got [$got], want all true; $codes"
fi
cell child-wire "$HERE/suites/execution/drive/child_wire"
