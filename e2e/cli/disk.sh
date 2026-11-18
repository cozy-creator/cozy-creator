# tfs-150 (11-15): a pod whose disk held idle models' weights failed fibo-2's run with `DiskFullError: … the disk is full`
# instead of evicting them. Here the local machine's store is a 2.5 GiB disk, 1 GiB of it TensorFS's reserve: a run of each of
# three models (384 MiB of random weights each, e2e/isolated.sh's `disk`) fills it, and their executors go idle. Then three
# runs at once, each reading a model not here, as a session queues them, while a reader holds the store's fence for 15 s (as
# the pod's checkpoint opens and holds each did for a moment): the machine's room for their weights waits for it, never
# refused `model data is in use` (#2858 made it refuse; 0.25.0 waited). Then one more of the first model. Each completes,
# the idle models' weights evicted, least recently used first, to make its room. Last, a 2 GiB model, more than the disk can
# make free, is refused `machine_disk_full` naming the sizes, nothing of it kept.

# disk_store <config.toml> <dir>: config.toml with `machine.store` at <dir>.
disk_store() {
  python3 - "$1" "$2" <<'PY'
import re, sys
path, store = sys.argv[1], sys.argv[2]
try:
    text = open(path).read()
except FileNotFoundError:
    text = ""
text = re.sub(r"(?m)^store = .*\n", "", text)
line = f'store = "{store}"\n'
text = re.sub(r"(?m)^\[machine\]\n", lambda m: m.group(0) + line, text, count=1) if re.search(r"(?m)^\[machine\]$", text) else text.rstrip("\n") + f"\n\n[machine]\n{line}"
open(path, "w").write(text)
PY
}

