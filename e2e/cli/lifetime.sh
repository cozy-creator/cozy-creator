# e2e/cli/lifetime.sh: how a run ends now that nothing judges its progress (cl-663, design stalls.md), the part
# cli/run.sh calls `lifetime`, with the local machine up. A run silent in a native wait far past the old 30 s floor
# completes, and `run watch` says how long it has made no progress; a cancel ends a silent run at once (its executor is
# killed); an update waits for the runs in flight, says which, holds new runs behind it, and goes on once a halted run
# is canceled. A package can no longer fork (the contract's `process` rule), so the executor's dying under a child that
# holds its channel is the release gate's to prove, with a Runtime-owned child.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
J() { jq -e "$@" > /dev/null; }

# launch <name> <entrypoint> [inputs...]: the run in the background, followed into $OUT/<name>.ndjson; its follower's
# pid in $begun, its number in $n once its executor names its stage (silent's "waiting silently").
launch() {
  local name=$1 entry=$2
  shift 2
  (cd "$RUN_PROJECT" && cozy --json run "$here/weightless/$entry" "$@" --on local > "$OUT/$name.ndjson" 2>&1) &
  begun=$! n=''
  for _ in $(seq 180); do grep -qE '"stage": *"waiting silently"' "$OUT/$name.ndjson" && break; sleep 1; done
  grep -qE '"stage": *"waiting silently"' "$OUT/$name.ndjson" || { check "$name: its executor never began"; return 1; }
  n=$(grep -oE '"number": *[0-9]+' "$OUT/$name.ndjson" | head -1 | grep -oE '[0-9]+$')
}
# settled <name> <seconds>: run $n's record once it ended, polled at most that long (a hang fails the step, never the
# suite), into $OUT/<name>.show.json; its follower is stopped.
settled() {
  for _ in $(seq "$2"); do
    cozy --json run watch "$n" --once > "$OUT/$1.show.json" 2> /dev/null || true
    J '.status | IN("completed", "failed", "canceled")' "$OUT/$1.show.json" && break
    sleep 1
  done
  kill "$begun" 2> /dev/null || true
  wait "$begun" 2> /dev/null || true
}
# update_waiting <name>: `cozy machine update local --gpu-memory-limit off` (a restart) in the background, its pid in
# $updating, until `machine show` says it waits, on the machine and as its update job (cl-1028: a job, no project's run).
update_waiting() {
  (cozy --json machine update local --gpu-memory-limit off -y > "$OUT/$1.update.json" 2>&1; echo $? > "$OUT/$1.update.code") &
  updating=$!
  # The machine names no project's run (TensorD says only "protected project execution"): a project run is other work.
  local said="waiting for other work to end"
  for _ in $(seq 60); do
    cozy --json machine show local > "$OUT/$1.machine.json" 2> /dev/null || true
    J --arg w "$said" '.status.update == $w' "$OUT/$1.machine.json" && break
    sleep 1
  done
  J --arg w "$said" '.status.update == $w' "$OUT/$1.machine.json" \
    || check "$1: machine show local never said \"$said\": $(jq -c .status.update "$OUT/$1.machine.json")"
  J --arg w "$said" 'any(.jobs[]?; .kind == "update" and ((.waiting // "") | contains($w)))' "$OUT/$1.machine.json" \
    || check "$1: machine show local has no update job $said: $(jq -c '[.jobs[]? | select(.kind == "update") | .waiting]' "$OUT/$1.machine.json")"
}
# update_done <name>: the update's end (at most 5 min), and the machine ready after its restart.
update_done() {
  for _ in $(seq 300); do kill -0 "$updating" 2> /dev/null || break; sleep 1; done
  kill "$updating" 2> /dev/null || true
  wait "$updating" 2> /dev/null || true
  [ "$(cat "$OUT/$1.update.code" 2> /dev/null)" = 0 ] || check "$1: the update did not succeed: $(tail -c 300 "$OUT/$1.update.json")"
  machine_ready
}

