# control-plane (cl-1202; the coordinator, 11-18: a status read is a read and writes nothing to the store; no whole-filesystem flush
# under a store lock; reclaim measures an environment once, on events). One rental, two arms on the same pod, each the same Qwen
# Image box guessed and rendered while its ~20 GB of weights download as fast as the pod's link takes them: `fix` on the session's
# TensorD first (it also installs the package), then `base` on `base_cozy`'s, its weights uninstalled between. Per arm:
#   answers  the longest gap between the status reads each second (each `machine show` waits on the pod's Status), the slowest list
#            put as Calcifer timed it, and the render's acceptance after its run was made;
#   threads  the pod's TensorD threads each 0.25 s (tensord-threads.py over SSH): the time they spent in `syncfs`
#            (`wb_wait_for_completion`) and waiting on a file lock (`locks_lock_inode_wait`), and `machine-reclaim`'s CPU.
# The fix arm passes when no status read waited past 3 s, no list put past 2 s, its render was accepted within 5 s and no thread
# was in `syncfs`. The base arm is said, not judged.
# params:
#   tree       the packages_refs checkout the packages run from (required: -f packages_refs=pk=rc/<x>, tree pk)
#   base_cozy  the dev-build run whose cozy is the base TensorD (required)
#   seconds    what the cap must buy (default 900)
. "$(dirname "$0")/lib.sh"
TREE=$(param tree '') BASE_RUN=$(param base_cozy '') BEGAN=$(date +%s)
[[ $BASE_RUN =~ ^[0-9]+$ ]] || fail "base_cozy: a dev-build run id"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build run of the head>"
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen-image-2/pyproject.toml" ] || fail "the packages run from a checkout: -f packages_refs=<name>=<ref>, params.tree"
BASE=${RUNNER_TEMP:-/tmp}/base-cozy
gh run download "$BASE_RUN" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$BASE.d" > /dev/null 2>&1 &&
  install -m 0755 "$(find "$BASE.d" -name cozy-linux-x86_64 | head -1)" "$BASE" || fail "base_cozy $BASE_RUN: no cozy"
BV=$("$BASE" --version | awk '{print $2}')
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
say "a dev home on cozy $V (fix) and base TensorD $BV, its packages from $TREE $(git -C "$SOURCE" rev-parse --short HEAD 2> /dev/null)"
qwen() { jq -nc --argjson seed "$1" '{instructions: "a red lighthouse on a cliff at dusk, waves below, photo", turbo: true,
  prompt_upsampling: false, megapixels: 1, aspect_ratio: "1:1", seed: $seed}'; }
run_page add pier --tool qwen-image-2/image --fields "$(qwen 7)" > "$OUT/box-pier.json" 2>&1 || fail "the Qwen box: $(tail -c 300 "$OUT/box-pier.json")"

quote "$(param seconds 900)"
rent
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.status.phase // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
[ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] || fail "$NAME isn't on the RC's builds in 15 min: $(jq -c .rc "$OUT/show-rc.json")"

pod() { cozy machine ssh "$NAME" -- "$*" < /dev/null 2>> "$OUT/ssh.err"; }
# The pod's Status each second, as Calcifer relays it.
( while :; do
    printf '{"at_ms":%s,"show":%s}\n' "$(date +%s%3N)" "$(cozy machine show "$NAME" --json 2> /dev/null |
      jq -c '{version: .status.version, ahead: [.status.ahead[]? | {id, state, stage: .readiness.stage}]}' 2> /dev/null || echo null)" >> "$OUT/status.ndjson"
    sleep 1
  done ) & BG=$!
pod "mkdir -p /root/cl1202 && echo $(base64 -w0 "$(dirname "$0")/tensord-threads.py") | base64 -d > /root/cl1202/threads.py &&
  (setsid nohup python3 /root/cl1202/threads.py 1500 /root/cl1202/out < /dev/null > /root/cl1202/sampler.log 2>&1 &
   echo \$! > /root/cl1202/pid)" || fail "the sampler didn't start on $NAME"

