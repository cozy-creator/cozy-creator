# cl-967 (Paul 11-12): Calcifer plans with the shared estimator from the state its machines push.

# estimate_line: a job's weights wait behind the download ahead of it on its machine (one download at a time, in job order), and
# its quote says so. BiRefNet runs once, so its time here is measured; its weights go and the machine restarts, so nothing holds
# them. Then Qwen3.5 0.8B's weights download (`cozy model install`) and, while they come, BiRefNet is quoted and sent: its quote
# counts what Qwen has left, its own weights after them, and its measured time, and it finishes within 20% (or 15 s) of it.
estimate_line() {
[ "$RUNNER_OS" = Linux ] || return 0
step "estimate line (cl-967): a job's download waits behind the one ahead of it, and its quote says so"
local dog target model n left asked quoted finished took off bound
dog=$(picture dog2.jpg); target=$(birefnet)
run_cell "$OUT" line-first "$target" --file "image=$dog" --on local || { check "estimate line: the first BiRefNet run: $(run_get "$OUT" line-first .reason.message)"; return; }
cozy --json machine show local > "$OUT/line-models.json" || { check "estimate line: machine show local"; return; }
model=$(jq -r '[.status.models[]?.repository] | map(select(test("birefnet"; "i"))) | first // empty' "$OUT/line-models.json")
[ -n "$model" ] || { check "estimate line: BiRefNet's weights aren't listed: $(jq -c '[.status.models[]?.repository]' "$OUT/line-models.json")"; return; }
cozy --json model uninstall "$model" > "$OUT/line-uninstall.json" 2>&1 || { check "estimate line: model uninstall $model: $(tail -c 300 "$OUT/line-uninstall.json")"; return; }
machine_stop || { check "estimate line: the local machine did not stop"; return; }
machine_ready

n=$(run_submit "$OUT" line-qwen model install qwen/qwen3.5-0.8b --on local)
[ -n "$n" ] || { check "estimate line: model install qwen/qwen3.5-0.8b was never a run: $(tail -c 300 "$OUT/line-qwen.err")"; return; }
for _ in $(seq 600); do
  run_show "$OUT" line-qwen "$n"
  left=$(jq -r '(.progress.bytes_total // 0) - (.progress.bytes_done // 0)' "$OUT/line-qwen.json" 2> /dev/null || echo 0)
  [ "${left:-0}" -gt $((256 << 20)) ] && break
  sleep 0.2
done
[ "${left:-0}" -gt $((256 << 20)) ] || { check "estimate line: Qwen's download never had 256 MiB left to come: $(jq -c .progress "$OUT/line-qwen.json")"; return; }
asked=$(date +%s%3N)
python3 "$here/plan.py" "$target" --on local --project "$RUN_PROJECT" --file "image=$dog" > "$OUT/line-quote.json" 2> "$OUT/line-quote.err" \
  || { check "estimate line: run-estimates: $(tail -c 300 "$OUT/line-quote.err")"; return; }
run_submit "$OUT" line-after run "$target" --file "image=$dog" --on local > /dev/null
run_wait "$OUT" line-qwen || check "estimate line: Qwen's install didn't end"
run_wait "$OUT" line-after || { check "estimate line: BiRefNet behind it didn't end"; return; }
[ "$(run_get "$OUT" line-after .status)" = completed ] || { check "estimate line: BiRefNet behind it $(run_get "$OUT" line-after .status): $(run_get "$OUT" line-after .reason.message)"; return; }
quoted=$(jq -r '.machines[] | select(.machine == "local" and .job == 0) | .finish_ms // empty' "$OUT/line-quote.json")
[ -n "$quoted" ] || { check "estimate line: local quoted no finish: $(head -c 400 "$OUT/line-quote.json")"; return; }
finished=$(run_get "$OUT" line-after '.settled_at_ms')
took=$((finished - asked)) off=$((finished - asked - quoted))
bound=$((took / 5 > 15000 ? took / 5 : 15000))
echo "estimate line: Qwen had $((left >> 20)) MiB to come; BiRefNet quoted to finish in ${quoted} ms ($(jq -c '.machines[0] | {prepare_ms, start_ms, basis}' "$OUT/line-quote.json")), finished in ${took} ms; off by ${off} ms (bound ${bound})"
[ "${off#-}" -le "$bound" ] || check "estimate line: BiRefNet finished ${took} ms after its quote, which said ${quoted} ms: off by more than ${bound} ms"
}
