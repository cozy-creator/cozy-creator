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
# time is measured on each; then stand-w's copy of BiRefNet's weights, which it reads, goes, and its machine restarts so no warm executor
# holds them in memory either. A render that may go to either goes to local, where nothing downloads: on a tie a rental would go first, so only the download's time sends it there. (A machine busy for
# longer than a download is the GPU proof's: on a runner's CPU, runs go side by side.)
plan_picks() {
step "plan (cl-967): a job goes where its weights are"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "plan: its stand-in is another user of a fresh GitHub runner"; return; }
local P=$OUT/plan tool=$here/project/weighted.py/weighted r=$RANDOM went stand_holder stand_pid
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
# stand-w's copy goes (its user's cozy, whose machine it is): a job there downloads BiRefNet again. Its machine stops first, as
# machine_stop stops local, and its user's cozy starts it again: warm_w's executor would still hold BiRefNet, in memory and on its disk,
# so the uninstall would keep its bytes (rc-e2e 43807131201: near quoted 5 s on both, a tie that isn't local's). near waits until this
# computer sees stand-w holding BiRefNet nowhere and missing its bytes.
stand_holder=$(sudo find "/home/cozy-stand-w/.cozy/machine" -name agent.lock.holder 2> /dev/null | head -1)
stand_pid=$([ -z "$stand_holder" ] || sudo grep -o '"pid": *[0-9]*' "$stand_holder" | grep -o '[0-9]*$' || true)
[ -n "$stand_pid" ] || { check "plan: stand-w's machine has no lock holder"; return; }
sudo kill "$stand_pid"
for _ in $(seq 60); do case $(ps -o stat= -p "$stand_pid" 2> /dev/null || true) in '' | Z*) break ;; esac; sleep 1; done
stand_as stand-w --json machine show local > "$OUT/plan-stand-w-again.json" || { check "plan: stand-w didn't start again: $(tail -c 300 "$OUT/stand-w.err")"; return; }
stand_as stand-w --json model uninstall zhengpeng7/birefnet > "$OUT/plan-uninstall.json" || { check "plan: stand-w's model uninstall: $(tail -c 300 "$OUT/stand-w.err")"; return; }
for _ in $(seq 120); do
  cozy --json machine show stand-w > "$OUT/plan-stand-w.json" 2> /dev/null || true
  J '.state == "ready" and ([.status.resident[]?.checkpoints[]?] | length) == 0
     and ([.status.models[]? | select(.repository == "zhengpeng7/birefnet")] | length) == 0
     and ([.status.times[]?.reads[]? | select(.model == "zhengpeng7/birefnet") | .missing] | length == 0 or max > 0)' "$OUT/plan-stand-w.json" && break
  sleep 1
done
echo "plan: stand-w after the uninstall: $(jq -c '{uninstall: input, state, resident: [.status.resident[]?.checkpoints[]?] | length, reads: [.status.times[]?.reads[]? | select(.model == "zhengpeng7/birefnet") | {bytes, missing}]}' "$OUT/plan-stand-w.json" "$OUT/plan-uninstall.json" 2> /dev/null)"
J '.state == "ready"' "$OUT/plan-stand-w.json" || { check "plan: stand-w isn't ready again: $(jq -c '{state, resident: .status.resident}' "$OUT/plan-stand-w.json")"; return; }
in_project "$P" --json machine use "local,stand-w" > "$OUT/plan-use.json" 2>&1 || { check "plan: machine use: $(tail -c 300 "$OUT/plan-use.json")"; return; }

coming_go near '["local","stand-w"]'
coming_done near 900
went=$(plan_machine near)
[ "$went" = local ] || check "plan: near, whose weights only local holds, went to ${went:-nowhere}"
}

