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
# $updating, until `machine show` and `run ls` say it waits for run $n.
update_waiting() {
  (cozy --json machine update local --gpu-memory-limit off -y > "$OUT/$1.update.json" 2>&1; echo $? > "$OUT/$1.update.code") &
  updating=$!
  local said="waiting for run $n to end"
  for _ in $(seq 60); do
    cozy --json machine show local > "$OUT/$1.machine.json" 2> /dev/null || true
    J --arg w "$said" '.status.update == $w' "$OUT/$1.machine.json" && break
    sleep 1
  done
  J --arg w "$said" '.status.update == $w' "$OUT/$1.machine.json" \
    || check "$1: machine show local never said \"$said\": $(jq -c .status.update "$OUT/$1.machine.json")"
  cozy --json run ls --all --on local > "$OUT/$1.list.json" || true
  J --arg w "$said" 'any(.runs[]; .kind == "update" and ((.waiting // "") | contains($w)))' "$OUT/$1.list.json" \
    || check "$1: run ls shows no update $said: $(jq -c '[.runs[] | select(.kind == "update") | .waiting]' "$OUT/$1.list.json")"
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

step "a machine from before the one store moves its held outputs into the TensorFS store at its next start (cl-843)"
# What such a machine kept beside its journal: an output a hold keeps, an object nothing holds, an upload under way.
local E=$HOME/.cozy/machine/var/lib/cozy/tensord store=${TENSORFS_HOME:-$HOME/.cache/tensorfs} kept loose old
old=$E/execution/objects
printf 'a held output %s' "$RANDOM" > "$OUT/upgrade.kept"
printf 'nothing holds this %s' "$RANDOM" > "$OUT/upgrade.loose"
kept=$(sha256sum "$OUT/upgrade.kept" | cut -c1-64) loose=$(sha256sum "$OUT/upgrade.loose" | cut -c1-64)
mkdir -p "$old/holds" "$old/${kept:0:2}" "$old/${loose:0:2}" "$E/writes/e2e"
cp "$OUT/upgrade.kept" "$old/${kept:0:2}/$kept"
cp "$OUT/upgrade.loose" "$old/${loose:0:2}/$loose"
echo part > "$E/writes/e2e/$loose.part"
jq -n --arg s "$kept" --argjson n "$(wc -c < "$OUT/upgrade.kept")" '{owner: "e2e-upgrade", roots: [{sha256: $s, length: $n}]}' > "$old/holds/tree-e2e.json"
cozy --json machine update local --gpu-memory-limit off -y > "$OUT/upgrade.update.json" 2>&1 || check "upgrade: the restart failed: $(tail -c 300 "$OUT/upgrade.update.json")"
machine_ready
cmp -s "$OUT/upgrade.kept" "$store/blobs/${kept:0:2}/${kept:2:2}/$kept" || check "upgrade: the held output is not in the store: $(ls "$old" 2>&1 | head -5)"
grep -qx "$kept" "$store/uses/$(printf e2e-upgrade | sha256sum | cut -c1-64)" || check "upgrade: no use keeps the held output"
[ ! -e "$store/blobs/${loose:0:2}/${loose:2:2}/$loose" ] || check "upgrade: an object nothing held was moved"
{ [ ! -e "$old" ] && [ ! -e "$E/writes" ]; } || check "upgrade: the old folders remain: $(ls -d "$old" "$E/writes" 2> /dev/null | tr '\n' ' ')"
[ "$(ls -ld "$store" | cut -c1-10)" = drwx------ ] || check "upgrade: the store is not its owner's alone: $(ls -ld "$store")"
# A second start finds nothing to move, and what it moved is still there.
cozy --json machine update local --gpu-memory-limit off -y > "$OUT/upgrade.again.json" 2>&1 || check "upgrade: the second restart failed: $(tail -c 300 "$OUT/upgrade.again.json")"
machine_ready
cmp -s "$OUT/upgrade.kept" "$store/blobs/${kept:0:2}/${kept:2:2}/$kept" || check "upgrade: the held output did not outlive the next start"
}

# kept_install: the id of kept-gate's install once its build holds it (`installing local/kept-gate`), said each doubling wait
# (attempt k/5, about 8 min in all: the machine's PyTorch layer may come first); nothing when it never does.
kept_install() {
  local k at=15 start=$SECONDS id
  for k in 1 2 3 4 5; do
    while [ $((SECONDS - start)) -lt "$at" ]; do
      id=$(cozy --json run ls --all --limit 20 2> /dev/null | jq -r 'first(.runs[]? | select(.target == "install local/kept-gate"
        and ("\(.progress.stage // "") \(.waiting // "")" | contains("installing local/kept-gate"))) | .id) // empty')
      [ -n "$id" ] && { echo "$id"; return 0; }
      kill -0 "$(cat "$OUT/kept.pid")" 2> /dev/null || return 1
      sleep 2
    done
    echo "kept: its install does not hold yet after $((SECONDS - start)) s (attempt $k/5)" >&2
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
# said <log> <run id> <words>: whether a line of the log names the run and says the words.
said() { [ -n "$2" ] && awk -v id="$2" -v words="$3" 'index($0, id) && index($0, words) { found = 1 } END { exit !found }' "$1"; }

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
  check "kept: its install never held in its preparation: $(tail -n 3 "$OUT/kept.err" | tr '\n' ' ')"