disk_full() {
[ "$RUNNER_OS" = Linux ] || return 0
local script=$here/script/disk.py img=$RUNNER_TEMP/tfs-disk.img mnt=/mnt/tfs-disk name i used
step "tfs-150: the machine's store on a 2.5 GiB disk, filled with three idle models"
truncate -s 2560M "$img" && mkfs.ext4 -q -F -m 0 "$img" && sudo mkdir -p "$mnt" && sudo mount -o loop "$img" "$mnt" \
  && sudo chown "$(id -u):$(id -g)" "$mnt" || { check "disk: the 2.5 GiB disk was not made"; return 0; }
disk_store "$HOME/.cozy/config.toml" "$mnt"
machine_stop || check "disk: the local machine did not stop"
machine_ready
for i in 1 2 3; do
  cell "disk-$i" "$script" --model "weights=e2e/disk-$i@0.1.0" text=$i || return 0
  df -B1 --output=used,avail "$mnt" | tail -1 | sed "s/^/disk-$i: used, free: /"
done
used=$(df -B1 --output=used "$mnt" | tail -1 | tr -d ' ')
[ "$used" -ge $((3 * 384 << 20)) ] || { check "disk: the store's disk holds $used bytes after three 384 MiB models: the machine does not fill it"; return 0; }

step "tfs-150: three runs at once, each reading a model not here, while a reader holds the store, then the first model again"
# A reader between reading the store's records and holding what they name holds `holds/gc.lock` shared (retention.rs).
python3 -c 'import fcntl, sys, time
fence = open(sys.argv[1], "a+")
fcntl.flock(fence, fcntl.LOCK_SH)
open(sys.argv[2], "w").write(str(time.time()))
time.sleep(15)' "$mnt/holds/gc.lock" "$OUT/disk-reader.held" &
local reader=$!
for _ in $(seq 50); do [ -s "$OUT/disk-reader.held" ] && break; sleep 0.2; done
[ -s "$OUT/disk-reader.held" ] || check "disk: the reader did not hold the store's fence"
for i in 4 5 6; do
  run_submit "$OUT" "disk-$i" run "$script" --model "weights=e2e/disk-$i@0.1.0" text=$i --on local > /dev/null
done
for i in 4 5 6; do
  run_wait "$OUT" "disk-$i" || check "disk: disk-$i did not end: $(tail -c 300 "$OUT/disk-$i.ndjson")"
  [ "$(run_get "$OUT" "disk-$i" .status)" = completed ] \
    || check "disk: disk-$i (e2e/disk-$i) $(run_get "$OUT" "disk-$i" .status): $(run_get "$OUT" "disk-$i" .reason.message | head -c 300)"
done
wait "$reader" || check "disk: the reader holding the store failed"
cell disk-1-again "$script" --model "weights=e2e/disk-1@0.1.0" text=again || true
step "tfs-150: a model bigger than the disk can make free is refused, naming the sizes, nothing of it kept"
local before said
before=$(df -B1 --output=used "$mnt" | tail -1 | tr -d ' ')
ALLOW=machine_disk_full cell disk-big "$script" --model "weights=e2e/disk-big@0.1.0" text=big || true
said=$(run_get "$OUT" disk-big '.reason.message')
echo "disk-big: $(run_get "$OUT" disk-big .status): ${said:0:400}"
{ [ -f "$OUT/disk-big.refused" ] && grep -qE '[0-9.]+(GB|MB)' <<< "$said"; } \
  || check "disk: e2e/disk-big (2 GiB on a 2.5 GiB disk) was not refused machine_disk_full naming the sizes: ${said:0:300}"
[ "$(df -B1 --output=used "$mnt" | tail -1 | tr -d ' ')" -le $((before + (64 << 20))) ] || check "disk: the refused e2e/disk-big left bytes on the disk"
df -B1 --output=used,avail "$mnt" | tail -1 | sed "s/^/after: used, free: /"
cozy machine logs local --lines 0 --since 1h > "$OUT/disk.tensord.log" 2>&1 || check "disk: machine logs local failed"
grep -E "evicted|freed|fetch refused .*CAPACITY|disk is full|making room|short for this render|ended the idle|doesn't fit|held it" "$OUT/disk.tensord.log" | cut -c1-300 | head -60 || true
for name in disk-4 disk-5 disk-6 disk-1-again; do
  jq -r --arg n "$name" '.warnings[]? | "\($n) warning: \(.message)"' "$OUT/$name.json" 2> /dev/null | cut -c1-300 || true
  ! jq -e '[.warnings[]?.message] | any(test("model data is in use"))' "$OUT/$name.json" > /dev/null 2>&1 \
    || check "disk: $name's room was refused while a reader held the store, not waited for"
done
grep "making room in $mnt waited" "$OUT/disk.tensord.log" | cut -c1-200 \
  || check "disk: the machine's room for the runs' weights never waited for the reader"
disk_below_reserve
disk_run_object
}