# plan_published (phase b2): each job of a published tool is planned, and its run's record says where the plan sent it and what it
# quoted there (`placed`; GPU session 43352204256: the second and third shots of one weren't planned, and went to the first of their
# machines saying nothing). BiRefNet, as its isolated Hub publishes it, cuts out three stills: twice on stand-w, then on local or
# stand-w. Each cut's run is placed (the first two on stand-w), and no job's line in Calcifer's diagnostics says it `isn't planned`.
plan_published() {
step "plan (cl-967): each job of a published tool is planned, and its run says where it went"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "plan: its stand-in is another user of a fresh GitHub runner"; return; }
local P=$OUT/plan tool=$here/project/weighted.py/weighted r=$RANDOM at on key job run placed keys=() log=$HOME/.cozy/calcifer/diagnostics.log
[ -f "$log" ] || { check "plan: Calcifer kept no diagnostics log"; return; }
jq -n --arg t "$tool" --arg r "$r" '[range(1; 4) as $i | (
  {name: "still_\($i)", tool: $t, kind: "image", fields: {instructions: "Still \($i), \($r).", seconds: 0}},
  {name: "cut_\($i)", tool: "birefnet/remove_background", kind: "image", fields: {image: "<still_\($i)>"}})]' | boxes_in "$P" || return
for at in 1 2 3; do
  on='["stand-w"]'; [ "$at" != 3 ] || on='["local","stand-w"]'
  coming_go "cut_$at" "$on"
  coming_done "cut_$at" 900
  job=$(status "$P" | jq -c --arg a "cut_$at" '[.board.jobs[]? | select(.asset == $a)] | last // empty')
  key=$(jq -r '.id // empty' <<< "${job:-null}") run=$(jq -r '.run // empty' <<< "${job:-null}")
  [ -n "$key" ] && [ -n "$run" ] || { check "plan: cut_$at has no job with a run on the board: ${job:-none}"; return; }
  keys+=("$key")
  echo "$P" > "$OUT/plan-cut-$at.at"
  run_show "$OUT" "plan-cut-$at" "$run"
  placed=$(jq -c '.placed // empty' "$OUT/plan-cut-$at.json" 2> /dev/null)
  echo "plan: cut_$at (run $run) placed: ${placed:-none}"
  case $at in
    3) [ -n "$placed" ] || check "plan: cut_3's run $run has no placement on its record" ;;
    *) J '.machine == "stand-w" and .finish_at_ms >= .quoted_at_ms' <<< "${placed:-null}" || check "plan: cut_$at's run $run, on stand-w alone, isn't placed there: ${placed:-none}" ;;
  esac
done
cp "$log" "$OUT/plan-published.log"
{ grep -E "isn't planned: " "$OUT/plan-published.log" || true; } | sed 's/^/plan: /'
for at in 1 2 3; do
  key=${keys[$((at - 1))]}
  ! grep -qF "$key isn't planned: " "$OUT/plan-published.log" || check "plan: cut_$at wasn't planned: $(grep -F "$key isn't planned: " "$OUT/plan-published.log" | head -1)"
done
}

# plan_fetch (cl-1071; Paul 11-16, a 65 GiB quote where 22 landed: "why would it take 15 mins to download?"): a run's quoted download is
# what its machine fetches. composed.py builds a transformer and a VAE, and a third model on demand that it never reads; its box takes
# e2e/composed-tune (its own transformer on e2e/composed-base's VAE) for the transformer. On stand-c, which holds none of them, the run
# is quoted the tune's transformer and the base's VAE once (64 MiB) to fetch before it starts, not each named checkpoint whole and the
# on-demand one (160 MiB); and that is what stand-c then holds of the two, within 5%. The on-demand model, which a quote can't know a
# run reads, is quoted besides ("plus up to" its 32 MiB, `on_demand_bytes`); its machine fetches it beside the run, and it is that,
# within 5%: the gap measured, never silent.
# plan_fetch_held: stand-c's own listing of what it holds; and how much of `$1` it holds, `$2` the other model whose parts it shares
# (`total_bytes` of one, `unique_bytes` of the other: their objects once).
plan_fetch_held() {
  stand_as stand-c --json machine show local > "$OUT/plan-fetch-held.json" 2>> "$OUT/stand-c.err" || true
  jq -r --arg a "$1" --arg b "${2:-}" '[.status.models[]? | select(.repository == $a) | .total_bytes] + [.status.models[]? | select(.repository == $b) | .unique_bytes]
    | add // 0' "$OUT/plan-fetch-held.json" 2> /dev/null || echo 0
}

