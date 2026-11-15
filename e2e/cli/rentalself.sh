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
