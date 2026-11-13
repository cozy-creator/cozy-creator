# cl-967 (Paul 11-12): Calcifer plans with the shared estimator from the state its machines push.

# estimate_line: a job's weights wait behind the download ahead of it on its machine (one download at a time, in job order), and
# its quote says so. BiRefNet runs once, so its time here is measured; its weights go and the machine restarts, so nothing holds
# them. Qwen3.5 0.8B's weights (1.6 GiB, `cozy model install`) come once, so the machine has measured its link, and go. They come
# again, and while they do BiRefNet is quoted and sent on another picture (the first's would be answered from its memo): its quote
# counts what Qwen has left, its own weights after them, and its measured time, and it finishes within 20% (or 15 s) of it.
estimate_line() {
[ "$RUNNER_OS" = Linux ] || return 0
step "estimate line (cl-967): a job's download waits behind the one ahead of it, and its quote says so"
local dog other target model n left asked quoted finished took off bound
# BiRefNet as its isolated Hub publishes it: run-estimates quotes published code, as the page asks.
dog=$(picture dog2.jpg); other=$(picture dog1.jpg); target=birefnet/remove_background
run_cell "$OUT" line-first "$target" --file "image=$dog" --on local || { check "estimate line: the first BiRefNet run: $(run_get "$OUT" line-first .reason.message)"; return; }
cozy --json machine show local > "$OUT/line-models.json" || { check "estimate line: machine show local"; return; }
model=$(jq -r '[.status.models[]?.repository] | map(select(test("birefnet"; "i"))) | first // empty' "$OUT/line-models.json")
[ -n "$model" ] || { check "estimate line: BiRefNet's weights aren't listed: $(jq -c '[.status.models[]?.repository]' "$OUT/line-models.json")"; return; }
cozy --json model uninstall "$model" > "$OUT/line-uninstall.json" 2>&1 || { check "estimate line: model uninstall $model: $(tail -c 300 "$OUT/line-uninstall.json")"; return; }
machine_stop || { check "estimate line: the local machine did not stop"; return; }
machine_ready
run_submit "$OUT" line-link model install qwen/qwen3.5-0.8b --on local > /dev/null
run_wait "$OUT" line-link && [ "$(run_get "$OUT" line-link .status)" = completed ] || { check "estimate line: Qwen's first install: $(run_get "$OUT" line-link .reason.message)"; return; }
cozy --json model uninstall qwen/qwen3.5-0.8b > "$OUT/line-uninstall-qwen.json" 2>&1 || { check "estimate line: model uninstall qwen: $(tail -c 300 "$OUT/line-uninstall-qwen.json")"; return; }

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
python3 "$here/plan.py" "$target" --on local --project "$RUN_PROJECT" --file "image=$other" > "$OUT/line-quote.json" 2> "$OUT/line-quote.err" \
  || { check "estimate line: run-estimates: $(tail -c 300 "$OUT/line-quote.err")"; return; }
run_submit "$OUT" line-after run "$target" --file "image=$other" --on local > /dev/null
run_wait "$OUT" line-qwen || check "estimate line: Qwen's install didn't end"
run_wait "$OUT" line-after || { check "estimate line: BiRefNet behind it didn't end"; return; }
[ "$(run_get "$OUT" line-after .status)" = completed ] || { check "estimate line: BiRefNet behind it $(run_get "$OUT" line-after .status): $(run_get "$OUT" line-after .reason.message)"; return; }
J '.memo.memoized != true' "$OUT/line-after.json" || { check "estimate line: BiRefNet behind it was answered from its memo: nothing to time"; return; }
quoted=$(jq -r '.machines[] | select(.machine == "local" and .job == 0) | .finish_ms // empty' "$OUT/line-quote.json")
[ -n "$quoted" ] || { check "estimate line: local quoted no finish: $(head -c 400 "$OUT/line-quote.json")"; return; }
finished=$(run_get "$OUT" line-after '.settled_at_ms')
took=$((finished - asked)) off=$((finished - asked - quoted))
bound=$((took / 5 > 15000 ? took / 5 : 15000))
echo "estimate line: Qwen had $((left >> 20)) MiB to come; BiRefNet quoted to finish in ${quoted} ms ($(jq -c '.machines[0] | {prepare_ms, start_ms, basis}' "$OUT/line-quote.json")), finished in ${took} ms; off by ${off} ms (bound ${bound})"
[ "${off#-}" -le "$bound" ] || check "estimate line: BiRefNet finished ${took} ms after its quote, which said ${quoted} ms: off by more than ${bound} ms"
}