plan_fetch() {
step "plan (cl-1071): a run's quoted download is what its machine fetches"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "plan: its stand-in is another user of a fresh GitHub runner"; return; }
local P=$OUT/plan-fetch tool=$here/project/composed.py/composed r=$RANDOM job run placed quoted demand landed extra=0
cozy --json machine show local > "$OUT/set-local.json" || { check "plan fetch: machine show local"; return; }
stand_in stand-c || return
cozy --json new --unencrypted "$P" > "$OUT/plan-fetch-new.json" 2>&1 || { check "plan fetch: new: $(tail -c 300 "$OUT/plan-fetch-new.json")"; return; }
jq -n --arg t "$tool" --arg r "$r" '[{name: "fetched", tool: $t, kind: "image",
  fields: {instructions: "A tune on a base, \($r).", models: {model: {ref: "e2e/composed-tune@0.1.0"}}}}]' | boxes_in "$P" || return
coming_go fetched '["stand-c"]'
coming_done fetched 900
job=$(status "$P" | jq -c '[.board.jobs[]? | select(.asset == "fetched")] | last // empty')
run=$(jq -r '.run // empty' <<< "${job:-null}")
[ -n "$run" ] && J '.state == "done"' <<< "$job" || { check "plan fetch: its run didn't end done: ${job:-no job}"; return; }
echo "$P" > "$OUT/plan-fetch.at"
run_show "$OUT" plan-fetch "$run"
placed=$(jq -c '.placed // empty' "$OUT/plan-fetch.json" 2> /dev/null)
quoted=$(jq -r '.fetch_bytes // 0' <<< "${placed:-null}") demand=$(jq -r '.on_demand_bytes // 0' <<< "${placed:-null}")
landed=$(plan_fetch_held e2e/composed-tune e2e/composed-base)
# The on-demand model comes beside the run, after it may have ended.
for _ in $(seq 120); do
  extra=$(plan_fetch_held e2e/composed-extra)
  J '[.status.models[]? | select(.repository == "e2e/composed-extra") | .checkpoints[]?.complete] | any' "$OUT/plan-fetch-held.json" && break
  sleep 1
done
echo "plan fetch: run $run on stand-c quoted $((quoted >> 20)) MiB to fetch, plus up to $((demand >> 20)) MiB on demand ($placed);" \
  "stand-c holds $((landed >> 20)) MiB of the tune and the base, and $((extra >> 20)) MiB of the on-demand model"
echo "plan fetch: stand-c holds $(jq -c '[.status.models[]? | select(.repository | startswith("e2e/composed")) | {repository, total_bytes, unique_bytes, complete: [.checkpoints[]?.complete]}]' "$OUT/plan-fetch-held.json" 2> /dev/null)"
[ "$landed" -gt 0 ] && [ $(( (quoted > landed ? quoted - landed : landed - quoted) * 20 )) -le "$landed" ] \
  || check "plan fetch: quoted $((quoted >> 20)) MiB to fetch before it starts, but stand-c holds $((landed >> 20)) MiB of what it waits for"
[ "$extra" -gt 0 ] && [ $(( (demand > extra ? demand - extra : extra - demand) * 20 )) -le "$extra" ] \
  || check "plan fetch: quoted up to $((demand >> 20)) MiB on demand, but stand-c fetched $((extra >> 20)) MiB of its on-demand model"
}

