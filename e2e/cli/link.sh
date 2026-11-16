# e2e/cli/link.sh: tfs-300's download-system metrics (Paul 11-16: "we have a limited resource, network bandwidth, (and perhaps also
# disc write to a lesser degree), and we need to manage it efficiently across multiple different consumers … it'll be proven
# empirically"). Each scenario runs arms, each from an empty store on a restarted machine, through one bottleneck on the loopback
# (tc, MTU 1500), and keeps what the kernel saw each second: every traffic class's bytes by its server's port (iptables), the
# bottleneck's queue and drops, TensorD's memory and writes; and what Status said of each download, read from the run's project.
# e2e/link/judge.py turns an arm's files into a scenario's record ($OUT/link/<scenario>.json); e2e/link/table.py compares records.
# Nothing here judges pass or fail but the yes/no guarantees the judge names; the bars are table.py's.
#   forms   q60   60 Mbit, a FIFO of 1 s (tbf); the runner's congestion control, kept in each arm's meta
#           q60c  q60 with cubic as the senders' congestion control (a loss-based sender fills the queue)
#           p60   60 Mbit, one shared token bucket that drops, no queue (tc police; tfs-300 proof branch 4158e8650's)
#           open  no limit
here_link=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
LINK_TOOLS=$here_link/../link
LINK_SCRIPT=$here_link/script/link.py
LINK_ARM='' LINK_CC='' LINK_STORE=''

now_ms() { date +%s%3N; }
link_minio() { pgrep -af minio | grep -oE -- '--address[ =]127\.0\.0\.1:[0-9]+' | grep -oE '[0-9]+$' | head -1; }
link_hub() { grep -oE ':[0-9]+/' <<< "${COZY_TEST_HUB_HEALTH:-}" | tr -d :/; }

# link_files: a file server for the cell's own files (the ceiling's, package code's downloads), else the one isolated.sh started
# for link-cold's packages (E2E_LINK_FILES): LINK_FILES_PORT, LINK_WWW.
link_files() {
  [ -z "${LINK_FILES_PORT:-}" ] || return 0
  if [ -n "${E2E_LINK_FILES:-}" ]; then
    LINK_FILES_PORT=${E2E_LINK_FILES##*:} LINK_WWW=$E2E_LINK_WWW
  else
    LINK_WWW=$RUNNER_TEMP/link-www LINK_FILES_PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
    mkdir -p "$LINK_WWW"
    (cd / && exec python3 -m http.server "$LINK_FILES_PORT" --bind 127.0.0.1 --directory "$LINK_WWW") > "$OUT/link-files.log" 2>&1 &
  fi
  [ -f "$LINK_WWW/ceiling.bin" ] || head -c $((256 << 20)) /dev/urandom > "$LINK_WWW/ceiling.bin"
  for _ in $(seq 50); do curl -s -o /dev/null "http://127.0.0.1:$LINK_FILES_PORT/" && return 0; sleep 0.2; done
  check "link: the file server on $LINK_FILES_PORT didn't start: $(tail -c 300 "$OUT/link-files.log")"; return 1
}

# link_up <form> <class>:<port>[:d]...: the bottleneck, and a counter per class (a server's port: bytes it sent; `:d` a port
# bytes were sent to).
link_up() {
  local form=$1 spec name port dir match; shift
  sudo iptables -w -N E2E_LINK 2> /dev/null; sudo iptables -w -F E2E_LINK
  sudo iptables -w -C INPUT -i lo -j E2E_LINK 2> /dev/null || sudo iptables -w -I INPUT -i lo -j E2E_LINK || return 1
  case $form in
    q60 | q60c | p60) sudo ip link set dev lo mtu 1500 && sudo tc qdisc add dev lo root handle 1: prio || return 1 ;;
    open) ;;
    *) check "link: no form $form"; return 1 ;;
  esac
  case $form in
    q60 | q60c) sudo tc qdisc add dev lo parent 1:3 handle 30: tbf rate 60mbit burst 1mb latency 1s || return 1 ;;
    p60) sudo tc actions add action police rate 60mbit burst 128k drop index 1 || return 1 ;;
  esac
  if [ "$form" = q60c ]; then
    LINK_CC=$(sysctl -n net.ipv4.tcp_congestion_control)
    sudo sysctl -qw net.ipv4.tcp_congestion_control=cubic || return 1
  fi
  for spec in "$@"; do
    IFS=: read -r name port dir <<< "$spec"
    match=sport; [ "$dir" != d ] || match=dport
    sudo iptables -w -A E2E_LINK -p tcp --"$match" "$port" -m comment --comment "$name" || return 1
    case $form in
      q60 | q60c) sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip "$match" "$port" 0xffff flowid 1:3 || return 1 ;;
      p60) sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip "$match" "$port" 0xffff flowid 1:3 action police index 1 || return 1 ;;
    esac
  done
}

