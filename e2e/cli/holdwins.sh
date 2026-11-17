# hold_wins (cl-1196). Found 11-17 on a rented RTX 4090, beerus: `cozy machine update beerus --runtime 0.47.0 --tensorfs 0.26.0
# --tensord 0.26.0` held it by hand, and the RC update Calcifer queued at its boot ran anyway: beerus ended on 0.27.0-dev and Runtime
# 0.48.0.dev2319, while `machine ls` and `show` said "held by hand on runtime 0.47.0". The hand's update staged nothing and reached
# beerus first; the RC's, staging the RC's cozy, came after, and beerus ran it next.
# A dev home's rental (`dev = true`), stood in by `cozy machine serve --rental` on this runner against a stood-in Hub
# (rentalself/standin.py) whose lease grants this computer's key, as a Hub grants the devices of the account that rented it; its
# image's pair is the candidates'. Recorded as a rental this computer ordered, it boots, and Calcifer queues its move to the open RC
# (rc-<x>'s prerelease). Then held by hand on the newest releases (PyPI's Runtime and TensorFS, GitHub's Latest cozy): it ends on
# them, the RC update canceled and never run, and `machine show` says it is held on what it runs. Then its hold names another
# Runtime, as beerus's store kept it: `machine show` and `machine ls` say what it runs. Linux, a GitHub runner; its own shard.
here_holdwins=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
HW_DIR='' HW_CFG='' HW_SAVED='' HW_PID='' HW_STANDIN='' HW_PORT='' HW_HUB='' HW_PROVIDER='' HW_CA=''

hold_wins() {
step "a hand update supersedes the RC update queued at a rental's boot, and machine show says what it runs (cl-1196)"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "hold wins: a fresh GitHub Linux runner only"; return 0; }
HW_DIR=$OUT/holdwins HW_CFG=$HOME/.cozy/config.toml HW_SAVED=$OUT/holdwins-config.toml
cp "$HW_CFG" "$HW_SAVED" || { check "hold wins: no config.toml to keep"; return; }
hw_cell
hw_quit
cp "$HW_SAVED" "$HW_CFG"
cozy restart > "$OUT/holdwins-back.txt" 2>&1 || check "hold wins: Calcifer didn't start again off the dev home: $(tail -c 300 "$OUT/holdwins-back.txt")"
}