# plan_update (phase b3), cl-1073 (Paul, 11-16): "I had an idle more powerful machine, with another machine that already had heavy
# load on it, and instead it assigned the work to the old machine rather than the newer one. This is stupid." chartreux's update waited
# for its runs to end and held each run sent after it (LAB2-47846, "getting chartreux ready" for 20 minutes), and placement quoted
# chartreux as if it took them at once. Here stand-w holds BiRefNet's weights and local doesn't, so stand-w finishes a weighted still
# first, until an update of stand-w waits behind a minute's still there: then local, which downloads them, finishes it first.
plan_update() {
step "plan (cl-1073): a machine whose update waits for its runs isn't quoted as free"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "plan update: its stand-in is another user of a fresh GitHub runner"; return; }
local P=$OUT/plan weighted=$here/project/weighted.py/weighted wait=$here/project/waits.py/wait r=$RANDOM updating began went
jq -n --arg w "$weighted" --arg h "$wait" --arg r "$r" '[
  {name: "hold_w", tool: $h, kind: "image", fields: {instructions: "A minute on stand-w, \($r).", megapixels: 0.25, seconds: 60}},
  {name: "past_update", tool: $w, kind: "image", fields: {instructions: "Where it finishes first, \($r).", seconds: 5}}]' | boxes_in "$P" || return
# local's copy of BiRefNet goes, its machine stopped first so no executor holds it: a job there downloads it.
machine_stop || { check "plan update: the local machine did not stop"; return; }
machine_ready
cozy --json model uninstall zhengpeng7/birefnet > "$OUT/update-uninstall.json" 2>&1 || { check "plan update: model uninstall: $(tail -c 300 "$OUT/update-uninstall.json")"; return; }
cozy --json machine show stand-w > "$OUT/update-stand-w-held.json" 2> /dev/null || true
J '[.status.models[]? | select(.repository == "zhengpeng7/birefnet")] | length > 0' "$OUT/update-stand-w-held.json" \
  || { check "plan update: stand-w holds no BiRefNet: $(jq -c '[.status.models[]?.repository]' "$OUT/update-stand-w-held.json")"; return; }
coming_go hold_w '["stand-w"]'
for _ in $(seq 240); do
  cozy --json machine show stand-w > "$OUT/update-stand-w-hold.json" 2> /dev/null || true
  J '[.status.runs[]? | select(.state == "running")] | length > 0' "$OUT/update-stand-w-hold.json" && break
  sleep 0.5
done
J '[.status.runs[]? | select(.state == "running")] | length > 0' "$OUT/update-stand-w-hold.json" \
  || { check "plan update: hold_w never ran on stand-w: $(jq -c '[.status.runs[]? | {state, waiting}]' "$OUT/update-stand-w-hold.json")"; return; }
# stand-w's own cozy updates it (a setting, so a restart): it waits for hold_w to end, and holds what comes after.
(stand_as stand-w --json machine update local --gpu-memory-limit off -y > "$OUT/update-stand-w.json"; echo $? > "$OUT/update-stand-w.code") &
updating=$!
for _ in $(seq 60); do
  cozy --json machine show stand-w > "$OUT/update-stand-w-show.json" 2> /dev/null || true
  J '(.status.update // "") | test("wait")' "$OUT/update-stand-w-show.json" && break
  sleep 1
done
J '(.status.update // "") | test("wait")' "$OUT/update-stand-w-show.json" \
  || check "plan update: stand-w's update never waited for hold_w: $(jq -c '.status.update' "$OUT/update-stand-w-show.json")"
began=$(date +%s)
coming_go past_update '["local","stand-w"]'
coming_done past_update 300
went=$(plan_machine past_update)
echo "plan update: past_update went to ${went:-nowhere}, done in $(($(date +%s) - began)) s; stand-w's update: $(jq -c '.status.update' "$OUT/update-stand-w-show.json")"
[ "$went" = local ] || check "plan update: past_update went to ${went:-nowhere}, whose update waited for its run, not to local, which was free"
# stand-w as the next cell takes it: its update done, ready again.
coming_done hold_w 300
for _ in $(seq 240); do kill -0 "$updating" 2> /dev/null || break; sleep 1; done
[ "$(cat "$OUT/update-stand-w.code" 2> /dev/null)" = 0 ] || check "plan update: stand-w's update: $(tail -c 300 "$OUT/update-stand-w.json")"
for _ in $(seq 180); do
  cozy --json machine show stand-w > "$OUT/update-stand-w-after.json" 2> /dev/null || true
  J '.state == "ready" and (.status.update // "") == ""' "$OUT/update-stand-w-after.json" && break
  sleep 1