# plan_machine <asset>: the machine its job on the board went to.
plan_machine() { status "$P" | jq -r --arg a "$1" '[.board.jobs[]? | select(.asset == $a) | .machine // empty] | last // empty'; }

# plan_picks (phase b): this computer's machine and a stand-in (another user's cozy on this runner) both ran the weighted tool, so its
# time is measured on each; then stand-w's copy of BiRefNet's weights, which it reads, goes. A render that may go to either goes to
# local, where nothing downloads: on a tie a rental would go first, so only the download's time sends it there. (A machine busy for
# longer than a download is the GPU proof's: on a runner's CPU, runs go side by side.)
plan_picks() {
step "plan (cl-967): a job goes where its weights are"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "plan: its stand-in is another user of a fresh GitHub runner"; return; }
local P=$OUT/plan tool=$here/project/weighted.py/weighted r=$RANDOM went
cozy --json machine show local > "$OUT/set-local.json" || { check "plan: machine show local"; return; }
stand_in stand-w || return
cozy --json new --unencrypted "$P" > "$OUT/plan-new.json" 2>&1 || { check "plan: new: $(tail -c 300 "$OUT/plan-new.json")"; return; }
jq -n --arg t "$tool" --arg r "$r" '[
  {name: "warm_w", tool: $t, kind: "image", fields: {instructions: "Weights onto stand-w, \($r).", seconds: 5}},
  {name: "warm_l", tool: $t, kind: "image", fields: {instructions: "Its time on local, \($r).", seconds: 5}},
  {name: "near", tool: $t, kind: "image", fields: {instructions: "Where its weights are, \($r).", seconds: 5}}]' | boxes_in "$P" || return
coming_go warm_w '["stand-w"]'
coming_done warm_w 900
coming_go warm_l '["local"]'
coming_done warm_l 900
# stand-w's copy goes (its user's cozy, whose machine it is): a job there downloads BiRefNet again.
stand_as stand-w --json model uninstall zhengpeng7/birefnet > "$OUT/plan-uninstall.json" || { check "plan: stand-w's model uninstall: $(tail -c 300 "$OUT/stand-w.err")"; return; }
in_project "$P" --json machine use "local,stand-w" > "$OUT/plan-use.json" 2>&1 || { check "plan: machine use: $(tail -c 300 "$OUT/plan-use.json")"; return; }

coming_go near '["local","stand-w"]'
coming_done near 900
went=$(plan_machine near)
[ "$went" = local ] || check "plan: near, whose weights only local holds, went to ${went:-nowhere}"
}

# plan_published (phase b2): each job of a published tool is planned (GPU session 43352204256: the second and third shots of one
# weren't, and went to the first of their machines saying nothing). BiRefNet, as its isolated Hub publishes it, cuts out three stills:
# twice on stand-w, then on local or stand-w. Calcifer keeps no project's lines in its own log; its test-only diagnostics (warnings,
# run.sh: calcifer_diagnostics) say a job that `isn't planned`, and no cut's key is there.
plan_published() {
step "plan (cl-967): each job of a published tool is planned"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "plan: its stand-in is another user of a fresh GitHub runner"; return; }
local P=$OUT/plan tool=$here/project/weighted.py/weighted r=$RANDOM at on key keys=() log=$HOME/.cozy/calcifer/diagnostics.log
[ -f "$log" ] || { check "plan: Calcifer kept no diagnostics log"; return; }
jq -n --arg t "$tool" --arg r "$r" '[range(1; 4) as $i | (
  {name: "still_\($i)", tool: $t, kind: "image", fields: {instructions: "Still \($i), \($r).", seconds: 0}},
  {name: "cut_\($i)", tool: "birefnet/remove_background", kind: "image", fields: {image: "<still_\($i)>"}})]' | boxes_in "$P" || return
