# Suite `checkpoints` (xs-183): every published checkpoint against what its tool reads (requirements.toml), from headers
# alone through `cozy model show` (walk.py). It needs no machine: skus names none, release-gate.yml's `checkpoints` job runs
# it on a runner, and by hand it is walk.py. Picked with other suites, it runs here beside their cells.
python3 -I "$HERE/suites/checkpoints/walk.py" --cli "$CLI" --out "$OUT/checkpoints" > checkpoints.log 2>&1
case $? in
  0) expect checkpoints PASS "$(tail -1 checkpoints.log)" ;;
  *) expect checkpoints FAIL "$(tail -1 checkpoints.log): $(grep -E '^(GAP|UNREAD) ' checkpoints.log | head -5 | paste -sd ';')" ;;
esac
