# cl-905: a machine's three lines, installs, downloads and its runs, each in job order and all at once. Three jobs queued
# together on this cold machine: A, BiRefNet (its environment, its Hub weights, a run on the runner's CPU); B, Qwen3.5
# 0.8B's weights (`cozy model install`, a download alone); C, the weightless package (an environment of its own, and a run).
# One environment builds at a time, A's weights come whole before B's take the link, and B downloads while A starts and
# runs. lines.py prints the timeline (lines.json): each job's spans in each line, end to end against the ideal for them.
# The race release run 44011938933 hit by chance, forced: A's header comes 4 s late (e2e/hubcheck --delays), so B, sent once A
# is accepted, resolves first. A still downloads first and whole: its place in line is its acceptance's (cl-905).

machine_lines() {
[ "$RUNNER_OS" = Linux ] || return 0
step "three jobs on one machine: each line in job order, all at once (cl-905)"
local began name n dog a c
began=$(date -u +%s%3N)
dog=$(picture dog2.jpg)
cozy --json model show zhengpeng7/birefnet > "$OUT/lines-a-model.json" || check "lines: model show zhengpeng7/birefnet failed"
jq -r '.lanes[]?.header // empty | ltrimstr("sha256:") | "GET \(.) 4000"' "$OUT/lines-a-model.json" > "$OUT/hub-delays"
[ -s "$OUT/hub-delays" ] || check "lines: no header of zhengpeng7/birefnet to hold back: $(head -c 300 "$OUT/lines-a-model.json")"
# A and C at once: local code installs before its call is sent, so their installs meet in the install line. B once A's call
# is on the machine, so the machine takes its download after A's.
run_submit "$OUT" lines-a run "$(birefnet)" --file "image=$dog" --on local > /dev/null & a=$!
run_submit "$OUT" lines-c run "$here/weightless/reverse" "text=lines$RANDOM" --on local > /dev/null & c=$!
wait "$a" "$c"
for name in a c; do [ "$(cat "$OUT/lines-$name.number" 2> /dev/null)" != none ] || { check "lines: $name was never a run: $(tail -c 300 "$OUT/lines-$name.err")"; return; }; done
n=$(cat "$OUT/lines-a.number")
for _ in $(seq 600); do cozy --json run watch "$n" --once 2> /dev/null | jq -e '.accepted_at_ms != null' > /dev/null && break; sleep 0.2; done
n=$(run_submit "$OUT" lines-b model install qwen/qwen3.5-0.8b --on local)
[ -n "$n" ] || { check "lines: b was never a run: $(tail -c 300 "$OUT/lines-b.err")"; return; }
for name in a b c; do
  run_wait "$OUT" "lines-$name" || check "lines: $name (run $(cat "$OUT/lines-$name.number")) did not end: $(tail -c 300 "$OUT/lines-$name.ndjson")"
  [ "$(run_get "$OUT" "lines-$name" .status)" = completed ] || check "lines: $name (run $(cat "$OUT/lines-$name.number")) $(run_get "$OUT" "lines-$name" .status): $(run_get "$OUT" "lines-$name" .reason.message | head -c 300)"
done
rm -f "$OUT/hub-delays"
cozy --json run ls --all --limit 40 > "$OUT/lines-runs.json" || check "lines: run ls failed"
cozy machine logs local --since 2h > "$OUT/lines.tensord.log" || check "lines: machine logs local failed"
# What each resolution pulled (`pull … header= tree=`): those objects are no job's download.
cozy machine logs local tensorfs --lines 0 --since 2h > "$OUT/lines.tensorfs.log" 2>&1 || check "lines: machine logs local tensorfs failed"
python3 "$here/lines.py" "$OUT" "$began" "$OUT/lines.tensord.log" a b c > "$OUT/lines.json" 2> "$OUT/lines.failed" || true
cat "$OUT/lines.json"
while read -r why; do check "lines: $why"; done < "$OUT/lines.failed"
}