done
J '.state == "ready"' "$OUT/update-stand-w-after.json" || check "plan update: stand-w isn't ready after its update: $(jq -c '{state, update: .status.update}' "$OUT/update-stand-w-after.json")"
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

# plan_resent (cl-1116; release blocker, the user flow's 43996197790: a moved H3 take, done on kubera, read as `run_not_found` and was
# lost; Paul: a finished take is never lost). A run sent again, here moved from one machine to another that finishes it sooner
# (cl-1073), is its next attempt there, which its machine knows as `<id>~2`. Every word Calcifer says to the machine about it uses that
# name: its events, its cancel, its outputs' reads. stand-a holds as many runs as it has CPU slots, and stand-b more, so `moved` and
# `canceled` (on both) go to stand-a and wait in its queue. stand-b's runs are canceled: both move to stand-b, now clearly sooner, as
# their attempt 2. `canceled` is canceled there; `moved` completes there, its take lands, and its events say the move and its end.
plan_resent_run() { status "$P" | jq -r --arg a "$1" '[.board.jobs[]? | select(.asset == $a) | .run // empty] | last // empty'; }
plan_resent_show() { project_api "$P" "/runs/$1" 2> /dev/null || echo null; }
plan_resent() {
step "plan (cl-1116): a run moved to another machine is that machine's to cancel and follow, and its take lands"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "resent: its stand-ins are other users of a fresh GitHub runner"; return; }
local P=$OUT/resent tool=$here/project/waits.py/wait r=$RANDOM slots a b name n run moved='' canceled='' watcher end
slots=$(( $(nproc) / 4 )); [ "$slots" -ge 1 ] || slots=1
a=$(seq -s ' ' 1 "$slots") b=$(seq -s ' ' 1 $((slots + 2)))
cozy --json machine show local > "$OUT/set-local.json" || { check "resent: machine show local"; return; }
stand_in stand-a || return
stand_in stand-b || return
cozy --json new --unencrypted "$P" > "$OUT/resent-new.json" 2>&1 || { check "resent: new: $(tail -c 300 "$OUT/resent-new.json")"; return; }
jq -n --arg t "$tool" --arg r "$r" --argjson a "[$(tr ' ' , <<< "$a")]" --argjson b "[$(tr ' ' , <<< "$b")]" '
  [$a[] | {name: "hold_a\(.)", tool: $t, kind: "image", fields: {instructions: "Holds stand-a, \(.), \($r).", megapixels: 0.25, seconds: 590}}]
  + [$b[] | {name: "hold_b\(.)", tool: $t, kind: "image", fields: {instructions: "Holds stand-b, \(.), \($r).", megapixels: 0.25, seconds: 590}}]
  + [{name: "moved", tool: $t, kind: "image", fields: {instructions: "Moves to stand-b and lands, \($r).", megapixels: 0.25, seconds: 1}},
     {name: "canceled", tool: $t, kind: "image", fields: {instructions: "Moves to stand-b and is canceled there, \($r).", megapixels: 0.25, seconds: 590}}]' \
  | boxes_in "$P" || return

# stand-a's CPU slots taken first, then stand-b's and more queued behind them: stand-a is the sooner of the two.
for n in $a; do coming_go "hold_a$n" '["stand-a"]'; done
for _ in $(seq 600); do
  cozy --json machine show stand-a > "$OUT/resent-stand-a.json" 2> /dev/null || true
  J --argjson s "$slots" '[.status.runs[]? | select(.state == "running")] | length >= $s' "$OUT/resent-stand-a.json" && break
  sleep 0.5
done
J --argjson s "$slots" '[.status.runs[]? | select(.state == "running")] | length >= $s' "$OUT/resent-stand-a.json" \
  || { check "resent: stand-a never ran its $slots holds: $(jq -c '[.status.runs[]? | {id, state, waiting}]' "$OUT/resent-stand-a.json")"; return; }
