# e2e/cli/rentalself.sh: a rental never stops while it may still bill (cl-977), on this runner with no pod: `cozy machine serve
# --rental` against a stood-in Hub and provider (rentalself/standin.py). Linux.
#   idle-ended    Idle past its window (6 s here), its release refused because the Hub says the rental ended, it ends itself at
#                 its provider and asks until the provider takes the pod (refused twice, then taken: the stand-in kills it). Its
#                 check-ins (reads of its keys) come each half lease from their own thread, before and after its release.
#   cannot-start  Its service can't start (its store is unwritable) and exits twice before readiness: its parent ends the pod the
#                 same way before it gives up.
#   rental_queue  th-378: its check-ins come back after its Hub connection drops while a download fills its link (below).
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

# rental_queue (th-378): a rental's check-ins come back after its Hub connection drops while a download fills its link. One FIFO on the
# loopback, 1 MB/s and 4 MB deep (about 4 s each way through it once full), carries the stood-in Hub's packets and 8 bulk streams'; the
# connection is cut mid-download by restarting the stand-in on its port. A reconnect there takes four round trips. 11-16 the Hub
# client's 5 s connect bound cut each one (cale's speed report, pods downloading at 28 MB/s over 64 streams), so a pod's check-ins
# would stop until its download ended and the Hub could release it as silent.
rental_queue() {
  [ "$RUNNER_OS" = Linux ] || return 0
  step "a rental's check-ins come back through a link a download fills, after its Hub connection drops (th-378)"
  local dir=$OUT/rentalqueue standin pid hub provider ca port bulk www cut rtt tsq curls=() i
  rm -rf "$dir" && mkdir -p "$dir/home/.cozy" "$dir/root" "$dir/www"
  printf '[machine]\nidle_seconds = 3600\n' > "$dir/home/.cozy/config.toml"
  python3 "$here_rentalself/rentalself/standin.py" serve "$dir" queue > "$dir/standin.log" 2>&1 & standin=$!
  for _ in $(seq 60); do [ -s "$dir/ready" ] && break; kill -0 "$standin" 2> /dev/null || break; sleep 0.5; done
  read -r hub provider ca < "$dir/ready" 2> /dev/null || { check "rental_queue: the stand-in didn't start: $(tail -c 400 "$dir/standin.log")"; return; }
  port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
  env HOME="$dir/home" COZY_MACHINE_ROOT="$dir/root" COZY_WORKER_ID=rentalqueue COZY_WORKER_INTERNAL_PORT="$port" \
    COZY_LISTEN_HOST=127.0.0.1 TENSORHUB_ORIGIN="https://127.0.0.1:$hub" TENSORHUB_CA_DER_B64URL="$ca" CUDA_VISIBLE_DEVICES= \
    CONTAINER_ID=4243 CONTAINER_API_KEY=rentalqueue-standin-key COZY_PROVIDER_API_ORIGIN="http://127.0.0.1:$provider" \
    setsid "$COZY" machine serve --rental > "$dir/machine.log" 2>&1 & pid=$!
  echo "$pid" > "$dir/machine.pid"
  for _ in $(seq 120); do [ "$(grep -c '"kind": "check_in"' "$dir/events.jsonl" 2> /dev/null)" -ge 2 ] && break; sleep 0.5; done
  if [ "$(grep -c '"kind": "check_in"' "$dir/events.jsonl" 2> /dev/null)" -lt 2 ]; then
    check "rental_queue: no check-ins before the download: $(tail -c 600 "$dir/machine.log")"
  else
    truncate -s 4G "$dir/www/blob" && : > "$dir/www/tiny"
    bulk=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
    python3 -m http.server "$bulk" --bind 127.0.0.1 --directory "$dir/www" > "$dir/bulk.log" 2>&1 & www=$!
    sleep 1
    # TCP small queues keep each socket's own share of a local qdisc to a few hundred KB, which a router's queue on a pod's path
    # isn't: raised here, so the loopback FIFO fills as that one does.
    tsq=$(sysctl -n net.ipv4.tcp_limit_output_bytes)
    if sudo sysctl -qw net.ipv4.tcp_limit_output_bytes=16777216 && sudo ip link set dev lo mtu 1500 && sudo tc qdisc add dev lo root handle 1: prio \
      && sudo tc qdisc add dev lo parent 1:3 handle 30: tbf rate 8mbit burst 32kb limit 4mb \
      && sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip sport "$hub" 0xffff flowid 1:3 \
      && sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip sport "$bulk" 0xffff flowid 1:3; then
      for i in 1 2 3 4 5 6 7 8; do curl -s -o /dev/null --max-time 170 "http://127.0.0.1:$bulk/blob" & curls+=($!); done
      sleep 25
      rtt=$(curl -s -o /dev/null --max-time 60 -w '%{time_starttransfer}' "http://127.0.0.1:$bulk/tiny" || true)
      kill "$standin" 2> /dev/null; wait "$standin" 2> /dev/null || true
      rm -f "$dir/ready"
      python3 "$here_rentalself/rentalself/standin.py" serve "$dir" queue "$hub" >> "$dir/standin.log" 2>&1 & standin=$!
      for _ in $(seq 60); do [ -s "$dir/ready" ] && break; sleep 0.5; done
      cut=$(date +%s.%N)
      sleep 100
      running=$(for i in "${curls[@]}"; do kill -0 "$i" 2> /dev/null && echo x; done | wc -l)
      while IFS= read -r line; do
        check "rental_queue: $line; its log: $(grep -E 'Hub' "$dir/machine.log" | tail -4 | tr '\n' ' ' | tail -c 600)"
      done < <(python3 - "$dir/events.jsonl" "$cut" "$rtt" "${#curls[@]}" "$running" <<'PY'
import json, sys
events, cut, rtt, started, running = sys.argv[1], float(sys.argv[2]), sys.argv[3], int(sys.argv[4]), int(sys.argv[5])
after = [e["at"] - cut for e in map(json.loads, open(events)) if e["kind"] == "check_in" and e["at"] > cut]
print(f"rental_queue: a round trip through the queue {rtt} s; {len(after)} check-ins in the 100 s after the cut, the first at "
      f"{after[0]:.1f} s" if after else f"rental_queue: a round trip through the queue {rtt} s; no check-in in the 100 s after the cut",
      file=sys.stderr)
if not rtt or float(rtt) < 3:
    print(f"the queue's round trip was {rtt or 'unmeasured'} s, under the 3 s a deep queue gives: the cell did not make the link it means to")
elif running < started:
    print(f"the download ended before the cell did ({running} of {started} streams still running): the link wasn't full throughout")
elif not after:
    print("no check-in reached its Hub in the 100 s after the connection dropped, while the download filled the link")
PY
)
      for i in "${curls[@]}"; do kill "$i" 2> /dev/null; done
    else
      check "rental_queue: the bottleneck wasn't made"
    fi
    sudo tc qdisc del dev lo root 2> /dev/null; sudo ip link set dev lo mtu 65536; sudo sysctl -qw net.ipv4.tcp_limit_output_bytes="$tsq"
    kill "$www" 2> /dev/null
  fi
  kill -KILL -- "-$pid" 2> /dev/null || true
  kill "$standin" 2> /dev/null || true
  wait "$pid" "$standin" ${www:+"$www"} "${curls[@]}" 2> /dev/null || true
}