# cl-1076 (11-16): kamitsure's disk sat at its reserve for an hour while an idle LTX executor held 71.5 GB open, and its
# outputs went instead. Below its reserve the machine ended idle executors only for the bytes coming in, never for the
# reserve too: it ended one, whose model alone couldn't make the room, and so evicted nothing. Here four idle executors
# (one script each) hold a 384 MiB model apiece on a fresh 4 GiB store disk (1 GiB reserve), the disk 576 MiB below its
# reserve. A run of a fifth model completes: the idle executors it needs end, their models evicted.
disk_below_reserve() {
local script=$here/script/disk.py img=$RUNNER_TEMP/tfs-reserve.img mnt=/mnt/tfs-reserve dir=$OUT/cl1076 i avail size reserve
step "cl-1076: a disk below its reserve evicts as many idle executors' models as it lacks"
truncate -s 4096M "$img" && mkfs.ext4 -q -F -m 0 "$img" && sudo mkdir -p "$mnt" && sudo mount -o loop "$img" "$mnt" \
  && sudo chown "$(id -u):$(id -g)" "$mnt" || { check "disk: the 4 GiB disk was not made"; return 0; }
disk_store "$HOME/.cozy/config.toml" "$mnt"
machine_stop || check "disk: the local machine did not stop"
machine_ready
mkdir -p "$dir"
cell reserve-1 "$script" --model "weights=e2e/disk-1@0.1.0" text=1 || return 0
for i in 2 3 4; do
  sed "s/disk filler/disk filler $i/" "$script" > "$dir/disk$i.py"
  cell "reserve-$i" "$dir/disk$i.py" --model "weights=e2e/disk-$i@0.1.0" text=$i || return 0
done
read -r size avail < <(df -B1 --output=size,avail "$mnt" | tail -1)
reserve=$((size / 50)); [ "$reserve" -ge $((1 << 30)) ] || reserve=$((1 << 30))
fallocate -l $((avail - reserve + (576 << 20))) "$mnt/cl-1076-filler" || { check "disk: the filler was not made"; return 0; }
df -B1 --output=used,avail "$mnt" | tail -1 | sed "s/^/below the reserve: used, free: /"
cell reserve-5 "$script" --model "weights=e2e/disk-5@0.1.0" text=5 || true
jq -r '.warnings[]?.message' "$OUT/reserve-5.json" 2> /dev/null | cut -c1-300 || true
cozy machine logs local --lines 0 --since 15m 2>&1 | grep -E "ended the idle|evicted|doesn't fit" | cut -c1-300 | tail -8 || true
rm -f "$mnt/cl-1076-filler"
}