link_down() {
  sudo tc qdisc del dev lo root 2> /dev/null
  sudo tc actions flush action police 2> /dev/null
  sudo ip link set dev lo mtu 65536
  [ -z "$LINK_CC" ] || sudo sysctl -qw net.ipv4.tcp_congestion_control="$LINK_CC"
  LINK_CC=''
  sudo iptables -w -D INPUT -i lo -j E2E_LINK 2> /dev/null
  sudo iptables -w -F E2E_LINK 2> /dev/null; sudo iptables -w -X E2E_LINK 2> /dev/null
  true
}

# link_store <name> [dir]: the machine on an empty store (dir, else one of its own), restarted; the last arm's store removed.
link_store() {
  local store=${2:-$RUNNER_TEMP/link-store/$1}
  rm -rf "$store" && mkdir -p "$store" || return 1
  disk_store "$HOME/.cozy/config.toml" "$store"
  machine_stop || true
  [ -z "$LINK_STORE" ] || [ "$LINK_STORE" = "$store" ] || [ "${LINK_STORE#$RUNNER_TEMP/link-store/}" = "$LINK_STORE" ] || rm -rf "$LINK_STORE"
  LINK_STORE=$store
  machine_ready
}

# meta <key> <json>: one fact of the arm, into its meta.json.
meta() {
  local file=$LINK_ARM/meta.json
  [ -s "$file" ] || echo '{}' > "$file"
  jq --arg k "$1" --argjson v "$2" '.[$k] = $v' "$file" > "$file.next" && mv "$file.next" "$file"
}
mark() { meta "$1" "$(now_ms)"; }

# link_arm <scenario> <arm> <form> [--disk DEV] [--poll DIR]... <class:port>...: an arm begins: its machine on an empty store, the
# link made, the instruments started (the kernel's every LINK_INTERVAL s, 1 by default; Status from the run project, and from each
# --poll project), the link's ceiling measured on a shaped form. link_arm_end ends it.
link_arm() {
  local scenario=$1 arm=$2 form=$3 disk='' polls=() p; shift 3
  while [ $# -gt 0 ]; do
    case $1 in
      --disk) disk=$2; shift 2 ;;
      --poll) polls+=("$2"); shift 2 ;;
      *) break ;;
    esac
  done
  LINK_ARM=$OUT/link/$scenario/$arm
  rm -rf "$LINK_ARM" && mkdir -p "$LINK_ARM"
  step "tfs-300 $scenario: $arm on $form"
  link_store "$scenario-$arm" "${LINK_STORE_DIR:-}" || { check "link $scenario/$arm: no empty store"; return 1; }
  link_files || return 1
  link_up "$form" "files:$LINK_FILES_PORT" "$@" || { link_down; check "link $scenario/$arm: the $form link wasn't made"; return 1; }
  meta scenario "\"$scenario\""; meta arm "\"$arm\""; meta form "\"$form\""
  meta cc "\"$(sysctl -n net.ipv4.tcp_congestion_control)\""; meta mtu "$(cat /sys/class/net/lo/mtu)"
  meta classes "$(printf '%s\n' "files:$LINK_FILES_PORT" "$@" | jq -R 'split(":") | {(.[0]): (.[1] | tonumber)}' | jq -s add)"
  meta runner "$(jq -n --arg k "$(uname -r)" --arg c "$(nproc)" --arg m "$(awk '/MemTotal/ {print $2}' /proc/meminfo)" '{kernel: $k, cpus: ($c | tonumber), mem_kib: ($m | tonumber)}')"
  sudo python3 "$LINK_TOOLS/watch.py" kernel "$LINK_ARM/kernel.ndjson" "$LINK_ARM/stop" "${LINK_INTERVAL:-1}" ${disk:+--disk "$disk"} 2> "$LINK_ARM/kernel.err" &
  (cd "$RUN_PROJECT" && COZY_BIN=$COZY exec python3 "$LINK_TOOLS/watch.py" status "$LINK_ARM/status.ndjson" "$LINK_ARM/stop" local) 2> "$LINK_ARM/status.err" &
  for p in ${polls[@]+"${polls[@]}"}; do
    (cd "$p" && COZY_BIN=$COZY exec python3 "$LINK_TOOLS/watch.py" status "$LINK_ARM/status-${p##*/}.ndjson" "$LINK_ARM/stop" local) 2> "$LINK_ARM/status-${p##*/}.err" &
  done
  if [ "${LINK_PROBE:-}" = 1 ]; then
    python3 "$LINK_TOOLS/watch.py" probe "$LINK_ARM/probe.ndjson" "$LINK_ARM/stop" "$COZY_TEST_HUB_HEALTH" 2> "$LINK_ARM/probe.err" &
  fi
  sleep 2
  [ "$form" = open ] || link_ceiling
  mark began_ms
}