lifetime() {
step "a silent run is never judged, and shows how long it has not moved"
if launch silent silent seconds:=70; then
  # Past the old 30 s floor, and past the minute after which run watch says how long the run has not moved.
  for _ in $(seq 70); do
    cozy run watch "$n" --once > "$OUT/silent.still.txt" 2>&1 || true
    grep -q 'no progress for 1m' "$OUT/silent.still.txt" && break
    sleep 1
  done
  grep -q 'no progress for 1m' "$OUT/silent.still.txt" || check "silent: run watch $n does not say how long it has made no progress: $(grep -i progress "$OUT/silent.still.txt")"
  settled silent 120
  J '.status == "completed"' "$OUT/silent.show.json" || check "silent: run $n is $(jq -c '[.status, .reason]' "$OUT/silent.show.json"), not completed"
  echo "silent: run $n completed after 70 s without a word; while it went: $(grep -o 'no progress for [0-9a-z]*' "$OUT/silent.still.txt")"
fi

step "a cancel ends a silent run at once"
if launch cancel-silent silent seconds:=600; then
  t0=$SECONDS
  cozy --json run cancel "$n" > "$OUT/cancel-silent.json" || check "run cancel $n failed"
  settled cancel-silent 120
  took=$((SECONDS - t0))
  J '.status == "canceled"' "$OUT/cancel-silent.show.json" || check "cancel-silent: run $n is $(jq -r .status "$OUT/cancel-silent.show.json"), not canceled"
  # The old rule waited at least its 30 s floor for the call to stop by itself.
  [ "$took" -lt 30 ] || check "cancel-silent: run $n took ${took} s to end after its cancel"
  echo "cancel-silent: run $n canceled ${took} s after its cancel"
  (cd "$here" && cell after-cancel ./weightless/reverse "text=after$RANDOM") || check "no run went after a canceled run's executor was killed"
fi

step "an update waits for a running run, which completes, and a run submitted meanwhile queues behind it"
if launch going silent seconds:=45; then
  update_waiting going
  (cd "$here" && cell behind ./weightless/reverse "text=behind$RANDOM") &
  behind=$!
  settled going 180
  J '.status == "completed"' "$OUT/going.show.json" || check "going: run $n is $(jq -c '[.status, .reason]' "$OUT/going.show.json"), not completed: the update must not end it"
  update_done going
  wait "$behind" || check "behind: the run submitted while the update waited did not complete"
  echo "going: run $n completed, then the update activated; run $(cat "$OUT/behind.number") queued behind it and completed"
fi

step "an update behind a halted run waits, visibly, until the run is canceled"
if launch halted silent seconds:=600; then
  update_waiting halted
  sleep 5
  kill -0 "$updating" 2> /dev/null || check "halted: the update did not wait for run $n"
  cozy --json run cancel "$n" > "$OUT/halted.cancel.json" || check "run cancel $n failed"
  settled halted 60
  J '.status == "canceled"' "$OUT/halted.show.json" || check "halted: run $n is $(jq -r .status "$OUT/halted.show.json"), not canceled"
  update_done halted
  echo "halted: the update said $(jq -c .status.update "$OUT/halted.machine.json") until run $n was canceled, then activated"
fi

step "this machine's TensorFS store is its owner's alone"
local store=${TENSORFS_HOME:-$HOME/.cache/tensorfs}
[ "$(ls -ld "$store" | cut -c1-10)" = drwx------ ] || check "store: not its owner's alone: $(ls -ld "$store")"
}

# kept_install: the id of kept-gate's install once its build holds it (`installing local/kept-gate`), said each doubling wait
# (attempt k/5, about 8 min in all: the machine's PyTorch layer may come first); nothing when it never does.
kept_install() {
  local k at=15 start=$SECONDS id
  for k in 1 2 3 4 5; do
    while [ $((SECONDS - start)) -lt "$at" ]; do
      # Its own project's runs: the install is the project's.
      id=$(cozy --json run ls --limit 20 2> "$OUT/kept-ls.err" | jq -r 'first(.runs[]? | select(.target == "install local/kept-gate"
        and ("\(.progress.stage // "") \(.waiting // "")" | contains("installing local/kept-gate"))) | .id) // empty')
      [ -n "$id" ] && { echo "$id"; return 0; }
      kill -0 "$(cat "$OUT/kept.pid")" 2> /dev/null || return 1
      sleep 2
    done
    echo "kept: its install does not hold yet after $((SECONDS - start)) s (attempt $k/5)$(tail -c 200 "$OUT/kept-ls.err" | tr '\n' ' ')" >&2
    at=$((at * 2 + 15))
  done
  return 1
}

# kept_called: the call's run number once the CLI sends it after the install, said each doubling wait (attempt k/5,
# 10 min in all); nothing when it never comes.
kept_called() {
  local k at=20 start=$SECONDS n
  for k in 1 2 3 4 5; do
    while [ $((SECONDS - start)) -lt "$at" ]; do
      n=$(run_number "$OUT/kept.ndjson")
      [ -n "$n" ] && { echo "$n"; return 0; }
      kill -0 "$(cat "$OUT/kept.pid")" 2> /dev/null || return 1
      sleep 1
    done
    echo "kept: no call sent yet after $((SECONDS - start)) s (attempt $k/5): $(tail -n 1 "$OUT/kept.err")" >&2
    at=$((at * 2 + 20))
  done
  return 1
}

