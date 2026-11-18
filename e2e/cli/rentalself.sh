# e2e/cli/rentalself.sh: a rental never stops while it may still bill (cl-977), on this runner with no pod: `cozy machine serve
# --rental` against a stood-in Hub and provider (rentalself/standin.py). Linux.
#   idle-ended    Idle past its window (6 s here), its release refused because the Hub says the rental ended, it ends itself at
#                 its provider and asks until the provider takes the pod (refused twice, then taken: the stand-in kills it). Its
#                 check-ins (reads of its keys) come each half lease from their own thread, before and after its release.
#   cannot-start  Its service can't start (its store is unwritable) and exits twice before readiness: its parent ends the pod the
#                 same way before it gives up.
here_rentalself=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

rental_self() {
  [ "$RUNNER_OS" = Linux ] || return 0
  step "a rental ends itself until its provider takes it, checking in from its own thread (cl-977)"
  rental_self_case idle-ended
  step "a rental whose service can't start ends its pod before its parent gives up (cl-977)"
  rental_self_case cannot-start
}

# A process that runs, not yet exited: a child this shell hasn't waited for stays a zombie, which kill -0 still finds.
rental_self_alive() { [ -r "/proc/$1/stat" ] && ! grep -qE '^[0-9]+ \(.*\) Z' "/proc/$1/stat" 2> /dev/null; }

# rental_self_case <case>: one machine against a fresh stand-in, in $OUT/rentalself-<case>.
rental_self_case() {
  local case=$1 dir=$OUT/rentalself-$1 standin pid hub provider ca port store=() line
  rm -rf "$dir" && mkdir -p "$dir/home/.cozy" "$dir/root"
  printf '[machine]\nidle_seconds = 6\n' > "$dir/home/.cozy/config.toml"
  python3 "$here_rentalself/rentalself/standin.py" serve "$dir" "$case" > "$dir/standin.log" 2>&1 & standin=$!
  for _ in $(seq 60); do [ -s "$dir/ready" ] && break; kill -0 "$standin" 2> /dev/null || break; sleep 0.5; done
  read -r hub provider ca < "$dir/ready" 2> /dev/null || { check "rental_self $case: the stand-in didn't start: $(tail -c 400 "$dir/standin.log")"; return; }
  port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
  [ "$case" = cannot-start ] && store=(COZY_TENSORFS_ROOT=/dev/null/store)
  # Its own group (setsid), whose leader's pid the stand-in kills as a provider ends the container.
  env HOME="$dir/home" COZY_MACHINE_ROOT="$dir/root" COZY_WORKER_ID="rentalself-$case" COZY_WORKER_INTERNAL_PORT="$port" \
    COZY_LISTEN_HOST=127.0.0.1 TENSORHUB_ORIGIN="https://127.0.0.1:$hub" TENSORHUB_CA_DER_B64URL="$ca" CUDA_VISIBLE_DEVICES= \
    CONTAINER_ID=4242 CONTAINER_API_KEY=rentalself-standin-key COZY_PROVIDER_API_ORIGIN="http://127.0.0.1:$provider" "${store[@]}" \
    setsid "$COZY" machine serve --rental > "$dir/machine.log" 2>&1 & pid=$!
  echo "$pid" > "$dir/machine.pid"
  for _ in $(seq 240); do rental_self_alive "$pid" || break; sleep 0.5; done
  if rental_self_alive "$pid"; then
    check "rental_self $case: the machine still runs 120 s on: $(tail -c 600 "$dir/machine.log")"
    kill -KILL -- "-$pid" 2> /dev/null || true
  else
    sleep 1
    grep -q '"kind": "taken"' "$dir/events.jsonl" 2> /dev/null || touch "$dir/exited"
  fi
  wait "$pid" 2> /dev/null || true
  kill "$standin" 2> /dev/null || true
  wait "$standin" 2> /dev/null || true
  while IFS= read -r line; do
    [ -z "$line" ] || check "rental_self $line; its log: $(grep -E 'provider|released|release|Hub' "$dir/machine.log" | tail -5 | tr '\n' ' ' | tail -c 600)"
  done < <(python3 "$here_rentalself/rentalself/standin.py" verdict "$dir" "$case" 2>&1)
  echo "rental_self $case: $(grep -c '"kind": "end"' "$dir/events.jsonl" 2> /dev/null) provider asks, $(grep -c '"kind": "check_in"' "$dir/events.jsonl" 2> /dev/null) check-ins, taken: $(grep -c '"kind": "taken"' "$dir/events.jsonl" 2> /dev/null)"
}