for n in $b; do coming_go "hold_b$n" '["stand-b"]'; done
for _ in $(seq 600); do
  cozy --json machine show stand-b > "$OUT/resent-stand-b.json" 2> /dev/null || true
  J --argjson s "$slots" '[.status.runs[]? | select(.state == "running")] | length >= $s' "$OUT/resent-stand-b.json" && break
  sleep 0.5
done
J --argjson s "$slots" '[.status.runs[]? | select(.state == "running")] | length >= $s' "$OUT/resent-stand-b.json" \
  || { check "resent: stand-b never ran its holds: $(jq -c '[.status.runs[]? | {id, state, waiting}]' "$OUT/resent-stand-b.json")"; return; }
for _ in $(seq 60); do
  cozy --json machine show stand-b > "$OUT/resent-stand-b.json" 2> /dev/null || true
  J --argjson n "$((slots + 2))" '[.status.runs[]?] | length >= $n' "$OUT/resent-stand-b.json" && break
  sleep 0.5
done
echo "resent: stand-a: $(jq -c '[.status.runs[]? | {id, state}]' "$OUT/resent-stand-a.json"); stand-b: $(jq -c '[.status.runs[]? | {id, state}]' "$OUT/resent-stand-b.json")"

coming_go moved '["stand-a","stand-b"]'
coming_go canceled '["stand-a","stand-b"]'
for _ in $(seq 240); do
  moved=$(plan_resent_run moved) canceled=$(plan_resent_run canceled)
  [ -n "$moved" ] && [ -n "$canceled" ] && plan_resent_show "$moved" > "$OUT/resent-moved.json" && plan_resent_show "$canceled" > "$OUT/resent-canceled.json" \
    && J '.machine == "stand-a" and .accepted_at_ms != null' "$OUT/resent-moved.json" && J '.machine == "stand-a" and .accepted_at_ms != null' "$OUT/resent-canceled.json" && break
  sleep 0.5
done
echo "resent: placed: $(jq -c '{name, machine, status, machine_state}' "$OUT/resent-moved.json" "$OUT/resent-canceled.json" 2> /dev/null | paste -sd ' ')"
if ! J '.machine == "stand-a" and .accepted_at_ms != null and .started_at_ms == null' "$OUT/resent-moved.json" \
  || ! J '.machine == "stand-a" and .accepted_at_ms != null and .started_at_ms == null' "$OUT/resent-canceled.json"; then
  check "resent: moved and canceled weren't both waiting in stand-a's queue, so nothing moves: the cell can't tell: $(jq -c '{name, machine, status, machine_state, started_at_ms}' "$OUT/resent-moved.json" "$OUT/resent-canceled.json" 2> /dev/null | paste -sd ' ') $(status "$P" | jq -c '[.board.jobs[]? | {asset, state, machine, why}]')"
