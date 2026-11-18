# hold_wins (cl-1196). Found 11-17 on a rented RTX 4090, beerus: `cozy machine update beerus --runtime 0.47.0 --tensorfs 0.26.0
# --tensord 0.26.0` held it by hand, and the RC update Calcifer queued at its boot ran anyway: beerus ended on 0.27.0-dev and Runtime
# 0.48.0.dev2319, while `machine ls` and `show` said "held by hand on runtime 0.47.0". The hand's update staged nothing and reached
# beerus first; the RC's, staging the RC's 99 MB cozy over the laptop's uplink, came after, and beerus ran it next.
# A dev home's rental (`dev = true`), stood in by `cozy machine serve --rental` on this runner against a stood-in Hub
# (rentalself/standin.py) whose lease grants this computer's key, as a Hub grants the devices of the account that rented it; its
# image is the newest release's, as a worker image is: GitHub's Latest cozy, PyPI's newest Runtime and TensorFS. Calcifer reaches it
# through a laptop's uplink (uplink.py, 1 MiB/s). Recorded as a rental this computer ordered, it boots, and Calcifer queues its
# move to the open RC (rc-<x>'s prerelease), whose cozy it stages, as beerus's was. Then held by hand on
# that release, as beerus was: it stays on it, the RC update canceled and never run, and `machine show` says it is held on what it
# runs. Then its hold names another Runtime, as beerus's store kept it: `machine show` and `machine ls` say what it runs. Linux, a
# GitHub runner; its own shard.
here_holdwins=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
HW_DIR='' HW_CFG='' HW_SAVED='' HW_PID='' HW_STANDIN='' HW_UPLINK='' HW_PORT='' HW_HUB='' HW_PROVIDER='' HW_CA=''

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
local d=$HW_DIR v runtime tensorfs released key fp code rc held relay
v=$(cozy --version | awk '{print $2}')
[[ $v == *-dev+g* ]] || { check "hold wins: cozy $v is no RC build, with no RC to follow"; return; }
runtime=$(curl -fsSL --retry 3 https://pypi.org/pypi/cozy-runtime/json | jq -r '.info.version // empty')
tensorfs=$(curl -fsSL --retry 3 https://pypi.org/pypi/tensorfs/json | jq -r '.info.version // empty')
released=$(curl -fsSL --retry 3 https://github.com/cozy-creator/cozy-creator/releases/latest/download/dist-manifest.json \
  | jq -r '.releases[] | select(.app_name == "cozy") | .app_version')
[ -n "$runtime" ] && [ -n "$tensorfs" ] && [ -n "$released" ] \
  || { check "hold wins: the newest releases: Runtime ${runtime:-?}, TensorFS ${tensorfs:-?}, cozy ${released:-?}"; return; }
held="runtime $runtime, tensorfs $tensorfs, tensord $released"
key=$(cozy --json machine show local | jq -r '.controller_key // empty')
[ -n "$key" ] || { check "hold wins: machine show local names no controller key"; return; }

# The stood-in Hub, granting this computer's key; the rental's image: its own cozy (an update replaces the file it runs from) and pair.
rm -rf "$d" && mkdir -p "$d/home/.cozy" "$d/root/opt/cozy/wheels" "$d/bin" "$d/x" || { check "hold wins: $d not made"; return; }
printf '[machine]\nidle_seconds = 3600\n' > "$d/home/.cozy/config.toml"
printf '%s\n' "$key" > "$d/keys"
hw_image "$released" cozy-runtime "$runtime" tensorfs "$tensorfs" || return
python3 "$here_holdwins/rentalself/standin.py" serve "$d" held > "$d/standin.log" 2>&1 & HW_STANDIN=$!
for _ in $(seq 60); do [ -s "$d/ready" ] && break; kill -0 "$HW_STANDIN" 2> /dev/null || break; sleep 0.5; done
read -r HW_HUB HW_PROVIDER HW_CA < "$d/ready" 2> /dev/null || { check "hold wins: the stand-in Hub didn't start: $(tail -c 400 "$d/standin.log")"; return; }
HW_PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
python3 "$here_holdwins/uplink.py" "$d/uplink.port" "$HW_PORT" 1048576 > "$d/uplink.log" 2>&1 & HW_UPLINK=$!
for _ in $(seq 20); do [ -s "$d/uplink.port" ] && break; sleep 0.5; done
relay=$(cat "$d/uplink.port" 2> /dev/null) && [ -n "$relay" ] || { check "hold wins: the uplink didn't start: $(tail -c 300 "$d/uplink.log")"; return; }

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
  code=0; cozy --json machine add stand-h "127.0.0.1:$relay" --fingerprint "sha256:$fp" > "$d/add.json" 2>&1 || code=$?
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

# It boots, and Calcifer queues its move to the RC, a job of the machine's (`machine show`): the run the hand's supersedes.
hw_serve
rc=''
for _ in $(seq 360); do
  hw_show > "$d/show-rc.json" 2> /dev/null || true
  rc=$(jq -r '[.jobs[]? | select(.kind == "update" and (.target | test("\\(RC\\)")))] | first | .id // empty' "$d/show-rc.json" 2> /dev/null)
  [ -n "$rc" ] && break
  sleep 0.5
done
[ -n "$rc" ] || { check "hold wins: no RC update queued at stand-h's boot in 3 min: $(jq -c '{state, rc, jobs, status: (.status // {} | {phase, version, runtime})}' "$d/show-rc.json" | head -c 800)"; return; }
echo "hold wins: at stand-h's boot Calcifer queued $rc: $(jq -c --arg rc "$rc" '.jobs[] | select(.id == $rc) | {status, waiting, target}' "$d/show-rc.json")"

code=0
cozy --json machine update stand-h --runtime "$runtime" --tensorfs "$tensorfs" --tensord "$released" -y > "$d/hand.json" 2> "$d/hand.err" || code=$?
[ "$code" = 0 ] || check "hold wins: the hand's update ($held): exit $code: $(tail -c 300 "$d/hand.json") $(tail -c 300 "$d/hand.err")"
# Every update of it settled, and it ready on what the last left.
for _ in $(seq 600); do
  hw_show > "$d/show.json" 2> /dev/null && J '[.jobs[]? | select(.kind == "update" and (.status == "queued" or .status == "in_progress"))] | length == 0
    and .state == "ready" and .status.phase == "ready" and (.status.update // "") == ""' "$d/show.json" && break
  sleep 1
done
echo "hold wins: stand-h's updates:"
jq -r '.jobs[]? | select(.kind == "update") | "  \(.id) \(.status): \(.target) \(.reason.message // "")"' "$d/show.json"
echo "hold wins: stand-h runs $(jq -c '.status | {version, runtime, tensorfs}' "$d/show.json"); held $(jq -c '{held, unheld, rc}' "$d/show.json")"
J --arg rc "$rc" '[.jobs[]? | select(.id == $rc)][0].status == "canceled"' "$d/show.json" \
  || check "hold wins: the RC update $rc queued at boot wasn't canceled by the hand's: $(jq -c --arg rc "$rc" '[.jobs[]? | select(.id == $rc) | {status, reason}]' "$d/show.json")"
J '[.jobs[]? | select(.kind == "update" and (.target | test("\\(RC\\)")) and .status != "canceled")] | length == 0' "$d/show.json" \
  || check "hold wins: an RC update ran past the hold: $(jq -c '[.jobs[]? | select(.kind == "update" and (.target | test("\\(RC\\)"))) | {id, status}]' "$d/show.json")"
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

# hw_image <cozy> <distribution> <version>...: the release's cozy, checked against its release's sha256.sum, and each wheel from PyPI
# (its abi3 manylinux x86_64 wheel, else its pure one), where a worker image keeps them.
hw_image() {
  local d=$HW_DIR a=cozy-x86_64-unknown-linux-gnu.tar.gz base="https://github.com/cozy-creator/cozy-creator/releases/download/v$1" bin url
  curl -fsSL --retry 3 -o "$d/$a" "$base/$a" && curl -fsSL --retry 3 -o "$d/sha256.sum" "$base/sha256.sum" \
    && (cd "$d" && awk -v f="$a" '{ n = $2; sub(/^\*/, "", n) } n == f' sha256.sum | sha256sum -c --strict -) > /dev/null \
    && tar -xzf "$d/$a" -C "$d/x" --no-same-owner && bin=$(find "$d/x" -type f -name cozy -print -quit) && [ -n "$bin" ] \
    && install -m 755 "$bin" "$d/bin/cozy" || { check "hold wins: cozy $1's $a didn't check against its sha256.sum"; return 1; }
  shift
  while [ $# -ge 2 ]; do
    url=$(curl -fsSL --retry 3 "https://pypi.org/pypi/$1/$2/json" | jq -r '[.urls[] | select(.packagetype == "bdist_wheel")]
      | (map(select(.filename | test("abi3-manylinux.*x86_64"))) + map(select(.filename | test("-none-any")))) | first | .url // empty')
    [ -n "$url" ] && curl -fsSL --retry 3 -o "$d/root/opt/cozy/wheels/${url##*/}" "$url" || { check "hold wins: $1 $2's wheel from PyPI: ${url:-none}"; return 1; }
    shift 2
  done
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
  [ -z "$HW_UPLINK" ] || { kill "$HW_UPLINK" 2> /dev/null || true; wait "$HW_UPLINK" 2> /dev/null || true; HW_UPLINK=''; }
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

hw_show() { timeout 60 "$COZY" --json machine show stand-h 2> /dev/null; }