# slow_link (th-405, cl-1213). Paul, 11-18, after aono, an RTX 4090 on vast, pulled its models at 14.7 MB/s: "damn that's really
# slow; make sure we don't run on slow hosts like that again please". A rental holds its first download to the floor its lease
# names: under it for the lease's window, past its warm-up, it tells its Hub once and reads whether the Hub is placing it on another
# host; over it, it tells the pace its link sustained, once 1 GiB is in. Each case is a stood-in rental (as holdwins.sh's: `cozy
# machine serve --rental` on this runner, its stood-in Hub granting this computer's key) whose lease names 120 Mbit/s after 2 s over
# 6 s. Added here as a machine, it installs pagebox's 1.6 GB from the isolated Hub's store through one bottleneck (tc on the
# loopback, 1500-byte packets, as ondemand_share's).
#   slow    60 Mbit/s: one `slow_link`, the link's pace over its window, answered `replacing`, which its log says; no `download`.
#   paced   240 Mbit/s: no `slow_link`; one `download`, 1 GiB or more at the link's pace.
# On rc/0.27.0 the slow rental sent no `slow_link`.
slow_link() {
step "a rental whose first download stays under its lease's floor tells its Hub once; over it, the pace it sustained (th-405)"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "slow link: a fresh GitHub Linux runner only"; return 0; }
local key minio
key=$(cozy --json machine show local | jq -r '.controller_key // empty')
[ -n "$key" ] || { check "slow link: machine show local names no controller key"; return; }
minio=$(pgrep -af minio | grep -oE -- '--address[ =]127\.0\.0\.1:[0-9]+' | grep -oE '[0-9]+$' | head -1) || true
[ -n "$minio" ] || { check "slow link: the isolated Hub's MinIO port wasn't found: $(pgrep -af minio | head -2)"; return; }
slow_link_case slow 60 "$key" "$minio"
slow_link_case paced 240 "$key" "$minio"
}