else
  name=$(jq -r .name "$OUT/resent-moved.json")
  (in_project "$P" --json run watch "$name" > "$OUT/resent-moved-events.ndjson" 2> "$OUT/resent-moved-events.err") & watcher=$!

  # stand-b's runs end: it is free, and both move there as their attempt 2.
  for n in $b; do
    run=$(plan_resent_run "hold_b$n")
    [ -z "$run" ] || in_project "$P" --json run cancel "$(plan_resent_show "$run" | jq -r .name)" > "$OUT/resent-cancel-b$n.json" 2>&1 || true
  done
  for _ in $(seq 360); do
    plan_resent_show "$moved" > "$OUT/resent-moved.json"
    plan_resent_show "$canceled" > "$OUT/resent-canceled.json"
    J '.machine == "stand-b"' "$OUT/resent-moved.json" && J '.machine == "stand-b" and .accepted_at_ms != null' "$OUT/resent-canceled.json" && break
    sleep 0.5
  done
  echo "resent: after stand-b freed: $(jq -c '{name, machine, status, machine_state, reason}' "$OUT/resent-moved.json" "$OUT/resent-canceled.json" 2> /dev/null | paste -sd ' ')"
  cp "$P/.vault-calcifer/diagnostics/walk.log" "$OUT/resent-walk.log" 2> /dev/null || true
  grep -E ' moves from ' "$OUT/resent-walk.log" 2> /dev/null | tail -n 4 || true
  if ! J '.machine == "stand-b" and .accepted_at_ms != null' "$OUT/resent-canceled.json"; then
    check "resent: canceled never went to stand-b and was accepted there, so its attempt 2 can't be canceled: $(jq -c '{name, machine, status, machine_state, reason}' "$OUT/resent-canceled.json")"
  else
    # Canceled there, by the name stand-b knows it by: stand-b ends it and says so. A cancel of another name is recorded there as a
    # run never received while its attempt 2 runs on, its 590 s, and a machine with no record of it reads as one.
    name=$(jq -r .name "$OUT/resent-canceled.json")
    in_project "$P" --json run cancel "$name" > "$OUT/resent-cancel.json" 2>&1 || check "resent: run cancel $name: $(tail -c 300 "$OUT/resent-cancel.json")"
    for _ in $(seq 120); do
      plan_resent_show "$canceled" > "$OUT/resent-canceled.json"
      J '.status == "canceled"' "$OUT/resent-canceled.json" && break
      sleep 0.5
    done
    echo "resent: canceled: $(jq -c '{name, machine, status, reason}' "$OUT/resent-canceled.json")"
    J '.status == "canceled" and ((.reason.message // "") | test("no record") | not)' "$OUT/resent-canceled.json" \
      || check "resent: canceled, its attempt 2 on stand-b canceled, wasn't ended there within 60 s: $(jq -c '{name, machine, status, machine_state, reason}' "$OUT/resent-canceled.json")"
  fi

  # moved, done on stand-b as its attempt 2: its take lands.
  coming_done moved 300
  plan_resent_show "$moved" > "$OUT/resent-moved.json"
  echo "resent: moved: $(jq -c '{name, machine, status, reason, output: [.output[]? | {name, status, export_error}]}' "$OUT/resent-moved.json")"
  J '.machine == "stand-b"' "$OUT/resent-moved.json" \
    || check "resent: moved never moved to stand-b once stand-b was free: $(jq -c '{name, machine, status}' "$OUT/resent-moved.json") $(tail -n 4 "$OUT/resent-walk.log" 2> /dev/null)"
  J '.status == "completed" and .reason == null and ([.output[]? | select(.export_error != null)] | length == 0)' "$OUT/resent-moved.json" \
    || check "resent: moved ran on stand-b as its attempt 2 but its take was lost: $(jq -c '{status, reason, output: [.output[]? | {name, export_error}]}' "$OUT/resent-moved.json")"
  [ "$(made "$P" moved)" -ge 1 ] || check "resent: moved has no take after its run on stand-b"
  # Its events, followed as `cozy run watch` follows them: the move, then its attempt on stand-b to its end.
  for _ in $(seq 60); do kill -0 "$watcher" 2> /dev/null || break; sleep 0.5; done
  kill "$watcher" 2> /dev/null || true
  wait "$watcher" 2> /dev/null || true
  end=$(grep -n '"run.completed"' "$OUT/resent-moved-events.ndjson" | head -1 | cut -d: -f1)
  grep -q 'moved from stand-a to stand-b' "$OUT/resent-moved-events.ndjson" && [ -n "$end" ] \
    || check "resent: cozy run watch $(jq -r .name "$OUT/resent-moved.json") didn't follow it from its move to its end on stand-b: $(grep -o '"type":"[a-z_.]*"' "$OUT/resent-moved-events.ndjson" | sort | uniq -c | tr -s ' ' | paste -sd ' ') $(tail -c 300 "$OUT/resent-moved-events.err")"
fi
for n in $a; do
  run=$(plan_resent_run "hold_a$n")
  [ -z "$run" ] || in_project "$P" --json run cancel "$(plan_resent_show "$run" | jq -r .name)" > /dev/null 2>&1 || true
done
timeout 60 "$COZY" --json machine end stand-a --force -y > "$OUT/stand-a-end.json" 2>&1 || true
timeout 60 "$COZY" --json machine end stand-b --force -y > "$OUT/stand-b-end.json" 2>&1 || true
}