# cl-1076 (11-16): on a 2x A40 rental H3's 68.87 GiB could not go for LTX 2.5 once H3 had run from source (pod-session
# 43875722356). minimax-h3 carries the Qwen3-VL processor files H3's checkpoint holds too. A machine keeps a run's source
# files, each under its own digest, and holds them for 10 minutes after the run; the store took a file both named for part
# of the model, so the machine's hold on the file held the whole model. Here a stand-in machine (another user's cozy on
# this runner: this computer's own runs keep their files in their project, not in the machine's store) has its store on a
# 2.5 GiB disk, 1 GiB of it the reserve. e2e-tokenizer runs there from source: it carries the vocab.json its model
# e2e/disk-tokenizer holds as an asset (isolated.sh's `disk`), and its executor goes idle. A file then leaves 32 MiB free
# past the reserve, and a run of another 384 MiB model completes: the idle executor ended, e2e/disk-tokenizer evicted.
disk_run_object() {
step "cl-1076: a model holding a file its package carries too is evicted for a render's room"
[ "${GITHUB_ACTIONS:-}" = true ] || { echo "disk: a run's file: its stand-in is another user of a fresh GitHub runner"; return 0; }
local name=stand-d script=$here/script/disk.py img=$RUNNER_TEMP/tfs-stand.img mnt=/mnt/tfs-stand file size used avail reserve filler digest uses
file=$here/tokenizer/e2e_tokenizer/processor/vocab.json
id "cozy-$name" > /dev/null 2>&1 || sudo useradd -m "cozy-$name" || { check "disk: $name: no user"; return 0; }
truncate -s 2560M "$img" && mkfs.ext4 -q -F -m 0 "$img" && sudo mkdir -p "$mnt" && sudo mount -o loop "$img" "$mnt" \
  && sudo chown "cozy-$name:" "$mnt" || { check "disk: $name's 2.5 GiB disk was not made"; return 0; }
sudo -u "cozy-$name" -H bash -c 'mkdir -p ~/.cozy && printf "[machine]\nstore = \"%s\"\n" "$1" > ~/.cozy/config.toml' _ "$mnt" \
  || { check "disk: $name's store was not set"; return 0; }
cozy --json machine show local > "$OUT/set-local.json" || { check "disk: machine show local: $(head -c 300 "$OUT/set-local.json")"; return 0; }
stand_in "$name" || return 0
if ! run_cell "$OUT" object-1 "$here/tokenizer/read" text=1 --on "$name"; then
  check "disk: e2e-tokenizer on $name: $(run_get "$OUT" object-1 .reason.message | head -c 300) $(tail -c 300 "$OUT/object-1.err" 2> /dev/null)"
  timeout 60 "$COZY" --json machine end "$name" --force -y > "$OUT/$name-end.json" 2>&1 || true
  return 0
fi
read -r size used avail < <(df -B1 --output=size,used,avail "$mnt" | tail -1)
echo "object-1: run $(cat "$OUT/object-1.number") completed; $name's disk: used $used, free $avail"
[ "$used" -ge $((384 << 20)) ] || check "disk: $name's disk holds $used bytes after e2e/disk-tokenizer: its store isn't there"
# The file as the store names it: by the model's uses and by its own, the run's; and the processes holding it.
digest=$(sha256sum "$file" | cut -d' ' -f1)
uses=$(sudo grep -l "$digest" -r "$mnt/uses" 2> /dev/null | wc -l || true)
echo "vocab.json ${digest:0:12}: named by $uses uses; held by: $(sudo grep -l "$digest" -r "$mnt/holds" 2> /dev/null | while read -r f; do f=${f##*/}; ps -o pid=,args= -p "${f%%-*}" | cut -c1-70; done | sort -u | paste -sd ';' || true)"
[ "${uses:-0}" -ge 2 ] || check "disk: vocab.json is named by ${uses:-0} uses on $name, not by the model's and the run's own: the cell can't tell"
reserve=$((size / 50)); [ "$reserve" -ge $((1 << 30)) ] || reserve=$((1 << 30))
filler=$((avail - reserve - (32 << 20)))
sudo fallocate -l "$filler" "$mnt/cl-1076-filler" || { check "disk: $name's filler was not made"; return 0; }
df -B1 --output=used,avail "$mnt" | tail -1 | sed "s/^/32 MiB past the reserve: used, free: /"
if run_cell "$OUT" object-2 "$script" --model "weights=e2e/disk-2@0.1.0" text=2 --on "$name"; then
  used=$(df -B1 --output=used "$mnt" | tail -1 | tr -d ' ')
  echo "object-2: run $(cat "$OUT/object-2.number") completed; $name's disk holds $((used - filler)) bytes beside the filler"
  [ $((used - filler)) -lt $((2 * 384 << 20)) ] || check "disk: e2e/disk-2 landed on $name beside e2e/disk-tokenizer: the cell can't tell"
else
  check "disk: e2e/disk-2 on $name was not given the room of e2e/disk-tokenizer, idle, which holds a file the package that ran it carries: $(run_get "$OUT" object-2 .reason.message | head -c 500) $(tail -c 200 "$OUT/object-2.err" 2> /dev/null)"
fi
jq -r '.warnings[]? | "object-2 warning: \(.message)"' "$OUT/object-2.json" 2> /dev/null | cut -c1-400 || true
cozy machine logs "$name" --lines 0 --since 15m 2>&1 | grep -E "ended the idle|evicted|doesn't fit|doesn't have room" | cut -c1-400 | tail -8 || true
echo "$name holds: $(cozy --json machine show "$name" 2> /dev/null | jq -r '[.status.models[]? | select(any(.checkpoints[]?; .complete)) | .repository] | sort | join(" ")' || true)"
sudo rm -f "$mnt/cl-1076-filler"
timeout 60 "$COZY" --json machine end "$name" --force -y > "$OUT/$name-end.json" 2>&1 || true
}