# slow_link_case <slow|paced> <Mbit/s> <controller key> <store port>: one rental through a link that wide, in $OUT/slowlink-<case>.
slow_link_case() {
  local case=$1 wide=$2 d=$OUT/slowlink-$1 name=stand-$1 standin pid='' install='' hub provider ca port fp code line began left=20
  rm -rf "$d" && mkdir -p "$d/home/.cozy" "$d/root" || { check "slow link $case: $d not made"; return; }
  printf '[machine]\nidle_seconds = 3600\n' > "$d/home/.cozy/config.toml"
  printf '%s\n' "$3" > "$d/keys"
  echo '{"mbps": 120, "warmup_seconds": 2, "window_seconds": 6}' > "$d/link_floor.json"
  echo "$wide" > "$d/link_mbit"
  python3 "$here_rentalself/rentalself/standin.py" serve "$d" held > "$d/standin.log" 2>&1 & standin=$!
  slow_link_quit() {
    [ -z "$install" ] || { kill "$install" 2> /dev/null || true; wait "$install" 2> /dev/null || true; }
    [ -z "$pid" ] || { kill -KILL -- "-$pid" 2> /dev/null || true; wait "$pid" 2> /dev/null || true; }
    kill "$standin" 2> /dev/null || true; wait "$standin" 2> /dev/null || true
    sudo tc qdisc del dev lo root 2> /dev/null || true
    sudo ip link set dev lo mtu 65536 || true
    timeout 60 "$COZY" --json machine end "$name" --force -y > "$d/end.json" 2>&1 || true
  }
  for _ in $(seq 60); do [ -s "$d/ready" ] && break; kill -0 "$standin" 2> /dev/null || break; sleep 0.5; done
  read -r hub provider ca < "$d/ready" 2> /dev/null || { check "slow link $case: the stand-in Hub didn't start: $(tail -c 400 "$d/standin.log")"; slow_link_quit; return; }
  port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
  env HOME="$d/home" COZY_MACHINE_ROOT="$d/root" COZY_WORKER_ID="slowlink-$case" COZY_WORKER_INTERNAL_PORT="$port" \
    COZY_LISTEN_HOST=127.0.0.1 TENSORHUB_ORIGIN="https://127.0.0.1:$hub" TENSORHUB_CA_DER_B64URL="$ca" CUDA_VISIBLE_DEVICES= \
    CONTAINER_ID=4242 CONTAINER_API_KEY=slowlink-standin-key COZY_PROVIDER_API_ORIGIN="http://127.0.0.1:$provider" \
    setsid "$COZY" machine serve --rental > "$d/machine.log" 2>&1 & pid=$!
  echo "$pid" > "$d/machine.pid"
  for _ in $(seq 240); do
    openssl s_client -connect "127.0.0.1:$port" < /dev/null 2> /dev/null | openssl x509 -outform der > "$d/leaf.der" 2> /dev/null && [ -s "$d/leaf.der" ] && break
    kill -0 "$pid" 2> /dev/null || break
    sleep 0.5
  done
  [ -s "$d/leaf.der" ] || { check "slow link $case: the rental never served on $port: $(tail -c 800 "$d/machine.log")"; slow_link_quit; return; }
  fp=$(sha256sum < "$d/leaf.der" | cut -c1-64)
  for _ in $(seq 60); do
    code=0; cozy --json machine add "$name" "127.0.0.1:$port" --fingerprint "sha256:$fp" > "$d/add.json" 2>&1 || code=$?
    [ "$code" = 0 ] && break
    sleep 1
  done
  [ "$code" = 0 ] || { check "slow link $case: machine add $name: $(tail -c 400 "$d/add.json") $(tail -c 400 "$d/machine.log")"; slow_link_quit; return; }
  sudo tc qdisc del dev lo root 2> /dev/null || true
  sudo ip link set dev lo mtu 1500 && sudo tc qdisc add dev lo root handle 1: prio \
    && sudo tc qdisc add dev lo parent 1:3 handle 30: tbf rate "${wide}mbit" burst 1mb latency 200ms \
    && sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip sport "$4" 0xffff flowid 1:3 \
    || { check "slow link $case: the link wasn't made ${wide} Mbit/s wide"; slow_link_quit; return; }
  began=$(date +%s)
  cozy --json model install lilylilith/pagebox --on "$name" > "$d/install.ndjson" 2>&1 & install=$!
  # Its word to its Hub: 8 s into a slow download; 1 GiB into a paced one, 36 s at 240 Mbit/s, or as its download ends. One that
  # says nothing is given 4 min, or 10 s past its install's end.
  for _ in $(seq 480); do
    grep -q '"kind": "speed"' "$d/events.jsonl" 2> /dev/null && break
    kill -0 "$install" 2> /dev/null || left=$((left - 1))
    [ "$left" -gt 0 ] || break
    sleep 0.5
  done
  echo "slow link $case: $(($(date +%s) - began)) s after its install was asked: $(grep '"kind": "speed"' "$d/events.jsonl" 2> /dev/null | head -3 | tr '\n' ' ')"
  # The Hub's answer, as its log says it.
  [ "$case" != slow ] || for _ in $(seq 20); do grep -q 'another host' "$d/machine.log" && break; sleep 0.5; done
  slow_link_quit
  while IFS= read -r line; do
    [ -z "$line" ] || check "slow link $case: $line; its log: $(grep -E 'link|Hub' "$d/machine.log" | tail -4 | tr '\n' ' ' | tail -c 600); its install: $(tail -c 300 "$d/install.ndjson" | tr '\n' ' ')"
  done < <(python3 "$here_rentalself/rentalself/standin.py" link "$d" "$case" 2> "$d/link.ok")
  cat "$d/link.ok"
}