hw_cell() {
local d=$HW_DIR v runtime tensorfs released key fp code rc held
v=$(cozy --version | awk '{print $2}')
[[ $v == *-dev+g* ]] || { check "hold wins: cozy $v is no RC build, with no RC to follow"; return; }
[ -f "${CAND_RT:-}" ] && [ -f "${CAND_TFS:-}" ] || { check "hold wins: the rental's image pair is the candidate wheels: none (${CAND_RT:-} ${CAND_TFS:-})"; return; }
runtime=$(curl -fsSL --retry 3 https://pypi.org/pypi/cozy-runtime/json | jq -r '.info.version // empty')
tensorfs=$(curl -fsSL --retry 3 https://pypi.org/pypi/tensorfs/json | jq -r '.info.version // empty')
released=$(curl -fsSL --retry 3 https://github.com/cozy-creator/cozy-creator/releases/latest/download/dist-manifest.json \
  | jq -r '.releases[] | select(.app_name == "cozy") | .app_version')
[ -n "$runtime" ] && [ -n "$tensorfs" ] && [ -n "$released" ] \
  || { check "hold wins: the newest releases: Runtime ${runtime:-?}, TensorFS ${tensorfs:-?}, cozy ${released:-?}"; return; }
held="runtime $runtime, tensorfs $tensorfs, tensord $released"
key=$(cozy --json machine show local | jq -r '.controller_key // empty')
[ -n "$key" ] || { check "hold wins: machine show local names no controller key"; return; }

# The stood-in Hub, granting this computer's key; the rental's own cozy (an update replaces the file it runs from) and image pair.
rm -rf "$d" && mkdir -p "$d/home/.cozy" "$d/root/opt/cozy/wheels" "$d/bin" || { check "hold wins: $d not made"; return; }
printf '[machine]\nidle_seconds = 3600\n' > "$d/home/.cozy/config.toml"
printf '%s\n' "$key" > "$d/keys"
install -m 755 "$COZY" "$d/bin/cozy" && cp "$CAND_RT" "$CAND_TFS" "$d/root/opt/cozy/wheels/" || { check "hold wins: the rental's cozy and pair"; return; }
python3 "$here_holdwins/rentalself/standin.py" serve "$d" held > "$d/standin.log" 2>&1 & HW_STANDIN=$!
for _ in $(seq 60); do [ -s "$d/ready" ] && break; kill -0 "$HW_STANDIN" 2> /dev/null || break; sleep 0.5; done
read -r HW_HUB HW_PROVIDER HW_CA < "$d/ready" 2> /dev/null || { check "hold wins: the stand-in Hub didn't start: $(tail -c 400 "$d/standin.log")"; return; }
HW_PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')

# Added once for its address and fingerprint, then recorded as a rental this computer ordered, in a dev home, with it down.
hw_serve
for _ in $(seq 240); do
  openssl s_client -connect "127.0.0.1:$HW_PORT" < /dev/null 2> /dev/null | openssl x509 -outform der > "$d/leaf.der" 2> /dev/null && [ -s "$d/leaf.der" ] && break
  kill -0 "$HW_PID" 2> /dev/null || break
  sleep 0.5
done
[ -s "$d/leaf.der" ] || { check "hold wins: the rental never served on $HW_PORT: $(tail -c 800 "$d/machine.log")"; return; }
fp=$(sha256sum < "$d/leaf.der" | cut -c1-64)
for _ in $(seq 60); do
  code=0; cozy --json machine add stand-h "127.0.0.1:$HW_PORT" --fingerprint "sha256:$fp" > "$d/add.json" 2>&1 || code=$?
  [ "$code" = 0 ] && break
  sleep 1
done
[ "$code" = 0 ] || { check "hold wins: machine add stand-h: $(tail -c 400 "$d/add.json") $(tail -c 400 "$d/machine.log")"; return; }
hw_down
systemctl --user stop cozy-calcifer || { check "hold wins: Calcifer didn't stop"; return; }
hw_store "UPDATE machines SET kind = 'rental', rental_id = 'rental-holdwins', hub = 'http://127.0.0.1:1' WHERE name = 'stand-h'" \
  || { check "hold wins: stand-h wasn't recorded as a rental"; return; }
{ echo 'dev = true'; grep -v '^dev *=' "$HW_SAVED"; } > "$HW_CFG"
hw_start || return

# It boots, and Calcifer queues its move to the RC: the run the hand's supersedes.
hw_serve
rc=''
for _ in $(seq 360); do
  hw_runs "$d/runs-rc.json"
  rc=$(jq -r '[.runs[]? | select(.kind == "update" and (.target | test("\\(RC\\)")))] | first | .name // empty' "$d/runs-rc.json" 2> /dev/null)
  [ -n "$rc" ] && break
  sleep 0.5
done
[ -n "$rc" ] || { check "hold wins: no RC update queued at stand-h's boot in 3 min: $(head -c 400 "$d/runs-rc.json") $(hw_show | jq -c '{state, rc, status: (.status // {} | {phase, version, runtime})}')"; return; }
echo "hold wins: at stand-h's boot Calcifer queued $rc: $(jq -c --arg rc "$rc" '.runs[] | select(.name == $rc) | {status, target}' "$d/runs-rc.json")"

code=0
cozy --json machine update stand-h --runtime "$runtime" --tensorfs "$tensorfs" --tensord "$released" -y > "$d/hand.json" 2> "$d/hand.err" || code=$?
[ "$code" = 0 ] || check "hold wins: the hand's update ($held): exit $code: $(tail -c 300 "$d/hand.json") $(tail -c 300 "$d/hand.err")"
# Every update of it settled, and it ready on what the last left.
for _ in $(seq 600); do
  hw_runs "$d/runs.json"
  J '[.runs[]? | select(.kind == "update" and (.status == "queued" or .status == "in_progress"))] | length == 0' "$d/runs.json" && break
  sleep 1
done
for _ in $(seq 180); do hw_show > "$d/show.json" && J '.state == "ready" and .status.phase == "ready" and (.status.update // "") == ""' "$d/show.json" && break; sleep 1; done
echo "hold wins: stand-h's updates:"
jq -r '.runs | sort_by(.number)[] | select(.kind == "update") | "  \(.name) \(.status): \(.target) \(.reason.message // "")"' "$d/runs.json"
echo "hold wins: stand-h runs $(jq -c '.status | {version, runtime, tensorfs}' "$d/show.json"); held $(jq -c '{held, unheld, rc}' "$d/show.json")"
J --arg rc "$rc" '[.runs[] | select(.name == $rc)][0].status == "canceled"' "$d/runs.json" \
  || check "hold wins: the RC update $rc queued at boot wasn't canceled by the hand's: $(jq -c --arg rc "$rc" '.runs[] | select(.name == $rc) | {status, reason}' "$d/runs.json")"
J '[.runs[] | select(.kind == "update" and (.target | test("\\(RC\\)")) and .status != "canceled")] | length == 0' "$d/runs.json" \
  || check "hold wins: an RC update ran past the hold: $(jq -c '[.runs[] | select(.kind == "update" and (.target | test("\\(RC\\)"))) | {name, status}]' "$d/runs.json")"
J --arg rt "$runtime" --arg tfs "$tensorfs" --arg v "$released" '.status.runtime == $rt and .status.tensorfs == $tfs and .status.version == $v' "$d/show.json" \
  || check "hold wins: stand-h runs $(jq -c '.status | {version, runtime, tensorfs}' "$d/show.json"), not the $held it was held on by hand"
J --arg h "$held" '.held == $h and .unheld == null and .rc.state == "held"' "$d/show.json" \
  || check "hold wins: machine show doesn't say stand-h is held on what it runs: $(jq -c '{held, unheld, rc}' "$d/show.json")"

# The store as beerus's was left: a hold naming a Runtime it doesn't run. The machine's word stands.
systemctl --user stop cozy-calcifer || { check "hold wins: Calcifer didn't stop"; return; }
hw_store "UPDATE machines SET held = 'runtime 0.1.0, tensorfs $tensorfs, tensord $released' WHERE name = 'stand-h'" \
  || { check "hold wins: stand-h's hold wasn't written"; return; }
hw_start || return
for _ in $(seq 120); do hw_show > "$d/show-off.json" && J '.status.runtime != null and .status.phase == "ready"' "$d/show-off.json" && break; sleep 1; done
cozy machine ls > "$d/ls.txt" 2>&1 || true
echo "hold wins: held on runtime 0.1.0, machine show says $(jq -c '{unheld, rc}' "$d/show-off.json"); machine ls: $(grep 'stand-h' "$d/ls.txt" | tr -s ' ' | paste -sd '|')"
J --arg rt "$runtime" '.unheld == "Runtime \($rt)" and (.rc.detail | contains("but it runs Runtime \($rt)"))' "$d/show-off.json" \
  || check "hold wins: machine show doesn't say stand-h runs Runtime $runtime, not the 0.1.0 its hold names: $(jq -c '{held, unheld, rc}' "$d/show-off.json")"
grep -qF "stand-h: runs Runtime $runtime, though held by hand on runtime 0.1.0" "$d/ls.txt" && grep -qE '^ *stand-h .*held, runs other software' "$d/ls.txt" \
  || check "hold wins: machine ls doesn't say what stand-h runs: $(grep 'stand-h' "$d/ls.txt" | head -c 600)"
}

# hw_serve: the stood-in rental, booting, its own process group.
hw_serve() {
  env HOME="$HW_DIR/home" COZY_MACHINE_ROOT="$HW_DIR/root" COZY_WORKER_ID=holdwins COZY_WORKER_INTERNAL_PORT="$HW_PORT" \
    COZY_LISTEN_HOST=127.0.0.1 TENSORHUB_ORIGIN="https://127.0.0.1:$HW_HUB" TENSORHUB_CA_DER_B64URL="$HW_CA" CUDA_VISIBLE_DEVICES= \
    CONTAINER_ID=4242 CONTAINER_API_KEY=holdwins-standin-key COZY_PROVIDER_API_ORIGIN="http://127.0.0.1:$HW_PROVIDER" \
    setsid "$HW_DIR/bin/cozy" machine serve --rental >> "$HW_DIR/machine.log" 2>&1 & HW_PID=$!
  echo "$HW_PID" > "$HW_DIR/machine.pid"
}

# hw_down: the rental gone at once, as a host lost.
hw_down() {
  [ -n "$HW_PID" ] || return 0
  kill -KILL -- "-$HW_PID" 2> /dev/null || true
  wait "$HW_PID" 2> /dev/null || true
  HW_PID=''
}

hw_quit() {
  hw_down
  [ -z "$HW_STANDIN" ] || { kill "$HW_STANDIN" 2> /dev/null || true; wait "$HW_STANDIN" 2> /dev/null || true; HW_STANDIN=''; }
}

# hw_store <sql>: one statement on Calcifer's stopped store, which must change one row.
hw_store() {
  python3 -I - "$HOME/.cozy/calcifer/state.db" "$1" <<'PY'
import sqlite3, sys
db = sqlite3.connect(sys.argv[1], isolation_level=None)
changed = db.execute(sys.argv[2]).rowcount
db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
sys.exit(0 if changed == 1 else 1)
PY
}

hw_start() {
  systemctl --user start cozy-calcifer || { check "hold wins: Calcifer didn't start"; return 1; }
  for _ in $(seq 60); do cozy --json machine ls > /dev/null 2>&1 && return 0; sleep 1; done
  check "hold wins: Calcifer didn't answer in 60 s"
  return 1
}

hw_runs() { cozy --json run ls --all --on stand-h --limit 0 > "$1" 2> /dev/null || true; }
hw_show() { timeout 60 "$COZY" --json machine show stand-h 2> /dev/null; }