else
  machine_stop || check "kept: the local machine did not stop"
  touch "$gate"
  machine_ready
  n=$(kept_called) || n=
  echo "${n:-none}" > "$OUT/kept.number"
  run_wait "$OUT" kept || true
  J '.status == "completed"' "$OUT/kept.json" || check "kept: run ${n:-none} is $(jq -c '[.status, .reason]' "$OUT/kept.json" 2> /dev/null), not completed"
  grep -q '"words": *7\b' "$OUT/kept.ndjson" || check "kept: run ${n:-none}'s reply is not its own code's"
  cozy machine logs local --lines 5000 > "$OUT/kept.tensord.log" || check "kept: machine logs local failed"
  said "$OUT/kept.tensord.log" "$install" needs_access || check "kept: the restarted machine did not keep install $install as needs_access"
  cozy machine logs local calcifer --lines 5000 > "$OUT/kept.calcifer.log" || check "kept: machine logs local calcifer failed"
  said "$OUT/kept.calcifer.log" "$install" "its access sent again" || check "kept: Calcifer did not send install $install's access again"
  echo "kept: install $install was kept across the restart as needs_access, took its access again and completed; run ${n:-none} completed"
fi
trap - EXIT
kept_restore "$gate" "$config" "$OUT/kept/config.toml"
}

# Two downloads on one machine, the second's weights behind the first's: it says whose download it waits for and how much is
# ahead, and doesn't read stalled while that one moves (cl-849). The first is 1.6 GiB, the second 115 MiB, used nowhere else.
queued_download() {
[ "$RUNNER_OS" = Linux ] || return 0
step "a download queued behind another run's says whose, and how much is ahead (cl-849)"
local first second n1='' n2='' said='' behind
cozy --json model install qwen/qwen3.5-0.8b --on local > "$OUT/queued-first.ndjson" 2>&1 &
first=$!
# The second goes once the first's bytes move: its objects queue behind all of the first's.
for _ in $(seq 240); do grep -qE '"bytes_done": *[1-9]' "$OUT/queued-first.ndjson" && break; sleep 0.25; done
cozy --json model install lilylilith/fontbox --on local > "$OUT/queued-second.ndjson" 2>&1 &
second=$!
for _ in $(seq 80); do
  n1=$(grep -oE '"number": *[0-9]+' "$OUT/queued-first.ndjson" | head -1 | grep -oE '[0-9]+$') || true
  n2=$(grep -oE '"number": *[0-9]+' "$OUT/queued-second.ndjson" | head -1 | grep -oE '[0-9]+$') || true
  [ -n "$n1" ] && [ -n "$n2" ] && break
  sleep 0.25
done
behind="waiting for run $n1's download ("
for _ in $(seq 240); do
  cozy --json run watch "${n2:-0}" --once > "$OUT/queued-show.json" 2> /dev/null || true
  said=$(jq -r '.waiting // empty' "$OUT/queued-show.json" 2> /dev/null) || said=
  [[ $said == "$behind"*" ahead)" ]] && break
  J '.status == "completed"' "$OUT/queued-show.json" 2> /dev/null && break
  sleep 0.25
done
echo "queued: run ${n2:-?} said: ${said:-nothing}"
[[ $said == "$behind"*" ahead)" ]] || check "queued: run ${n2:-?} never said it waits for run ${n1:-?}'s download: ${said:-nothing}"
J '.stalled == null' "$OUT/queued-show.json" || check "queued: run ${n2:-?} reads stalled while it waits"

# A machine's jobs are its function runs, its GPUs' work (Paul 11-11, cl-889): beside the downloads, local counts the one
# run, in `machine show`, `machine ls` and the page's list alike (they read one count). `wait` runs no model: a function's run.
step "a download beside a function's run counts one job (cl-889)"
local call busy='' live jobs ls
(cd "$RUN_PROJECT" && cozy --json run "$here/weightless/wait" seconds:=20 --on local > "$OUT/busy-call.ndjson" 2>&1) & call=$!
for _ in $(seq 160); do
  cozy --json run ls --all --limit 20 > "$OUT/busy-runs.json" 2> /dev/null || true
  cozy --json machine show local > "$OUT/busy-local.json" 2> /dev/null || true
  live=$(jq -c '[.runs[] | select(.machine == "local" and (.status == "queued" or .status == "in_progress")) | .kind] | group_by(.) | map({(.[0]): length}) | add // {}' "$OUT/busy-runs.json" 2> /dev/null || echo '{}')
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
}
