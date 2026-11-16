# The harness itself. Source alongside lib.sh.

# A part that ends on a shell error fails its shard, never passes it (cl-1079; coordinator 11-16: "this can make any shard look green
# while it ran nothing"). A suite of lib.sh's own run_parts and finish, run apart, whose first part ends on the arithmetic error that
# hid most of db_legacy_burst (`$(( $(grep -c … || echo 0) - n ))` reads "0 0"), must exit non-zero naming that part.
harness_abort() {
  step "a part that ends on a shell error fails its shard (cl-1079)"
  local dir=$OUT/harness-abort code=0
  mkdir -p "$dir/out"
  cat > "$dir/suite.sh" <<'EOF'
. "$1/lib.sh"
OUT=$2
aborts() { local n; n=$(( $(printf '0\n0') - 1 )); echo "$n" > "$OUT/went-on"; }
after() { echo ran > "$OUT/after-ran"; }
run_parts aborts after
finish "harness abort"
EOF
  bash "$dir/suite.sh" "$E2E" "$dir/out" > "$dir/suite.log" 2>&1 || code=$?
  echo "harness abort: the suite exited $code: $(grep -a -m 1 -o 'aborts ended on a shell error[^:]*' "$dir/suite.log" || tail -n 1 "$dir/suite.log")"
  [ ! -e "$dir/out/went-on" ] || check "harness abort: the injected error didn't end its part, so this cell proves nothing: $(tail -c 300 "$dir/suite.log")"
  [ "$code" != 0 ] && grep -a -q "aborts ended on a shell error" "$dir/suite.log" \
    || check "harness abort: a suite whose part ended on a shell error exited $code without naming it: $(tail -c 300 "$dir/suite.log" | tr '\n' ' ')"
}
