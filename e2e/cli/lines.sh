# cl-905: a machine's three lines, installs, downloads and its runs, each in job order and all at once. Three jobs queued
# together on this cold machine, each sent once the machine took the one before: A, BiRefNet (its environment, its Hub
# weights, minutes on a runner's CPU); B, Qwen3.5 0.8B's weights (`cozy model install`, a download alone); C, the weightless
# package (an environment of its own, and a run). A's weights come whole before B's take the link, C builds only after A
# has, and B downloads and C installs while A runs. lines.py prints the timeline (lines.json): each job's spans in each
# line, end to end against the ideal for them.

machine_lines() {
[ "$RUNNER_OS" = Linux ] || return 0
step "three jobs on one machine: each line in job order, all at once (cl-905)"
local began name n dog
began=$(date -u +%s%3N)
dog=$(picture dog2.jpg)
# Each goes once the machine took the one before, so the machine's order is A, B, C.
for name in a b c; do
  case $name in
    a) n=$(run_submit "$OUT" lines-a run "$(birefnet)" --file "image=$dog" --on local) ;;
    b) n=$(run_submit "$OUT" lines-b model install qwen/qwen3.5-0.8b --on local) ;;
    c) n=$(run_submit "$OUT" lines-c run "$here/weightless/reverse" "text=lines$RANDOM" --on local) ;;
  esac
  [ -n "$n" ] || { check "lines: $name was never a run: $(tail -c 300 "$OUT/lines-$name.err")"; return; }
  for _ in $(seq 600); do cozy --json run watch "$n" --once 2> /dev/null | jq -e '.accepted_at_ms != null' > /dev/null && break; sleep 0.2; done
done
for name in a b c; do
  run_wait "$OUT" "lines-$name" || check "lines: $name (run $(cat "$OUT/lines-$name.number")) did not end: $(tail -c 300 "$OUT/lines-$name.ndjson")"
  [ "$(run_get "$OUT" "lines-$name" .status)" = completed ] || check "lines: $name (run $(cat "$OUT/lines-$name.number")) $(run_get "$OUT" "lines-$name" .status): $(run_get "$OUT" "lines-$name" .reason.message | head -c 300)"
done
cozy machine logs local --since 2h > "$OUT/lines.tensord.log" || check "lines: machine logs local failed"
python3 "$here/lines.py" "$OUT" "$began" "$OUT/lines.tensord.log" a b c > "$OUT/lines.json" 2> "$OUT/lines.failed" || true
cat "$OUT/lines.json"
while read -r why; do check "lines: $why"; done < "$OUT/lines.failed"
}