# The link's ceiling: four GETs of the file server's 256 MiB through it for 12 s; the judge reads the files class's bytes over the
# last 10 s of the window.
link_ceiling() {
  local i pids=()
  mark ceiling_from_ms
  for i in 1 2 3 4; do curl -s -o /dev/null --max-time 12 "http://127.0.0.1:$LINK_FILES_PORT/ceiling.bin" & pids+=($!); done
  wait "${pids[@]}"
  mark ceiling_to_ms
  sleep 2
}

link_arm_end() {
  mark ended_ms
  sleep "${LINK_TAIL_S:-2}"
  touch "$LINK_ARM/stop"
  sleep 3
  sudo pkill -f "watch.py kernel $LINK_ARM/" 2> /dev/null || true
  link_down
  cozy machine logs local --lines 0 --since 1h > "$LINK_ARM/tensord.log" 2>&1 || true
}

# link_run <name> <cozy args…>: a run asked now, its ask's time kept as <name>_ms in the arm's meta; its files in the arm.
link_run() {
  local name=$1; shift
  meta "ask_$name" "$(now_ms)"
  python3 "$LINK_TOOLS/watch.py" stamp "$LINK_ARM/$name.stamped" "$LINK_ARM/stop" "$LINK_ARM/$name.ndjson" 2> /dev/null &
  run_submit "$LINK_ARM" "$name" "$@" > /dev/null
}

# link_end <name>…: each run followed to its end; one that didn't complete is said.
link_end() {
  local name
  for name in "$@"; do
    run_wait "$LINK_ARM" "$name" || check "link $(jq -r '.scenario + "/" + .arm' "$LINK_ARM/meta.json"): $name did not end: $(tail -c 300 "$LINK_ARM/$name.ndjson")"
    [ "$(run_get "$LINK_ARM" "$name" .status)" = completed ] \
      || check "link $(jq -r '.scenario + "/" + .arm' "$LINK_ARM/meta.json"): $name $(run_get "$LINK_ARM" "$name" .status): $(run_get "$LINK_ARM" "$name" .reason.message | head -c 300)"
  done
}

# link_bytes <class>: the bytes the class has moved since the arm's link was made.
link_bytes() { sudo iptables -w -nvxL E2E_LINK | awk -v c="/* $1 */" 'index($0, c) { n += $2 } END { print n + 0 }'; }

# link_until <class> <bytes> [seconds]: until the class has moved that many bytes, at most that long (600 s).
link_until() {
  local i
  for i in $(seq $((${3:-600} * 5))); do [ "$(link_bytes "$1")" -ge "$2" ] && return 0; sleep 0.2; done
  return 1
}

