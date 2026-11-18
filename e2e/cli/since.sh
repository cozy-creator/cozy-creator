# e2e/cli/since.sh: a machine log's time window is cut on the machine (cl-1216: Paul's 26 MiB video came from a rental at 1.2 MB/s
# while another session read that machine's whole 16 MB log through the same Calcifer, and `--lines 0 --since 5m` moved the whole
# log). The local TensorD's log gets, in one append, 4 MB of random lines stamped an hour back (gzip can't shrink them), then ten
# marker lines before a cut and ten after it. `--since` returns exactly the ten after, and what TensorD sends Calcifer for them is
# about their size, not the log's. Linux, the runner's own machine; its own shard (`log-since`).
log_since() (
step "a log's time window is cut on the machine: exactly its lines, and only they, cross (cl-1216)"
[ "$RUNNER_OS" = Linux ] || { echo "log since: the runner's own machine is Linux's"; return 0; }
local root=$HOME/.cozy/machine/var/lib/cozy log port wrote at run filler wire text
machine_ready
log=$(ls -t "$root"/machine/logs/tensord.*.log 2> /dev/null | head -1)
port=$(jq -r '.address // empty' "$root/tensord/api-ready.json" 2> /dev/null); port=${port##*:}
[ -n "$log" ] && [ -n "$port" ] || { check "log since: no TensorD log ('$log') or address ('$port') under $root"; return; }
# Stamped ahead of the write, so no line TensorD wrote before it is in the window: before the cut 2-6 s on, after it 14-18 s on.
wrote=$(python3 - "$log" "$OUT/since-expected.txt" <<'PY'
import os, secrets, sys, time
from datetime import datetime, timezone

def at(t):
    return datetime.fromtimestamp(t, timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")

now, run = time.time(), secrets.token_hex(4)
filler = "".join(f"{at(now - 3600 + i / 1000)}  INFO cl1216: {run} filler {secrets.token_hex(48)}\n" for i in range(4_000_000 // 160))
before = "".join(f"{at(now + 2 + i * 0.4)}  INFO cl1216: {run} before {i}\n" for i in range(10))
after = "".join(f"{at(now + 14 + i * 0.4)}  INFO cl1216: {run} after {i}\n" for i in range(10))
block = (filler + before + after).encode()
fd = os.open(sys.argv[1], os.O_WRONLY | os.O_APPEND)
written = os.write(fd, block)
os.close(fd)
if written != len(block):
    sys.exit(f"one append wrote {written} of {len(block)} B")
open(sys.argv[2], "w").write(after)
print(now, run, len(filler))
PY
) || { check "log since: the lines weren't appended: $wrote"; return; }
read -r at run filler <<< "$wrote"
# The read 30 s after the cut (10 s after the write), asking for the last 30 s.
sleep "$(python3 -c "import time; print(max(0.0, $at + 40 - time.time()))")"
ss -tinH dst "127.0.0.1:$port" > "$OUT/since.ss-before"
cozy machine logs local --lines 0 --since 30s > "$OUT/since.txt" 2> "$OUT/since.err" || { check "log since: the read: $(tail -c 300 "$OUT/since.err")"; return; }
ss -tinH dst "127.0.0.1:$port" > "$OUT/since.ss-after"
wire=$(python3 - "$OUT/since.ss-before" "$OUT/since.ss-after" <<'PY'
import re, sys

def received(path):
    found, port = {}, None
    for line in open(path):
        if line.strip() and not line[0].isspace():
            port = line.split()[3].rsplit(":", 1)[-1]
        elif port and (got := re.search(r"bytes_received:(\d+)", line)):
            found[port] = int(got[1])
    return found

was, now = received(sys.argv[1]), received(sys.argv[2])
print(sum(max(n - was.get(p, 0), 0) for p, n in now.items()))
PY
)
text=$(stat -c %s "$OUT/since.txt")
echo "log since: ${filler} B before the cut, read with --since 30s: $text B of text, $wire B from TensorD to Calcifer"
grep -F "cl1216: $run " "$OUT/since.txt" > "$OUT/since-got.txt" || true
cmp -s "$OUT/since-got.txt" "$OUT/since-expected.txt" \
  || check "log since: not exactly the 10 lines after the cut: $(grep -c ' before ' "$OUT/since-got.txt") before it, $(grep -c ' filler ' "$OUT/since-got.txt") filler, $(grep -c ' after ' "$OUT/since-got.txt") after"
[ -n "$wire" ] && [ "$wire" -lt 524288 ] || check "log since: TensorD sent ${wire:-an uncounted number of} B for a $text B window: the cut happened after the log crossed"
)
