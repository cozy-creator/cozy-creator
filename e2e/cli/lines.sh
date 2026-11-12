# cl-905: a machine's three lines, installs, downloads and its runs, each in job order and all at once. Two jobs queued
# together on this cold machine: A, BiRefNet (its environment, its Hub weights, minutes on a runner's CPU), then B,
# Qwen3.5 at 0.8B (another environment, other weights). A's weights come whole before B's take the link, B builds only
# after A has, and B installs and downloads while A runs. lines.py prints the timeline (lines.json): each job's spans in
# each line, end to end against the ideal for them.

# Qwen3.5's `ask`: a pinned release, the packages checkout's directory, or the published release (as `birefnet`).
qwen() {
  if [ -z "$PACKAGES" ] || [[ $(pinned qwen3.5) == *@* ]]; then pinned qwen3.5 ask
  else (cd "$PACKAGES" && printf '%s/qwen3.5/ask\n' "$PWD"); fi
}

machine_lines() {
[ "$RUNNER_OS" = Linux ] || return 0
step "two jobs on one machine: each line in job order, all at once (cl-905)"
local began a b dog
began=$(date -u +%s%3N)
dog=$(picture dog2.jpg)
a=$(run_submit "$OUT" lines-a run "$(birefnet)" --file "image=$dog" --on local)
[ -n "$a" ] || { check "lines: A (BiRefNet) was never a run: $(tail -c 300 "$OUT/lines-a.err")"; return; }
# B goes once the machine took A, so A is first in every line.
for _ in $(seq 600); do cozy --json run watch "$a" --once 2> /dev/null | jq -e '.accepted_at_ms != null' > /dev/null && break; sleep 0.2; done
b=$(run_submit "$OUT" lines-b run "$(qwen)" "prompt=Say one word. $RANDOM" max_tokens:=4 --model model=qwen/qwen3.5-0.8b --on local)
[ -n "$b" ] || check "lines: B (Qwen3.5) was never a run: $(tail -c 300 "$OUT/lines-b.err")"
run_wait "$OUT" lines-a || check "lines: A (run $a) did not end: $(tail -c 300 "$OUT/lines-a.ndjson")"
[ -z "$b" ] || run_wait "$OUT" lines-b || check "lines: B (run $b) did not end: $(tail -c 300 "$OUT/lines-b.ndjson")"
echo "lines: A run $a $(run_get "$OUT" lines-a .status), B run ${b:-none} $(run_get "$OUT" lines-b .status 2> /dev/null)"
cozy machine logs local --since 2h > "$OUT/lines.tensord.log" || check "lines: machine logs local failed"
python3 "$here/lines.py" "$OUT" "$began" "$OUT/lines.tensord.log" > "$OUT/lines.json" 2> "$OUT/lines.failed" || true
cat "$OUT/lines.json"
while read -r why; do check "lines: $why"; done < "$OUT/lines.failed"
}