# link_judge <scenario>: the scenario's record from its arms.
link_judge() {
  python3 "$LINK_TOOLS/judge.py" "$1" "$OUT/link/$1" > "$OUT/link/$1.json" 2> "$OUT/link/$1.err" \
    || check "link $1: no record: $(tail -c 600 "$OUT/link/$1.err")"
  jq -c '{scenario, rows, guarantees}' "$OUT/link/$1.json" 2> /dev/null || true
  jq -r '.failed[]? // empty' "$OUT/link/$1.json" 2> /dev/null | while read -r why; do check "link $1: $why"; done
}

PAGEBOX=lilylilith/pagebox@0.1.0

# 1. Cold start (Paul 11-15: "some long package install (say 2 or 3 minutes) might pause all model downloads unnecessarily"): a
# published package's first run on an empty machine. Its install downloads for real through the link (a 30 MB wheel by URL, and an
# sdist whose build waits 30 s, fetches 60 MB and waits 60 s: a compile's quiet stretches), and its function needs pagebox's
# 1.6 GB, which the run lines up from the package's card before the install begins. Per form: the install alone (its weightless
# function, its own package), the weights alone (`model install`), and the cold run. isolated.sh's `linkpkg` published
# e2e-link-<form><a|c> (and e2e-link-warm, run first, so no arm pays for uv's empty cache).
link_cold() {
  [ "$RUNNER_OS" = Linux ] || return 0
  local minio form
  minio=$(link_minio)
  [ -n "$minio" ] && [ -n "${E2E_LINK_FILES:-}" ] || { check "link cold: no MinIO port ($minio) or no packages (isolated.sh linkpkg)"; return 0; }
  step "tfs-300 cold: uv's cache warmed by e2e-link-warm"
  LINK_ARM=$OUT/link/cold/warm
  mkdir -p "$LINK_ARM"
  run_submit "$LINK_ARM" warm run e2e-link-warm/bare --on local > /dev/null
  link_end warm
  for form in ${LINK_FORMS:-q60 p60 open}; do
    link_arm cold "$form-install" "$form" "weights:$minio" || continue
    link_run install run "e2e-link-${form}a/bare" --on local
    link_end install
    link_arm_end
    link_arm cold "$form-weights" "$form" "weights:$minio" || continue
    link_run weights model install "$PAGEBOX" --on local
    link_end weights
    link_arm_end
    link_arm cold "$form-cold" "$form" "weights:$minio" || continue
    link_run cold run "e2e-link-${form}c/cold" --on local
    link_end cold
    link_arm_end
  done
  link_judge cold
}

# 2. A running job against prefetch (cl-966): a call holds 10 s, then reads its on-demand e2e/link-a (600 MiB, model_need), timing
# its own read; beside it, pagebox's 1.6 GB is asked once it holds, as a later job's weights fetched ahead. Alone, each.
link_need() {
  [ "$RUNNER_OS" = Linux ] || return 0
  local minio
  minio=$(link_minio)
  link_arm need need-alone q60 "weights:$minio" || return 0
  link_run need run "$LINK_SCRIPT/hold" seconds:=10 read:=true --on local
  link_end need
  link_arm_end
  link_arm need ahead-alone q60 "weights:$minio" || return 0
  link_run ahead model install "$PAGEBOX" --on local
  link_end ahead
  link_arm_end
  link_arm need both q60 "weights:$minio" || return 0
  link_run need run "$LINK_SCRIPT/hold" seconds:=10 read:=true --on local
  for _ in $(seq 900); do grep -q '"holding"' "$LINK_ARM/need.ndjson" 2> /dev/null && break; sleep 0.2; done
  link_run ahead model install "$PAGEBOX" --on local
  link_end need ahead
  link_arm_end
  link_judge need
}