# kept_restore <gate> <config> <saved config>: the gate open, config.toml as it was, and Calcifer started on it.
kept_restore() {
  touch "$1"
  if [ -f "$3" ]; then cp "$3" "$2"; else rm -f "$2"; fi
  systemctl --user restart cozy-calcifer || check "kept: systemctl --user restart cozy-calcifer failed"
}
# kept_preparing: a run accepted with access survives its machine's restart during its preparation (T4). Access stays
# off the machine's disk (a provider token here; a Hub capability alike): the machine keeps the run as needs_access,
# its sender's reattach sends that access again, and the same run prepares on and completes. Before, the machine
# forgot such a run and Calcifer sent it again as a new one; any other sender found it gone (run_not_found).
# Local code installs first, as its own run (`install local/kept-gate`) the CLI follows before it sends the call: its build
# backend holds that run in its preparation (installing) until the cell opens its gate, then the call runs.
kept_preparing() {
[ "$RUNNER_OS" = Linux ] || return 0
step "a run with access survives a machine restart during its preparation"
local dir=$OUT/kept/kept-gate gate=$OUT/kept/open config=$HOME/.cozy/config.toml n id
mkdir -p "$dir"
sed "s/words=len(payload.text.split())/words=7/" "$here/script/words.py" > "$dir/kept_gate.py"
sed "s#@GATE@#$gate#" > "$dir/gate.py" <<'PY'
"""hatchling's build, once the cell opens its gate: the run waits in its preparation until then."""
import pathlib
import time

from hatchling import build as _hatchling
from hatchling.build import build_sdist, get_requires_for_build_editable, get_requires_for_build_sdist, get_requires_for_build_wheel  # noqa: F401


def _gate():
    while not pathlib.Path("@GATE@").exists():
        time.sleep(0.2)


def build_wheel(*args, **kwargs):
    _gate()
    return _hatchling.build_wheel(*args, **kwargs)


def build_editable(*args, **kwargs):
    _gate()
    return _hatchling.build_editable(*args, **kwargs)
PY
cat > "$dir/pyproject.toml" <<'TOML'
[project]
name = "kept-gate"
version = "0.0.0"
requires-python = ">=3.12"
dependencies = ["cozy-runtime>=0.30", "msgspec>=0.19"]

[project.entry-points."cozy.application"]
default = "kept_gate:app"

[build-system]
requires = ["hatchling>=1.25"]
build-backend = "gate"
backend-path = ["."]

[tool.hatch.build.targets.wheel]
only-include = ["kept_gate.py"]
TOML
# Every run Calcifer sends carries the token from its next start: a top-level setting, so it goes before the file's
# first table. config.toml is restored however the cell ends, a `fail` or an error under `set -e` too.
[ -f "$config" ] && cp "$config" "$OUT/kept/config.toml"
trap "kept_restore '$gate' '$config' '$OUT/kept/config.toml'" EXIT
{ echo 'huggingface_token = "hf_e2e_kept_preparing"'; cat "$OUT/kept/config.toml" 2> /dev/null || true; } > "$config"
systemctl --user restart cozy-calcifer || check "kept: systemctl --user restart cozy-calcifer failed"
(cd "$RUN_PROJECT" && "$RUN_CLI" --json run "$dir" "text=kept $RANDOM" --on local) > "$OUT/kept.ndjson" 2> "$OUT/kept.err" &
echo $! > "$OUT/kept.pid"
install=$(kept_install) || install=
if [ -z "$install" ]; then
  check "kept: its install never held in its preparation: $(tail -n 3 "$OUT/kept.err" | tr '\n' ' ') listed: $(cozy --json run ls --limit 5 2>&1 | jq -c '[.runs[]? | {target, stage: .progress.stage, waiting}]' 2>&1 | head -c 400)"
else
  machine_stop || check "kept: the local machine did not stop"
  touch "$gate"
  machine_ready
  n=$(kept_called) || n=
  echo "${n:-none}" > "$OUT/kept.number"
  run_wait "$OUT" kept || true
  J '.status == "completed"' "$OUT/kept.json" || check "kept: run ${n:-none} is $(jq -c '[.status, .reason]' "$OUT/kept.json" 2> /dev/null), not completed"
  grep -q '"words": *7\b' "$OUT/kept.ndjson" || check "kept: run ${n:-none}'s reply is not its own code's"
  # The install's own log, in its project: machine and Calcifer logs hold no project run ids.
  (cd "$RUN_PROJECT" && cozy --full run watch "$install" --once) > "$OUT/kept.install.txt" 2>&1 || check "kept: run watch $install failed: $(tail -n 2 "$OUT/kept.install.txt")"
  # Said only when the restarted machine kept it as needs_access and Calcifer sent its access again (crates/calcifer/src/lib.rs, cl-1084).
  grep -q "restarted while it was preparing this run, so Cozy sent the run's access again" "$OUT/kept.install.txt" \
    || check "kept: the restarted machine did not keep install $install as needs_access, or Calcifer did not send its access again"
  echo "kept: install $install was kept across the restart as needs_access, took its access again and completed; run ${n:-none} completed"
fi
trap - EXIT
kept_restore "$gate" "$config" "$OUT/kept/config.toml"
}

