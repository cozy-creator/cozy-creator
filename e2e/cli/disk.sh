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
# The disk at its reserve, three runs at once each reading a model not here, five rounds: each round's models evicted the round
# before, so every round's admits race each other's evictions (43982726077: one refused 0.2 MiB short with an idle model left).
# tfs-302 (44054015865): a run's model, resolved, was evicted for another run's room before its fetch held it. tfs-303
# (44054038558): an executor that ran disk-1 and runs disk-5 kept both, and a run was refused disk-1's room; it lets go now.
local round set i
for round in 1 2 3 4 5; do
  if [ $((round % 2)) = 1 ]; then set="4 5 6"; else set="1 2 3"; fi
  for i in $set; do run_submit "$OUT" "race-$round-$i" run "$script" --model "weights=e2e/disk-$i@0.1.0" text=race$round --on local > /dev/null; done
  for i in $set; do
    run_wait "$OUT" "race-$round-$i" && [ "$(run_get "$OUT" "race-$round-$i" .status)" = completed ] \
      || check "disk: round $round's disk-$i $(run_get "$OUT" "race-$round-$i" .status): $(run_get "$OUT" "race-$round-$i" .reason.message | head -c 300)"
  done
done
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
grep -E "evicted|freed|fetch refused .*CAPACITY|disk is full|making room|short for this render|ended the idle|let go of|doesn't fit|held it|released construction" "$OUT/disk.tensord.log" | cut -c1-300 | head -60 || true
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
