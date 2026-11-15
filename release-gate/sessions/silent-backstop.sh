#!/usr/bin/env bash
# silent-backstop (cl-977 D1): the Hub releases a ready pod whose machine shows no sign of life for its 15-minute idle window while
# its provider still runs it, and never a working one. One pod with SSH (`cpu` is enough), its image's own TensorD: today's client.
#   working  a run silent for window + slack (a run is work, so its own idle release waits): its machine checks in and answers
#            throughout, and the Hub never ends it.
#   silent   its TensorD service stopped (SIGSTOP), its container and supervisor running: no check-in, no answer, and it can't
#            release itself. Within window + slack the Hub releases it as `machine_silent`, which it does only once its provider
#            shows the pod gone.
# params: slack_s (default 300)
. "$(dirname "$0")/lib.sh"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
SLACK=$(param slack_s 300) WINDOW=900
quote $((1200 + 2 * (WINDOW + SLACK)))
rent

sh() { cozy machine ssh "$NAME" -- "$*" < /dev/null 2>> "$OUT/ssh.err"; }
# The service: a tensord whose parent is the tensord supervising it.
service() { sh 'for p in $(pgrep -x tensord); do q=$(ps -o ppid= -p "$p" | tr -d " "); [ "$(cat /proc/$q/comm 2>/dev/null)" = tensord ] && echo "$p"; done' | tail -1; }
show() { cozy machine show "$NAME" --json 2> /dev/null < /dev/null; }

# working: a local function that says it waits, then sleeps without a word.
mkdir -p "$OUT/quiet/e2e_quiet"
cat > "$OUT/quiet/pyproject.toml" <<'TOML'
[build-system]
requires = ["uv_build>=0.12.7,<0.13"]
build-backend = "uv_build"

[project]
name = "e2e-quiet"
version = "0.1.0"
requires-python = ">=3.12"
dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]

[project.entry-points."cozy.application"]
default = "e2e_quiet:app"

[tool.uv.build-backend]
module-name = "e2e_quiet"
module-root = ""
TOML
printf '[application]\nobject = "e2e_quiet:app"\n' > "$OUT/quiet/package.toml"
cat > "$OUT/quiet/e2e_quiet/__init__.py" <<'PY'
import time

import msgspec

from cozy_runtime.author import App, Telemetry

app = App()


class Wait(msgspec.Struct, forbid_unknown_fields=True):
    seconds: float


class Waited(msgspec.Struct):
    seconds: float


@app.entrypoint(accelerator=False)
def silent(payload: Wait, tel: Telemetry) -> Waited:
    with tel.stage("waiting silently"):
        time.sleep(payload.seconds)
    return Waited(seconds=payload.seconds)
PY
LONG=$((WINDOW + SLACK))
(cd "$RUN_PROJECT" && cozy --json run "$OUT/quiet/silent" "seconds:=$LONG" --on "$NAME" > "$OUT/working.ndjson" 2> "$OUT/working.err") & QP=$!
for _ in $(seq 900); do grep -qE '"stage": *"waiting silently"' "$OUT/working.ndjson" && break; kill -0 "$QP" 2> /dev/null || break; sleep 1; done
grep -qE '"stage": *"waiting silently"' "$OUT/working.ndjson" || fail "working: the run never began: $(tail -c 300 "$OUT/working.ndjson") $(tail -c 300 "$OUT/working.err")"
N=$(jq -r 'select(.run?.number) | .run.number' "$OUT/working.ndjson" | tail -1)
[ -n "$N" ] || fail "working: the run names no number: $(tail -c 300 "$OUT/working.ndjson")"
T0=$(date +%s)
# The CLI stops following a run that says nothing for 10 min (it keeps running), so the run's own state and its machine's are read:
# the machine ready throughout, and the run completed once its silence ends.
status=''
while [ $(($(date +%s) - T0)) -le $((LONG + 300)) ]; do
  s=$(show | jq -r '.state // empty')
  [ "$s" = ready ] || fail "working: $NAME is ${s:-unlisted} $(($(date +%s) - T0))s into a live run: $(show | jq -c '{state, reason}')"
  status=$(cd "$RUN_PROJECT" && cozy run watch "$N" --once --json --full 2> /dev/null < /dev/null | jq -r '.status // empty')
  case $status in completed) break ;; failed | canceled) fail "working: run $N $status: $(cd "$RUN_PROJECT" && cozy run watch "$N" --once --json --full 2>&1 | tail -c 400)" ;; esac
  sleep 30
done
kill "$QP" 2> /dev/null; wait "$QP" 2> /dev/null
took=$(($(date +%s) - T0))
[ "$status" = completed ] || fail "working: run $N is '${status:-unknown}' ${took}s on, not completed"
[ "$took" -ge $((LONG - 5)) ] || fail "working: run $N completed after ${took}s, not its ${LONG}s"
say "working: run $N silent ${took}s (window ${WINDOW}s) completed; $NAME stayed ready throughout, untouched by its Hub"

pid=$(service)
[ -n "$pid" ] || fail "silent: no TensorD service on the pod: $(sh 'ps -eo pid,ppid,stat,comm' | head -20)"
# One SSH call stops it and reads its state: a later `machine ssh` waits on the machine it just stopped (43756749942).
state=$(sh "kill -STOP $pid && sleep 1 && cut -d' ' -f3 /proc/$pid/stat") || fail "silent: kill -STOP $pid"
T0=$(date +%s)
[ "$state" = T ] || fail "silent: the service $pid is '$state', not stopped"
say "silent: TensorD's service $pid stopped (its container and supervisor run on); the Hub must release it within $((WINDOW + SLACK))s"
until gone "$RID"; do
  [ $(($(date +%s) - T0)) -le $((WINDOW + SLACK)) ] || fail "silent: still there $((WINDOW + SLACK))s after its machine went silent: $(show | jq -c '{state, reason, unreachable}')"
  sleep 30
done
took=$(($(date +%s) - T0))
reason=$(cozy machine show "$RID" --json 2> /dev/null < /dev/null | jq -r '.reason // empty')
[ "$reason" = machine_silent ] || fail "silent: gone ${took}s later, but as '${reason:-no reason}', not machine_silent"
[ "$took" -ge $((WINDOW - 60)) ] || fail "silent: released ${took}s after it went silent, before its ${WINDOW}s window"
say "silent: released as machine_silent ${took}s after its machine went silent; its Hub proved the pod gone at its provider"