# Two downloads on one machine, the second's weights behind the first's: it says whose download it waits for and how much is
# ahead, and doesn't read stalled while that one moves (cl-849). The first is 1.6 GiB, the second 115 MiB, used nowhere else. The
# isolated Hub's store is slowed to 30 MB/s (slow_store), so the link is what they wait on, as on a pod: unslowed, the machine has
# every one of the first's objects in flight within 2.5 s and starts the second's beside them, whose bytes then move themselves
# and are behind nothing (e2e 44294017297: fontbox fetched from +2.5 s to +9.1 s alongside Qwen).
queued_download() {
[ "$RUNNER_OS" = Linux ] || return 0
step "a download queued behind another run's says whose, and how much is ahead (cl-849)"
local first second id1='' id2='' t1='' said='' behind moved=''
slow_store queued || return 0
cozy --json model install qwen/qwen3.5-0.8b --on local > "$OUT/queued-first.ndjson" 2>&1 &
first=$!
# The second goes once the first's bytes move: its objects queue behind all of the first's. The bytes are the machine's download
# item's, which the run names in `waiting_on` and says none of its own (cl-1205, #3842).
for _ in $(seq 240); do
  cozy --json machine show local > "$OUT/queued-item.json" 2> /dev/null || true
  J '[.status.activity[]? | select(.kind == "download" and .subject == "qwen/qwen3.5-0.8b" and .bytes_done > 0)] | length > 0' "$OUT/queued-item.json" 2> /dev/null && { moved=1; break; }
  sleep 0.25
done
[ -n "$moved" ] || check "queued: qwen/qwen3.5-0.8b's download never moved on local: $(jq -c '[.status.activity[]? | {kind, subject, bytes_done, bytes_total, runs}]' "$OUT/queued-item.json" 2> /dev/null)"
cozy --json model install lilylilith/fontbox --on local > "$OUT/queued-second.ndjson" 2>&1 &
second=$!
# A download is a job of the machine's, no project's run (cl-1028): it has no name, its id names it, and a run says it by what it does.
job_of() { { grep '^{' "$1" 2> /dev/null | jq -r "select(.run.id != null) | .run.$2" 2> /dev/null || true; } | head -1; }
for _ in $(seq 80); do
  id1=$(job_of "$OUT/queued-first.ndjson" id) id2=$(job_of "$OUT/queued-second.ndjson" id) t1=$(job_of "$OUT/queued-first.ndjson" target)
  [ -n "$id1" ] && [ -n "$id2" ] && break
  sleep 0.25
done
behind="waiting for $t1 to finish downloading ("
for _ in $(seq 240); do
  (cd / && cozy --json run watch "${id2:-none}" --once) > "$OUT/queued-show.json" 2> /dev/null || true
  said=$(jq -r '.waiting // empty' "$OUT/queued-show.json" 2> /dev/null) || said=
  [[ $said == "$behind"*" to go)" ]] && break
  J '.status == "completed"' "$OUT/queued-show.json" 2> /dev/null && break
  sleep 0.25
done
echo "queued: job ${id2:-?} said: ${said:-nothing}"
[[ $said == "$behind"*" to go)" ]] || check "queued: job ${id2:-?} never said it waits for $t1's download: ${said:-nothing}"
J '.stalled == null' "$OUT/queued-show.json" || check "queued: job ${id2:-?} reads stalled while it waits"

# A machine's jobs are its function runs, its GPUs' work (Paul 11-11, cl-889): beside the downloads, local counts the one
# run, in `machine show`, `machine ls` and the page's list alike (they read one count). `wait` runs no model: a function's run.
step "a download beside a function's run counts one job (cl-889)"
local call busy='' live jobs ls
(cd "$RUN_PROJECT" && cozy --json run "$here/weightless/wait" seconds:=20 --on local > "$OUT/busy-call.ndjson" 2>&1) & call=$!
for _ in $(seq 160); do
  cozy --json run ls --all --limit 20 > "$OUT/busy-runs.json" 2> /dev/null || true
  cozy --json machine show local > "$OUT/busy-local.json" 2> /dev/null || true
  # The call is the project's run; the downloads are local's jobs (cl-1028).
  live=$(jq -cs '[.[0].runs[]?, .[1].jobs[]? | select(.machine == "local" and (.status == "queued" or .status == "in_progress")) | .kind] | group_by(.) | map({(.[0]): length}) | add // {}' "$OUT/busy-runs.json" "$OUT/busy-local.json" 2> /dev/null || echo '{}')
  jq -e '.call == 1 and (.warm // 0) >= 1' <<< "$live" > /dev/null && { busy=1; break; }
  sleep 0.25
done
jobs=$(jq -c '{running, queued}' "$OUT/busy-local.json" 2> /dev/null || echo null)
ls=$(cozy --json machine ls 2> /dev/null | jq -c '.machines[] | select(.name == "local") | {running, queued}' || echo null)
echo "busy: live runs on local $live; machine show $jobs; machine ls $ls"
[ -n "$busy" ] || check "busy: never a function's run beside a download on local: $live"
J '.running + .queued == 1' <<< "$jobs" || check "busy: a download beside one function's run counts $jobs, not 1 (live: $live)"
J '.running + .queued == 1' <<< "$ls" || check "busy: machine ls counts $ls beside a download, not 1"
wait "$call" || check "busy: the function's run failed: $(tail -c 300 "$OUT/busy-call.ndjson")"
wait "$first" || check "queued: the first download failed: $(tail -c 300 "$OUT/queued-first.ndjson")"
wait "$second" || check "queued: the second download failed: $(tail -c 300 "$OUT/queued-second.ndjson")"
sudo tc qdisc del dev lo root || check "queued: the store's link wasn't restored"
}

