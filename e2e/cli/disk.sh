# tfs-150 (11-15): a pod whose disk held idle models' weights failed fibo-2's run with `DiskFullError: … the disk is full`
# instead of evicting them. Here the local machine's store is a 2.5 GiB disk, 1 GiB of it TensorFS's reserve: a run of each of
# three models (384 MiB of random weights each, e2e/isolated.sh's `disk`) fills it, and their executors go idle. Then three
# runs at once, each reading a model not here, as a session queues them, and one more of the first model after them: each
# completes, the idle models' weights evicted, least recently used first, to make its room.

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

step "tfs-150: three runs at once, each reading a model not here, then the first model again"
for i in 4 5 6; do
  run_submit "$OUT" "disk-$i" run "$script" --model "weights=e2e/disk-$i@0.1.0" text=$i --on local > /dev/null
done
for i in 4 5 6; do
  run_wait "$OUT" "disk-$i" || check "disk: disk-$i did not end: $(tail -c 300 "$OUT/disk-$i.ndjson")"
  [ "$(run_get "$OUT" "disk-$i" .status)" = completed ] \
    || check "disk: disk-$i (e2e/disk-$i) $(run_get "$OUT" "disk-$i" .status): $(run_get "$OUT" "disk-$i" .reason.message | head -c 300)"
done
cell disk-1-again "$script" --model "weights=e2e/disk-1@0.1.0" text=again || true
df -B1 --output=used,avail "$mnt" | tail -1 | sed "s/^/after: used, free: /"
cozy machine logs local --lines 0 --since 1h > "$OUT/disk.tensord.log" 2>&1 || check "disk: machine logs local failed"
grep -E "evicted|freed|fetch refused .*CAPACITY|disk is full|making room|disk was short" "$OUT/disk.tensord.log" | cut -c1-300 | head -40 || true
for name in disk-4 disk-5 disk-6 disk-1-again; do
  jq -r --arg n "$name" '.warnings[]? | "\($n) warning: \(.message)"' "$OUT/$name.json" 2> /dev/null | cut -c1-300 || true
done
}