# arm <label> <cozy binary> <seed> [fresh]: the pod on that TensorD (its executors end with the one before), with `fresh` the
# models it holds uninstalled so the arm downloads them again; Calcifer started again and the box shown with a new seed; its render
# asked the moment the pod lists its guess, and followed to its end.
arm() {
  local label=$1 binary=$2 seed=$3 want t run model
  want=$("$binary" --version | awk '{print $2}')
  if [ "$(cozy machine show "$NAME" --json | jq -r .status.version)" != "$want" ]; then
    step "tensord-$label" machine update "$NAME" --tensord "$binary" -y || fail "$label: the pod refused TensorD $want"
  fi
  [ "$(cozy machine show "$NAME" --json | jq -r .status.version)" = "$want" ] || fail "$label: the pod doesn't run TensorD $want"
  if [ -n "${4:-}" ]; then
    for model in $(cozy machine show "$NAME" --json | jq -r '.status.models[]?.repository'); do
      cozy model uninstall "$model" --on "$NAME" >> "$OUT/uninstall.txt" 2>&1 || say "$label: $model wasn't uninstalled: $(tail -c 200 "$OUT/uninstall.txt")"
    done
    say "$label: the pod's models uninstalled; $(cozy machine show "$NAME" --json | jq -r '.status.models | length') left"
  fi
  python3 "$RUN_PAGE" set "$RUN_PROJECT" pier --fields "$(qwen "$seed")" > "$OUT/set-$label.json" 2>&1 || fail "$label: the seed: $(tail -c 300 "$OUT/set-$label.json")"
  cozy restart > "$OUT/restart-$label.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart-$label.txt")"
  python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/shown-$label.txt" 2>&1 & BG="$BG $!"
  for _ in $(seq 1 60); do grep -q '"held": \["' "$OUT/shown-$label.txt" && break; sleep 1; done
  grep -q '"held": \["' "$OUT/shown-$label.txt" || fail "$label: the project wasn't shown: $(tail -c 300 "$OUT/shown-$label.txt")"
  local a0; a0=$(date +%s%3N)
  for t in $(seq 1 600); do
    cozy machine show "$NAME" --json 2> /dev/null | jq -e '[.status.ahead[]? | select(.id | startswith("cozy:prepare:"))] | length > 0' > /dev/null && break
    [ "$t" -lt 600 ] || fail "$label: the pod listed no guess in 300 s"
    sleep 0.5
  done
  local t0 t1; t0=$(date +%s%3N)
  timeout 120 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" --queue pier > "$OUT/ask-$label.json" 2>&1 || fail "$label: the ask: $(tail -c 300 "$OUT/ask-$label.json")"
  t1=$(date +%s%3N)
  for _ in $(seq 120); do
    (cd "$RUN_PROJECT" && cozy --json run ls --all --limit 20) > "$OUT/runs-$label.json" 2> /dev/null
    run=$(jq -r --argjson s "$t0" '[.runs[]? | select(.created_at_ms >= $s - 1000 and (.target | test("qwen-image-2")) and .kind == "call")]
      | min_by(.created_at_ms) | select(.accepted_at_ms != null) | "\(.number) \(.created_at_ms) \(.accepted_at_ms)"' "$OUT/runs-$label.json" 2> /dev/null)
    [ -z "$run" ] || break
    sleep 1
  done
  [ -n "$run" ] || fail "$label: no call accepted in 120 s"
  local number created accepted; read -r number created accepted <<<"$run"
  (cd "$RUN_PROJECT" && timeout 600 cozy run watch "$number" > "$OUT/watch-$label.txt" 2>&1)
  (cd "$RUN_PROJECT" && cozy --json run watch "$number" --once --full) > "$OUT/run-$label.json" 2>&1
  echo "$label $a0 $(date +%s%3N) $((t1 - t0)) $((accepted - created)) $(jq -r .status "$OUT/run-$label.json" 2> /dev/null)" >> "$OUT/arms"
  say "$label: TensorD $want; the ask answered in $((t1 - t0)) ms, run $number accepted $((accepted - created)) ms after it was made, $(jq -r .status "$OUT/run-$label.json" 2> /dev/null)"
}