# service_killed (cl-1268, the release coordinator 11-18: "the user-facing message on that path becomes one plain line in the product's
# voice saying what happened and what to do"): a run whose machine's own service died under it, as a host's memory limit killed an
# executor and TensorD's service on a runner (e2e 44302237049) and on W-S12's requeued run (pod-session 44338350009). local's service is
# stopped, the run's executor killed, then the service killed; its parent starts it again, and the new one finds the run's executor
# ended. The run fails `machine_stopped` in one plain line naming local, with no custody words; those are in TensorD's log.
service_killed() {
step "a run whose machine's service died under it says so in one line (cl-1268)"
[ "${RUNNER_OS:-Linux}" = Linux ] || { echo "service killed: Linux runners only"; return; }
local service parent executors pid said log=$HOME/.cozy/machine/var/lib/cozy/machine/logs
machine_ready
launch killed silent seconds:=600 || return
parent=$(machine_pid)
service=$(ps -o pid=,stat=,comm= --ppid "$parent" | awk '$3 == "tensord" && $2 !~ /^Z/ { print $1 }' | tail -1)
[ -n "$service" ] || { check "service killed: local's machine (pid $parent) has no live service"; settled killed 5; return; }
kill -STOP "$service"
executors=$(pgrep -f 'cozy_runtime[.]internal[.](executor|trampoline)' || true)
[ -n "$executors" ] || check "service killed: run $n has no executor process to kill"
for pid in $executors; do kill -KILL "$pid" 2> /dev/null || true; done
kill -KILL "$service"
settled killed 180
machine_ready
said=$(jq -r '.reason.message // empty' "$OUT/killed.show.json")
echo "service killed: run $n $(jq -c '[.status, .reason.code]' "$OUT/killed.show.json"): $said"
J '.status == "failed" and .reason.code == "machine_stopped"' "$OUT/killed.show.json" \
  || check "service killed: run $n is $(jq -c '[.status, .reason.code]' "$OUT/killed.show.json"), not failed machine_stopped"
[ "$said" = "local's worker stopped while this run was going, so the run did not finish. Nothing was saved from it; run it again." ] \
  || check "service killed: run $n doesn't say what happened in one plain line: $said"
! grep -qiE 'custody|birth' <<< "$said" || check "service killed: run $n's message has TensorD's custody words: $said"
cat "$log"/tensord.*.log 2> /dev/null | grep -q "owner lost before durable result custody" \
  || check "service killed: TensorD's log doesn't keep how it knew run $n's executor had ended"
}