# cl-1235 (11-18, the #3810 review): Status counted a model as one a download may take only when no process held it, while the machine
# itself (room_for) also takes one only idle executors hold, ending them. So a call that fits once a warm model goes was refused
# `machine_disk_full` by its quote and never sent: a rental with Qwen-Image warm waited "Needs N more" for H3 until it released idle.
# Here a stand-in machine (another user's cozy on this runner) has its store on a 6 GiB disk, 1 GiB of it the reserve, and a file
# leaves 1900 MiB past the reserve once e2e/disk-warm (1.5 GiB) lands. disk.py runs e2e/disk-warm there; its executor goes idle,
# holding it. Status's room then counts it, and `cozy run` of e2e-roomless/still (published, so quoted), whose e2e/disk-big is 2 GiB,
# goes there and completes, e2e/disk-warm evicted for it and named in its warning. Before, it was refused `machine_disk_full`.
disk_freeable() {
[ "$RUNNER_OS" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "disk freeable: its stand-in is another user of a fresh GitHub runner"; return 0; }
local name=stand-f script=$here/script/disk.py img=$RUNNER_TEMP/tfs-free.img mnt=/mnt/tfs-free size avail reserve filler room=0 i said
step "cl-1235: a call that fits once an idle executor's model goes is sent there, that model evicted for it"
id "cozy-$name" > /dev/null 2>&1 || sudo useradd -m "cozy-$name" || { check "disk freeable: $name: no user"; return 0; }
truncate -s 6144M "$img" && mkfs.ext4 -q -F -m 0 "$img" && sudo mkdir -p "$mnt" && sudo mount -o loop "$img" "$mnt" \
  && sudo chown "cozy-$name:" "$mnt" || { check "disk freeable: $name's 6 GiB disk was not made"; return 0; }
sudo -u "cozy-$name" -H bash -c 'mkdir -p ~/.cozy && printf "[machine]\nstore = \"%s\"\n" "$1" > ~/.cozy/config.toml' _ "$mnt" \
  || { check "disk freeable: $name's store was not set"; return 0; }
cozy --json machine show local > "$OUT/set-local.json" || { check "disk freeable: machine show local: $(head -c 300 "$OUT/set-local.json")"; return 0; }
stand_in "$name" || return 0
read -r size avail < <(df -B1 --output=size,avail "$mnt" | tail -1)
reserve=$((size / 50)); [ "$reserve" -ge $((1 << 30)) ] || reserve=$((1 << 30))
# Once e2e/disk-warm landed, 148 MiB short of e2e/disk-big's 2 GiB: e2e/disk-warm's 1.5 GiB covers it with room to spare, as a
# download's objects in flight take room before they land (a 192 MiB spare was refused mid-download, e2e 44284650344).
filler=$((avail - reserve - ((1900 + 1536) << 20)))
sudo fallocate -l "$filler" "$mnt/cl-1235-filler" || { check "disk freeable: $name's filler was not made"; return 0; }
if ! run_cell "$OUT" free-warm "$script" --model "weights=e2e/disk-warm@0.1.0" text=warm --on "$name"; then
  check "disk freeable: e2e/disk-warm on $name: $(run_get "$OUT" free-warm .reason.message | head -c 300) $(tail -c 300 "$OUT/free-warm.err" 2> /dev/null)"
  timeout 60 "$COZY" --json machine end "$name" --force -y > "$OUT/$name-end.json" 2>&1 || true
  return 0
fi
df -B1 --output=used,avail "$mnt" | tail -1 | sed "s/^/free-warm: run $(cat "$OUT/free-warm.number") completed; $name's disk used, free: /"
# Its executor idle, Status's room counts e2e/disk-warm: measured again when the idle executors change, not at the next sweep.
for i in $(seq 60); do
  cozy --json machine show "$name" > "$OUT/$name-show.json" 2> /dev/null || true
  room=$(jq -r '.status.disk_room_bytes // 0' "$OUT/$name-show.json" 2> /dev/null || echo 0)
  [ "${room:-0}" -le $((2048 << 20)) ] || break
  sleep 1
done
echo "disk freeable: $name's room after ${i}s: $((${room:-0} >> 20)) MiB; $(jq -c '.status | {disk_free_bytes, disk_room_bytes, disk_needs_bytes}' "$OUT/$name-show.json" 2> /dev/null)"
if run_cell "$OUT" free-big e2e-roomless/still instructions=freeable --on "$name"; then
  said=$(run_get "$OUT" free-big '[.warnings[]?.message] | join(" ")')
  echo "free-big: run $(cat "$OUT/free-big.number") completed on $(run_get "$OUT" free-big .machine): ${said:0:400}"
  grep -q 'e2e/disk-warm' <<< "$said" || check "disk freeable: the run's warning doesn't name e2e/disk-warm, evicted for it: ${said:0:400}"
else
  said=$(cat "$OUT/free-big.err" "$OUT/free-big.ndjson" 2> /dev/null | tr '\n' ' ')
  check "disk freeable: e2e-roomless/still on $name, whose idle executor's e2e/disk-warm covers what its disk lacks, didn't run: $(run_get "$OUT" free-big .reason.message | head -c 300) ${said:0:500}"
fi
cozy machine logs "$name" --lines 0 --since 15m 2>&1 | grep -E "ended the idle|evicted|doesn't have room|short for" | cut -c1-400 | tail -8 || true
sudo rm -f "$mnt/cl-1235-filler"
timeout 60 "$COZY" --json machine end "$name" --force -y > "$OUT/$name-end.json" 2>&1 || true
}