# 10. Package code's own download at run time (tfs-300's design asks): a call GETs 200 MiB from the file server through the link
# while pagebox is fetched ahead. Alone, each.
link_thirdparty() {
  [ "$RUNNER_OS" = Linux ] || return 0
  local minio url
  minio=$(link_minio)
  link_files || return 0
  [ -f "$LINK_WWW/blob-200.bin" ] || head -c $((200 << 20)) /dev/urandom > "$LINK_WWW/blob-200.bin"
  url=http://127.0.0.1:$LINK_FILES_PORT/blob-200.bin
  # Its environment first, so neither arm times an install.
  LINK_ARM=$OUT/link/thirdparty/warm
  mkdir -p "$LINK_ARM"
  run_submit "$LINK_ARM" warm run "$LINK_SCRIPT/fetch" "url=http://127.0.0.1:$LINK_FILES_PORT/ceiling.bin" --on local > /dev/null
  link_end warm
  link_arm thirdparty fetch-alone q60 "weights:$minio" || return 0
  link_run fetch run "$LINK_SCRIPT/fetch" "url=$url" --on local
  link_end fetch
  link_arm_end
  link_arm thirdparty ahead-alone q60 "weights:$minio" || return 0
  link_run ahead model install "$PAGEBOX" --on local
  link_end ahead
  link_arm_end
  link_arm thirdparty both q60 "weights:$minio" || return 0
  link_run ahead model install "$PAGEBOX" --on local
  link_until weights $((100 << 20)) 120 || check "link thirdparty: pagebox moved under 100 MiB in 120 s"
  link_run fetch run "$LINK_SCRIPT/fetch" "url=$url" --on local
  link_end fetch ahead
  link_arm_end
  link_judge thirdparty
}

# 8. Cancel: a run whose e2e/link-a (600 MiB) is a third in is canceled; the kernel's counts every 0.1 s until 20 s after.
link_cancel() {
  [ "$RUNNER_OS" = Linux ] || return 0
  local minio form n
  minio=$(link_minio)
  for form in q60 open; do
    LINK_INTERVAL=0.1 link_arm cancel "$form" "$form" "weights:$minio" || continue
    link_run read run "$LINK_SCRIPT/read" --on local
    link_until weights $((200 << 20)) 300 || check "link cancel $form: e2e/link-a moved under 200 MiB in 300 s"
    n=$(cat "$LINK_ARM/read.number")
    mark cancel_ms
    (cd "$RUN_PROJECT" && cozy --json run cancel "$(run_ref "$LINK_ARM" read "$n")") > "$LINK_ARM/cancel.json" 2>&1 || check "link cancel $form: run cancel refused: $(tail -c 300 "$LINK_ARM/cancel.json")"
    mark canceled_ms
    run_wait "$LINK_ARM" read || true
    sleep 20
    LINK_TAIL_S=0 link_arm_end
  done
  link_judge cancel
}

# 3. An update during downloads: pagebox a third in, the machine is updated (a settings update, which restarts TensorD as a software
# update does; a local update's files come from disk, so its bytes don't cross the link here: the pod session's). Alone, each.
link_update() {
  [ "$RUNNER_OS" = Linux ] || return 0
  local minio
  minio=$(link_minio)
  link_arm update weights-alone q60 "weights:$minio" || return 0
  link_run weights model install "$PAGEBOX" --on local
  link_end weights
  link_arm_end
  link_arm update update-alone q60 "weights:$minio" || return 0
  link_update_ask 6GiB
  link_arm_end
  link_arm update both q60 "weights:$minio" || return 0
  link_run weights model install "$PAGEBOX" --on local
  link_until weights $((530 << 20)) 300 || check "link update: pagebox moved under a third in 300 s"
  link_update_ask off
  link_end weights
  link_arm_end
  link_judge update
}

link_update_ask() {
  meta tensord_before "$(machine_pid || echo 0)"
  mark update_ms
  cozy --json machine update local --gpu-memory-limit "$1" -y > "$LINK_ARM/update.json" 2>&1 || check "link update: machine update refused: $(tail -c 300 "$LINK_ARM/update.json")"
  mark updated_ms
  machine_ready
  meta tensord_after "$(machine_pid || echo 0)"
}

# 4. Control traffic under a saturating download: a GET of the Hub's health each second through the link (the Hub's port is in
# it, so every Hub answer queues behind the weights, as on a pod's or a laptop's link), and `machine show local` each second;
# 20 s idle, then pagebox's 1.6 GB.
link_control() {
  [ "$RUNNER_OS" = Linux ] || return 0
  local minio hub form forms=${LINK_FORMS:-q60 p60 q60c}
  minio=$(link_minio) hub=$(link_hub)
  [ -n "$hub" ] || { check "link control: no Hub port in COZY_TEST_HUB_HEALTH ($COZY_TEST_HUB_HEALTH)"; return 0; }
  # cubic is the runner's own: q60c would be q60 again.
  [ "$(sysctl -n net.ipv4.tcp_congestion_control)" != cubic ] || forms=${forms/q60c/}
  for form in $forms; do
    LINK_PROBE=1 link_arm control "$form" "$form" "weights:$minio" "hub:$hub" || continue
    sleep 20
    mark idle_to_ms
    link_run weights model install "$PAGEBOX" --on local
    link_end weights
    LINK_PROBE=1 link_arm_end
  done
  link_judge control
}