for at in 1 2 3; do
  on='["stand-w"]'; [ "$at" != 3 ] || on='["local","stand-w"]'
  coming_go "cut_$at" "$on"
  coming_done "cut_$at" 900
  key=$(status "$P" | jq -r --arg a "cut_$at" '[.board.jobs[]? | select(.asset == $a) | .id // empty] | last // empty')
  [ -n "$key" ] || { check "plan: cut_$at has no job on the board"; return; }
  keys+=("$key")
done
cp "$log" "$OUT/plan-published.log"
grep -E "isn't planned: " "$OUT/plan-published.log" | sed 's/^/plan: /'
for at in 1 2 3; do
  key=${keys[$((at - 1))]}
  ! grep -qF "$key isn't planned: " "$OUT/plan-published.log" || check "plan: cut_$at wasn't planned: $(grep -F "$key isn't planned: " "$OUT/plan-published.log" | head -1)"
done
}

# prepare_ahead (phase c): BiRefNet, as its Hub publishes it, on a still that takes 4 min, can't start before it: unstarted work,
# planned where BiRefNet's weights are. stand-v, a new stand-in that holds nothing, has nothing planned and runs nothing, is told to
# prepare its function with no job (`PUT /v1/prepare`): it installs BiRefNet's environment and holds its weights while that box
# still waits.
prepare_ahead() {
step "prepare (cl-967): a new idle machine downloads what work waiting elsewhere needs, with no job"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "prepare: its stand-in is another user of a fresh GitHub runner"; return; }
local P=$OUT/plan wait=$here/project/waits.py/wait r=$RANDOM held='' state=''
stand_in stand-v || return
jq -n --arg w "$wait" --arg r "$r" '[
  {name: "source", tool: $w, kind: "image", fields: {instructions: "A still that takes 4 min, \($r).", megapixels: 0.25, seconds: 240}},
  {name: "after", tool: "birefnet/remove_background", kind: "image", fields: {image: "<source>"}}]' | boxes_in "$P" || return
# The still on stand-w, so stand-v runs nothing; BiRefNet after it may go to any of the three.
coming_go source '["stand-w"]'
coming_go after '["local","stand-w","stand-v"]'
for _ in $(seq 300); do
  cozy --json machine show stand-v > "$OUT/stand-v.json" 2> /dev/null || true
  held=$(jq -r '[.status.models[]? | select(.repository == "zhengpeng7/birefnet")] | length' "$OUT/stand-v.json" 2> /dev/null || echo 0)
  state=$(status "$P" | jq -r '[.board.jobs[]? | select(.asset == "after") | .state] | last // empty')
  [ "${held:-0}" -gt 0 ] && break
  sleep 1
done
echo "prepare: stand-v holds BiRefNet: ${held:-0}; after was ${state:-unknown} then; stand-v's activity: $(jq -c '.status.activity' "$OUT/stand-v.json" 2> /dev/null)"
[ "${held:-0}" -gt 0 ] || check "prepare: stand-v never downloaded BiRefNet's weights while after waited for source"
case $state in waiting | ready | queued | '') ;; *) check "prepare: after was $state, not waiting, when stand-v had BiRefNet: no prepare was needed" ;; esac
coming_done after 600
timeout 60 "$COZY" --json machine end stand-v --force -y > "$OUT/stand-v-end.json" 2>&1 || true
timeout 60 "$COZY" --json machine end stand-w --force -y > "$OUT/stand-w-end.json" 2>&1 || true
}
