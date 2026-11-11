# hub_slow_upload (xs-172; from the Hub agent's throttled proof): a 704 MiB model upload by the machine whose objects reach
# the store over an 8 Mbit/s link, 16 streams sharing it. Sourced by e2e/release/synthetic.sh, inside e2e/skew/run.sh
# (HB, CLI, M, WHO, HUB_PKG, S3_PORT, TWF). Each PUT's request line and the store's answers are captured on loopback.
# Passes when the upload's run succeeds without its follower giving up (no exit 4) and no object needed more than two PUTs:
# a PUT that takes minutes is never cut at a fixed deadline, and a grant is never spent after it expired.
hub_slow_upload() {
  local f=$HB/w-slow.safetensors w=$HB/w-slow store=$HB/tfs-store-slow model=$WHO/${HUB_PKG}-slow t0 t1 code
  uv run -q --no-project --with numpy --with safetensors python -I - "$f" <<'PY' || { expect "hub slow-upload" FAIL "no weights"; return; }
import sys
import numpy as np
from safetensors.numpy import save_file
rng = np.random.default_rng()
save_file({f"w.{i}": rng.standard_normal((4096, 4096)).astype(np.float16) for i in range(22)}, sys.argv[1])
PY
  TENSORFS_HOME=$store uv tool run -q --from "${TWF:-tensorfs}" tfs import "$f" --converter as-is --name local/w-slow > /dev/null &&
    TENSORFS_HOME=$store uv tool run -q --from "${TWF:-tensorfs}" tfs export local/w-slow "$w" > /dev/null ||
    { expect "hub slow-upload" FAIL "tfs import/export failed"; return; }
  rm -rf "$f" "$store"
  log "slow upload: $(du -sh "$w" | cut -f1) of weights; the store's port $S3_PORT throttled to 8 Mbit/s"
  sudo tc qdisc add dev lo root handle 1: prio bands 3 &&
    sudo tc qdisc add dev lo parent 1:3 handle 30: tbf rate 8mbit burst 256kb latency 4s &&
    sudo tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip dport "$S3_PORT" 0xffff flowid 1:3 ||
    { expect "hub slow-upload" FAIL "could not throttle the store's port"; sudo tc qdisc del dev lo root 2> /dev/null; return; }
  sudo tcpdump -i lo -l -nn -tttt -A -s 4096 "tcp port $S3_PORT" 2> /dev/null |
    awk '/^[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9] /{ts=$1" "$2}
      match($0, /(PUT|GET) \/[^ ]*X-Amz-Expires[^ ]*/){print ts, substr($0, RSTART, RLENGTH); fflush()}
      match($0, /HTTP\/1\.1 [0-9][0-9][0-9]/){print ts, "ANSWER", substr($0, RSTART + 9, 3); fflush()}' \
    > "$HB/slow-upload-requests.txt" &
  HUB_MODELS="$HUB_MODELS $model"
  t0=$(date +%s)
  "$CLI" --json model upload "$w" "$model" --lane f16 --on "$M" -y > "$HB/slow-upload.out" 2>&1; code=$?
  t1=$(date +%s)
  sudo tc qdisc del dev lo root
  sleep 2; sudo pkill -f "tcpdump -i lo -l -nn -tttt" 2> /dev/null
  rm -rf "$w"
  python3 -I - "$HB/slow-upload-requests.txt" "$code" "$((t1 - t0))" "$HB/slow-upload.out" > "$HB/slow-upload-verdict.txt" <<'PY'
import collections, datetime, re, sys, urllib.parse
path, code, took, out = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
puts, answers, per_object = [], collections.Counter(), collections.Counter()
for line in open(path, errors="replace"):
    date, clock, rest = line.rstrip("\n").split(" ", 2)
    if rest.startswith("ANSWER "):
        answers[rest[7:]] += 1
        continue
    method, target = rest.split(" ", 1)
    parts = urllib.parse.urlsplit(target)
    q = dict(urllib.parse.parse_qsl(parts.query))
    if method != "PUT" or "X-Amz-Date" not in q:
        continue
    used = datetime.datetime.fromisoformat(f"{date} {clock}").replace(tzinfo=datetime.timezone.utc)
    minted = datetime.datetime.strptime(q["X-Amz-Date"], "%Y%m%dT%H%M%SZ").replace(tzinfo=datetime.timezone.utc)
    puts.append(((used - minted).total_seconds(), int(q["X-Amz-Expires"])))
    per_object[parts.path] += 1
text = open(out, errors="replace").read()
reasons = re.findall(r'"reason"\s*:\s*\{[^}]*"message"\s*:\s*"([^"]{0,240})', text)
statuses = re.findall(r'"status"\s*:\s*"(completed|succeeded|failed|canceled)"', text)
if not puts:
    print(f"FAIL run exit {code} in {took} s; no presigned PUT captured"); sys.exit()
most = max(per_object.values())
summary = (f"run exit {code} ({statuses[-1] if statuses else 'no status'}) in {took} s; {len(per_object)} objects, "
           f"{len(puts)} presigned PUTs (at most {most} for one object), X-Amz-Expires {sorted({e for _, e in puts})} s, "
           f"the oldest grant spent {max(puts)[0]:.0f} s after minting; the store answered {dict(sorted(answers.items()))}"
           + (f"; reason: {reasons[-1]}" if reasons else ""))
passed = code == 0 and statuses and statuses[-1] in ("completed", "succeeded") and most <= 2
print(("PASS " if passed else "FAIL ") + summary)
PY
  log "slow upload verdict: $(cat "$HB/slow-upload-verdict.txt")"
  local verdict detail
  read -r verdict detail < "$HB/slow-upload-verdict.txt"
  expect "hub slow-upload" "$verdict" "$detail"
}