# cl-1175 (11-17), Paul: "if the user's disc space available is really small, we should probably warn them and stop them from trying to
# download models they cannot fit, instead telling them to free up disc-space or run on another machine + rent another machine".
# This computer's store is a 2.5 GiB disk, 1 GiB of it TensorFS's reserve, and e2e-roomless/still reads e2e/disk-big, 2 GiB, which has
# no room there (e2e/isolated.sh's `room`). `cozy run` of it is refused before anything goes to this computer's machine, saying how
# much room it lacks and the rental instead (--on new). A project's box of it, rendered, waits with nothing sent, its board, queue and
# box saying how much room it lacks (the page: 1440×900 and 1920×1080, paper and dark). Then stand-r, another user's cozy on this
# runner's own disk, which has room, is added: the waiting render goes there by itself and lands, and `cozy run` goes there too, each
# saying in a line that this computer's disk was short. Nothing of e2e/disk-big lands on the small disk. Before, both went to this
# computer's machine, which took them, installed the tool and failed machine_disk_full.
disk_room() {
[ "$RUNNER_OS" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "disk room: its small disk and its stand-in are a fresh GitHub runner's"; return 0; }
local img=$RUNNER_TEMP/room.img mnt=/mnt/room P=$OUT/room tool=e2e-roomless/still name=stand-r pw=$OUT/playwright before said job run chrome code=0
step "cl-1175: this computer's store on a 2.5 GiB disk, without room for e2e-roomless/still's 2 GiB model"
truncate -s 2560M "$img" && mkfs.ext4 -q -F -m 0 "$img" && sudo mkdir -p "$mnt" && sudo mount -o loop "$img" "$mnt" \
  && sudo chown "$(id -u):$(id -g)" "$mnt" || { check "disk room: the 2.5 GiB disk was not made"; return 0; }
disk_store "$HOME/.cozy/config.toml" "$mnt"
machine_stop || check "disk room: the local machine did not stop"
machine_ready
before=$(df -B1 --output=used "$mnt" | tail -1 | tr -d ' ')
cozy --json machine show local > "$OUT/room-local.json" 2>&1 || true
echo "disk room: local's disk as Calcifer has it: $(jq -c '.status | {disk_total_bytes, disk_free_bytes, disk_room_bytes, disk_needs_bytes}' "$OUT/room-local.json" 2> /dev/null)"

step "cl-1175: cozy run of it is refused before it goes to this computer's machine, saying the room it lacks and the rental"
cozy --json new --unencrypted "$P" > "$OUT/room-new.json" 2>&1 || { check "disk room: new $P: $(tail -c 300 "$OUT/room-new.json")"; return 0; }
# Naming no machine: where its project's runs go.
in_project "$P" --json run "$tool" instructions=room > "$OUT/room-run.json" 2> "$OUT/room-run.err" || code=$?
said=$(cat "$OUT/room-run.json" "$OUT/room-run.err" 2> /dev/null | tr '\n' ' ')
echo "disk room: cozy run exited $code: ${said:0:600}"
[ "$code" != 0 ] && grep -q machine_disk_full <<< "$said" && grep -qE 'This computer needs [0-9.]+(MB|GB) more disk space for its models' <<< "$said" \
  && grep -q -- '--on new' <<< "$said" \
  || check "disk room: cozy run of $tool wasn't refused saying how much room this computer lacks and the rental (--on new): ${said:0:500}"
in_project "$P" --json run ls --limit 0 > "$OUT/room-runs.json" 2>&1 || true
! J '[.runs[]? | select(.target | test("e2e-roomless"))] | length > 0' "$OUT/room-runs.json" \
  || check "disk room: a run of $tool went to a machine: $(jq -c '[.runs[] | select(.target | test("e2e-roomless")) | {number, machine, status, reason: .reason.message}]' "$OUT/room-runs.json" | head -c 600)"

step "cl-1175: a project's box of it, rendered, waits with nothing sent, saying how much room this computer lacks"
jq -n --arg t "$tool" '[{name: "roomless", tool: $t, kind: "image", fields: {instructions: "A still this computer has no room for."}}]' | boxes_in "$P" || return 0
project_api "$P" /runs -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: room-$RANDOM$RANDOM" -d '{"names":["roomless"],"on":[]}' \
  > "$OUT/room-go.json" 2>&1 || check "disk room: render roomless: $(tail -c 300 "$OUT/room-go.json")"
for _ in $(seq 240); do
  status "$P" > "$OUT/room-status.json" 2> /dev/null || true
  J '[.queue[]? | select(.name == "roomless") | .no_room.line // "" | test("^This computer needs [0-9.]+(MB|GB) more disk space for its models")] | any' "$OUT/room-status.json" && break
  J '[.board.jobs[]? | select(.asset == "roomless") | .state] | any(. == "failed" or . == "done")' "$OUT/room-status.json" && break
  sleep 0.5
done
job=$(jq -c '[.board.jobs[]? | select(.asset == "roomless")] | last // empty' "$OUT/room-status.json" 2> /dev/null)
echo "disk room: roomless: $(jq -c '[.queue[]? | select(.name == "roomless")]' "$OUT/room-status.json" 2> /dev/null) ${job:-no job}"
J '[.queue[]? | select(.name == "roomless") | .no_room.line // "" | test("^This computer needs [0-9.]+(MB|GB) more disk space for its models")] | any' "$OUT/room-status.json" \
  || check "disk room: roomless's render doesn't wait for room: ${job:-no job on the board}"
[ -z "$(jq -r '.run // empty' <<< "${job:-null}")" ] || check "disk room: roomless's render was sent, to run $(jq -r .run <<< "$job"): $job"
J '[.board.said[]? | .message] | any(test("^roomless: This computer needs [0-9.]+(MB|GB) more disk space for its models"))' "$OUT/room-status.json" \
  || check "disk room: the render didn't say how much room this computer lacks: $(jq -c '[.board.said[]?.message]' "$OUT/room-status.json" 2> /dev/null | head -c 500)"
# The page, open on the project: its box says so, on the canvas and opened.
chrome=$(web_chrome)
if [ -n "$chrome" ] && [ -x "$chrome" ] && npm install -s --prefix "$pw" playwright-core@1.55.0 > "$OUT/room-npm.log" 2>&1; then
  HOLDER=$HOME/.cozy/calcifer/calcifer.lock.holder CHROME=$chrome PLAYWRIGHT=$pw/node_modules/playwright-core SHOT=$OUT/room-page LAST_LOOK=$OUT/room-page \
    node "$here/web/room.js" "$P" roomless > "$OUT/room-page.json" 2> "$OUT/room-page.err" || code=$?
  J '.failures | length == 0' "$OUT/room-page.json" 2> /dev/null \
    || check "disk room: the page: $(jq -c '{failures, seen}' "$OUT/room-page.json" 2> /dev/null | head -c 1500) $(tail -c 400 "$OUT/room-page.err")"
else
  check "disk room: no Google Chrome or playwright-core on this runner (${chrome:-no chrome}): $(tail -c 300 "$OUT/room-npm.log" 2> /dev/null)"
fi

step "cl-1175: stand-r, a computer with room, added: the waiting render goes there and lands, and so does cozy run, each saying why"
cozy --json machine show local > "$OUT/set-local.json" || { check "disk room: machine show local"; return 0; }
stand_in "$name" || return 0
# Quoted before any run of it ends there, which would say what it reads.
code=0
in_project "$P" run "$tool" instructions=elsewhere > "$OUT/room-run-2.out" 2> "$OUT/room-run-2.err" || code=$?
said=$(tr '\n' ' ' < "$OUT/room-run-2.err")
echo "disk room: cozy run with $name exited $code: ${said:0:400}"
[ "$code" = 0 ] || check "disk room: cozy run of $tool with $name there failed: ${said:0:500}"
grep -qE "This computer's disk is [0-9.]+(MB|GB) short for its models, so it runs on $name" <<< "$said" \
  || check "disk room: cozy run didn't say it went to $name for this computer's short disk: ${said:0:500}"
coming_done roomless 900
job=$(status "$P" | jq -c '[.board.jobs[]? | select(.asset == "roomless")] | last // empty')
run=$(jq -r '.run // empty' <<< "${job:-null}")
if [ -n "$run" ]; then
  echo "$P" > "$OUT/room-made.at"
  run_show "$OUT" room-made "$run"
  J --arg m "$name" '.machine == $m' "$OUT/room-made.json" || check "disk room: roomless's render went to $(run_get "$OUT" room-made .machine), not $name, which has room"
else
  check "disk room: roomless's render was never sent once $name had room: ${job:-no job}"
fi
status "$P" > "$OUT/room-status.json" 2> /dev/null || true
J --arg m "$name" '[.board.said[]? | .message] | any(test("^roomless: This computer.s disk is [0-9.]+(MB|GB) short for its models, so it renders on " + $m))' "$OUT/room-status.json" \
  || check "disk room: the render didn't say it went to $name for this computer's short disk: $(jq -c '[.board.said[]?.message]' "$OUT/room-status.json" 2> /dev/null | head -c 500)"
in_project "$P" --json run ls --limit 0 > "$OUT/room-runs.json" 2>&1 || true
J --arg m "$name" '[.runs[]? | select(.target | test("e2e-roomless"))] | length > 0 and all(.machine == $m)' "$OUT/room-runs.json" \
  || check "disk room: the runs of $tool: $(jq -c '[.runs[]? | select(.target | test("e2e-roomless")) | {number, machine, status}]' "$OUT/room-runs.json" | head -c 400)"
[ "$(df -B1 --output=used "$mnt" | tail -1 | tr -d ' ')" -le $((before + (64 << 20))) ] \
  || check "disk room: $(( ($(df -B1 --output=used "$mnt" | tail -1 | tr -d ' ') - before) >> 20 )) MiB landed on this computer's small disk"
# What placement weighed, for a red to read.
grep -h "e2e/room\|roomless\|e2e-roomless" "$HOME"/.cozy/calcifer/calcifer.log* 2> /dev/null | cut -c1-600 > "$OUT/room-calcifer.log" || true
cp "$P/.vault-calcifer/diagnostics/walk.log" "$OUT/room-walk.log" 2> /dev/null || true
timeout 60 "$COZY" --json machine end "$name" --force -y > "$OUT/$name-end.json" 2>&1 || true
}