arm fix "$(command -v cozy)" 11
arm base "$BASE" 13 fresh
pod 'kill "$(cat /root/cl1202/pid)" 2> /dev/null; tar czf - -C /root/cl1202 out | base64 -w0' | base64 -d > "$OUT/probe.tgz" &&
  mkdir -p "$OUT/probe" && tar xzf "$OUT/probe.tgz" -C "$OUT/probe" || say "the samples weren't fetched"
rm -f "$OUT/probe.tgz"
kill $BG 2> /dev/null; BG=''
cozy machine logs "$NAME" --lines 0 --since "$(( ($(date +%s) - BEGAN) / 60 + 2 ))m" > "$OUT/tensord.log" 2>&1 || say "the pod's TensorD log wasn't read"
cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true

python3 - "$OUT" "$NAME" <<'PY' | tee -a "$R" > "$OUT/compared.json"
import json, re, sys
from datetime import datetime, timezone
out, name = sys.argv[1], sys.argv[2]
ms = lambda line: datetime.fromisoformat(line[:26].rstrip("Z")).replace(tzinfo=timezone.utc).timestamp() * 1000
status = [json.loads(line)["at_ms"] for line in open(f"{out}/status.ndjson") if line.startswith("{")]
puts = [(ms(line), int(m[1])) for line in open(f"{out}/calcifer.log", errors="replace")
        if line[:4].isdigit() and (m := re.search(rf"lists put on {re.escape(name)} in (\d+) ms", line))]
samples = [json.loads(line) for line in open(f"{out}/probe/out/threads.ndjson")] if __import__("os").path.exists(f"{out}/probe/out/threads.ndjson") else []
arms = {}
for line in open(f"{out}/arms"):
    label, a0, a1, asked, accepted, ended = line.split()[:6]
    a0, a1 = int(a0), int(a1)
    at = [t for t in status if a0 <= t <= a1]
    window = [s for s in samples if a0 <= s["t"] * 1000 <= a1]
    waits = lambda wchan: round(sum(0.25 for s in window for row in s["threads"] if row[3] == wchan), 2)
    reclaim = [(s["t"], row[4]) for s in window for row in s["threads"] if row[1] == "machine-reclaim"]
    cpu = round((reclaim[-1][1] - reclaim[0][1]) / 100 / (reclaim[-1][0] - reclaim[0][0]), 2) if len(reclaim) > 1 and reclaim[-1][0] > reclaim[0][0] else None
    arms[label] = {"seconds": round((a1 - a0) / 1000), "status_gap_ms": max((b - a for a, b in zip(at, at[1:])), default=0),
                   "put_ms": max((p for t, p in puts if a0 <= t <= a1), default=0), "ask_ms": int(asked), "accepted_ms": int(accepted),
                   "call": ended, "syncfs_s": waits("wb_wait_for_completion"), "flock_wait_s": waits("locks_lock_inode_wait"),
                   "reclaim_cpu": cpu, "samples": len(window)}
print(json.dumps(arms))
PY
fix() { jq -r --arg k "$1" '.fix[$k]' "$OUT/compared.json"; }
for k in status_gap_ms put_ms accepted_ms syncfs_s flock_wait_s reclaim_cpu; do
  say "$k: fix $(fix "$k"), base $(jq -r --arg k "$k" '.base[$k]' "$OUT/compared.json")"
done
[ "$(fix samples)" -gt 0 ] || { say "FAIL: no thread samples in the fix arm"; RC=1; }
[ "$(fix status_gap_ms)" -le 3000 ] || { say "FAIL: fix: no status read answered for $(fix status_gap_ms) ms"; RC=1; }
[ "$(fix put_ms)" -le 2000 ] || { say "FAIL: fix: a list put took $(fix put_ms) ms"; RC=1; }
[ "$(fix accepted_ms)" -le 5000 ] || { say "FAIL: fix: its render was accepted $(fix accepted_ms) ms after it was made"; RC=1; }
[ "$(fix syncfs_s)" = 0 ] || { say "FAIL: fix: TensorD's threads spent $(fix syncfs_s) s in syncfs"; RC=1; }
[ "$(fix call)" = completed ] || { say "FAIL: fix: its render $(fix call)"; RC=1; }
[ "$RC" = 0 ] && say "PASS: on the fix, TensorD answered throughout the download and no thread waited in syncfs"