# 6. Two projects on one machine: each runs a call needing its own 600 MiB (e2e/link-a, e2e/link-b), asked 1 s apart; each alone.
# Each project's downloads as its own Status lists them (a poller in each).
link_two() {
  [ "$RUNNER_OS" = Linux ] || return 0
  local minio main=$RUN_PROJECT p1=$OUT/link-two-p1 p2=$OUT/link-two-p2
  minio=$(link_minio)
  run_project "$p1" && run_project "$p2" || { RUN_PROJECT=$main; check "link two: its projects weren't made"; return 0; }
  RUN_PROJECT=$main
  link_arm two a-alone q60 --poll "$p1" "weights:$minio" || return 0
  link_two_run a "$p1" e2e/link-a
  link_end a
  link_arm_end
  link_arm two b-alone q60 --poll "$p2" "weights:$minio" || return 0
  link_two_run b "$p2" e2e/link-b
  link_end b
  link_arm_end
  link_arm two both q60 --poll "$p1" --poll "$p2" "weights:$minio" || return 0
  link_two_run a "$p1" e2e/link-a
  sleep 1
  link_two_run b "$p2" e2e/link-b
  link_end a b
  link_arm_end
  link_judge two
}

link_two_run() {
  local name=$1 project=$2 model=$3
  RUN_PROJECT=$project link_run "$name" run "$LINK_SCRIPT/read" --model "weights=$model@0.1.0" --on local
  echo "$project" > "$LINK_ARM/$name.at"
}

# 7. A slow disk: the store on its own loop disk (direct I/O to its file, so no second page cache), its writes capped at 20 MB/s for
# this user's services (cgroup io.max, systemd's IOWriteBandwidthMax) while the link is open: pagebox's 1.6 GB. Uncapped, the same.
# The kernel's counts say what was fetched (the weights class), written by TensorD (its wchar) and written to the disk (diskstats).
link_disk() {
  [ "$RUNNER_OS" = Linux ] || return 0
  local minio img=$RUNNER_TEMP/link-disk.img mnt=/mnt/link-disk dev unit=user@$(id -u).service
  minio=$(link_minio)
  truncate -s 6G "$img" && dev=$(sudo losetup --find --show --direct-io=on "$img") && sudo mkfs.ext4 -q -F -m 0 "$dev" \
    && sudo mkdir -p "$mnt" && sudo mount "$dev" "$mnt" && sudo chown "$(id -u):$(id -g)" "$mnt" \
    || { check "link disk: the loop disk was not made"; return 0; }
  LINK_STORE_DIR=$mnt/store link_arm disk uncapped open --disk "$dev" "weights:$minio" || return 0
  meta disk_cap_mb_s null
  link_run weights model install "$PAGEBOX" --on local
  link_end weights
  link_arm_end
  sudo systemctl set-property --runtime "$unit" "IOWriteBandwidthMax=$dev 20M" || check "link disk: io.max wasn't set"
  LINK_STORE_DIR=$mnt/store link_arm disk capped open --disk "$dev" "weights:$minio" || return 0
  meta disk_cap_mb_s 20
  meta io_max "\"$(cat "/sys/fs/cgroup/user.slice/user-$(id -u).slice/$unit/io.max" 2> /dev/null | tr '\n' ' ')\""
  link_run weights model install "$PAGEBOX" --on local
  link_end weights
  link_arm_end
  sudo systemctl set-property --runtime "$unit" "IOWriteBandwidthMax=" || true
  link_store disk-after
  sudo umount "$mnt" && sudo losetup -d "$dev" && rm -f "$img" || check "link disk: the loop disk wasn't removed"
  link_judge disk
}
