# stage-ahead (cl-1318; Paul, 11-19: "warming page cache; get the model into RAM / page-cache so it can be read more quickly"; "begin
# loading it into VRAM as soon as possible, when it's known to be needed"): one rental and boxes a page shows, each predicted onto it
# by this runner's Calcifer. Each arm has a project of its own; TensorD is restarted first and the store's page cache dropped
# (`evict`: the weights stay on disk, as on a machine whose RAM went to other work).
#   free    this build: a Qwen box `q` alone, its GPU free: loaded ahead as before (`loaded`), its render's first step loads nothing.
#   before  the RC's TensorD (`before`): an SDXL box `p` loaded ahead, another program holding `hold_gb` of the GPU (a free 4090
#           fits q beside p alone, by 0.23 GB: 44636453479), then `q`, which the GPU can't take beside them (`ready`); q rendered.
#   after   this build: the same; q's executor and models are built at level ram beside p, and its weights read into RAM (`built`,
#           all its bytes in the page cache) before its render.
#   disk    this build: as `after`, then the page cache dropped just before Render: the same built executor reading the disk, so
#           what the stage itself buys is after against disk.
#   yield   this build: an SDXL call sent cold while q is being staged (q can't fit beside another program holding `yield_gb`
#           of the GPU), against the same cold call with nothing staging (yield-control): a stage reads no disk from the call's
#           dispatch to its first step, so the call starts no later (TensorD's log: the stage stopped, and went on after).
#   back    this build: as `after` up to q `built` with its weights in RAM; a program then takes the host's free memory a while,
#           and what q held in RAM is given back, nothing ended (its executor still holds it built).
#   h3-before / h3-after  the RC's TensorD / this build: an H3 Shot box (`minimax-h3/generate`, a composite: its models are
#           its segment's, which its rules pick) from the packages checkout `tree`; the RC leaves it `ready`, this build builds its
#           segment at level ram (`built`) and stages it. Rendered, each one's time until its denoise loop could begin is
#           fixtures/cold-load/table.py's `ready` (cold-load's measure), with its executor start, construction and LoRA bake.
# A render's time until inference starts is its stages before its first denoising step (and that stage's loads), against its bound:
# its bytes over PCIe from RAM, or the larger of over the disk and over PCIe from disk: PCIe at the pod's own ceiling
# (`tensorfs.ceiling`), the disk as read past the page cache here (`dd iflag=direct`). Each arm's boxes have seeds of their own: the
# call memo outlives a restart, and answered a repeat of an earlier arm's render without running it (44633523612). params:
#   before   a dev-build run id whose cozy is the RC's (required)
#   tool, other  the boxes' tools (default qwen-image-2/image, sdxl/generate)
#   wait_s   how long a box may take to reach its stage (default 600)
#   arms     which arms run, in this order (default "free before after disk yield back")
#   yield_gb what the other program holds of the GPU in yield (default 6)
#   hold_gb  what the other program holds of the GPU in before, after and back (default 1.5)
#   tree     the packages_refs checkout H3 runs from (`-f packages_refs=pk=<ref>`, tree pk), for the h3 arms: a dev home
#   shot     the H3 Shot's beats (default one 5 s beat: "shorter is quadratically faster"; the load is the same)
. "$(dirname "$0")/lib.sh"
ARMS=" $(param arms 'free before after disk yield back') " YIELD_GB=$(param yield_gb 6)
HOLD_GB=$(param hold_gb 1.5) TOOL=$(param tool qwen-image-2/image) OTHER=$(param other sdxl/generate) WAIT=$(param wait_s 600) BEFORE=$(param before '') BEGAN=$(date +%s)
[ -n "$BEFORE" ] || fail "before: a dev-build run id of the RC's cozy"
fail_if() { say "FAIL: $*"; RC=1; }
arm() { [[ $ARMS == *" $1 "* ]]; }
QWEN=false; for a in free before after disk yield back; do arm "$a" && QWEN=true; done
H3=false; { arm h3-before || arm h3-after; } && H3=true
SHOT=$(param shot '[{"description":"[Shot 1]\nA paper boat drifts down a rain-filled gutter past glowing shop windows at night. The camera follows at street level as drops ripple the water.","duration":5}]')
if [ "$H3" = true ]; then
  TREE=$(param tree '')
  [ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/minimax-h3/pyproject.toml" ] || fail "the h3 arms run H3 from a packages checkout: -f packages_refs=pk=<ref>, params.tree pk"
  SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
  # A dev home, its first-party packages from the checkout; Calcifer started again on it (load-ahead.sh).
  cfg=$HOME/.cozy/config.toml
  mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
  { echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
  awk -v source="$SOURCE" '
    function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
    /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
    "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
  cozy restart > "$OUT/restart-calcifer.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart-calcifer.txt")"
  kernels "$SOURCE/minimax-h3"
  say "a dev home, minimax-h3 $(grep -m1 '^version' "$SOURCE/minimax-h3/pyproject.toml") from the checkout $TREE"
fi
PROMPT="a red lighthouse on a cliff at dusk, waves below, photo"
Q_FIELDS() { jq -nc --arg p "$PROMPT" --argjson s "$1" '{instructions: $p, turbo: false, prompt_upsampling: false, megapixels: 1, aspect_ratio: "1:1", seed: $s}'; }
P_FIELDS() { jq -nc --argjson s "$1" '{prompt: "a brass compass on a walnut desk", seed: $s}'; }
mkdir -p "$OUT/before" && gh run download "$BEFORE" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$OUT/before" > /dev/null 2>&1 || fail "no cozy from run $BEFORE"
RC_BIN=$(find "$OUT/before" -name cozy-linux-x86_64 | head -1) && chmod +x "$RC_BIN" || fail "run $BEFORE has no cozy-linux-x86_64"
RC_V=$("$RC_BIN" --version | awk '{print $2}') WANT=$(cozy --version | awk '{print $2}')
[ "$RC_V" != "$WANT" ] || fail "before ($RC_V) is this build"
say "this build $WANT, before $RC_V"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param need_s 3600)"
rent
pod() { timeout "${T:-300}" cozy machine ssh "$NAME" -- "$1" < /dev/null 2>> "$OUT/ssh.err"; }
# py [-p python] <script> [args]: a Python script on the pod, sent whole, each in a file of its own.
py() {
  local bin=python3 s f; [ "$1" != -p ] || { bin=$2; shift 2; }
  s=$(base64 -w0 <<<"$1"); f=/tmp/stage-ahead-$(md5sum <<<"$1" | cut -c1-8).py; shift
  pod "echo $s | base64 -d > $f && $bin $f $*"
}
# hold / unhold: another program holds HOLD_GB of the GPU (torch, in a machine's environment) until unheld.
HOLD='import os, sys, time, torch
x = torch.empty(int(float(sys.argv[1]) * 1e9), dtype=torch.uint8, device="cuda")
x.fill_(1)
torch.cuda.synchronize()
open("/tmp/stage-ahead-hold", "w").close()
while os.path.exists("/tmp/stage-ahead-hold"):
    time.sleep(0.2)'
HOLDER=''
hold() {
  local env t=0
  env=$(T=60 pod "ls -t /var/lib/cozy/tensord/generations/*/env/bin/python | head -1" | tail -1)
  T=7200 py -p "$env" "$HOLD" "$HOLD_GB" > "$OUT/hold-$1.txt" 2>&1 & HOLDER=$!; BG="$BG $HOLDER"
  until T=30 pod "test -f /tmp/stage-ahead-hold" > /dev/null 2>&1; do
    sleep 2; t=$((t + 2)); [ "$t" -lt 120 ] || fail "$1: the other program never held the GPU: $(tail -c 300 "$OUT/hold-$1.txt")"
  done
  say "$1: another program holds $HOLD_GB GB of the GPU: $(pod "nvidia-smi --query-gpu=memory.used,memory.free --format=csv,noheader")"
}
unhold() { [ -z "$HOLDER" ] || { pod "rm -f /tmp/stage-ahead-hold" > /dev/null; wait "$HOLDER" 2> /dev/null; HOLDER=''; }; }
meminfo() { pod "grep -E '^(MemTotal|MemFree|MemAvailable|Cached):' /proc/meminfo | awk '{printf \"%s %.1f GiB \", \$1, \$2 / 1048576}'"; }
# The store's page cache dropped (as derived-state.sh does): its files stay on disk.
EVICT='import os
for top, _, names in os.walk("/var/lib/tensorfs"):
    for name in names:
        try:
            fd = os.open(os.path.join(top, name), os.O_RDONLY)
        except OSError:
            continue
        os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        os.close(fd)'
evict() { pod sync; py "$EVICT" || fail "the page cache wasn't dropped"; say "$1: store page cache dropped: $(meminfo)"; }
# runs <version> <step>: the pod's TensorD is <version>, else updated to <bin> once more.
runs() {
  local v; v=$(cozy machine show "$NAME" --json 2> /dev/null | jq -r '.status.version // ""')
  echo "$(date -u +%T) $2 $v" >> "$OUT/versions.txt"
  [ "$v" = "$1" ] || fail "$2: the pod runs TensorD ${v:-?}, not $1"
}
# tensord <bin> <version> <arm>: the pod's TensorD is <bin>, restarted (every executor ends; the disk stays).
tensord() {
  close
  unhold
  step "tensord-$3" machine update "$NAME" --tensord "$1" -y || fail "$3: the pod refused TensorD $2"
  step "restart-$3" machine update "$NAME" --mode dedicated -y || fail "$3: the restart"
  [ "$(cozy machine show "$NAME" --json | jq -r .status.phase)" = ready ] || fail "$3: $NAME isn't ready after its restart"
  runs "$2" "$3"
}
tensord "$(command -v cozy)" "$WANT" start

# The pod's Status as Calcifer relays it, each second: each predicted request's readiness.
( while :; do
    printf '{"at_ms":%s,"status":%s}\n' "$(date +%s%3N)" \
      "$(cozy machine show "$NAME" --json 2> /dev/null | jq -c '{ahead: (.status.ahead // []), running: .status.running, queued: .status.queued}' 2> /dev/null || echo null)" >> "$OUT/status.ndjson"
    sleep 1
  done ) & BG=$!
# item <id|-not id> <field>: the newest Status row's predicted request `id` (or the one that isn't), its readiness field.
item() {
  local sel; case $1 in -*) sel="select(.id != \"${1#-}\")" ;; *) sel="select(.id == \"$1\")" ;; esac
  tail -1 "$OUT/status.ndjson" | jq -r "[.status.ahead[]? | $sel | .readiness.$2 // empty] | first // \"\""
}
# until_item <id|-not id> <stage regex> <arm>: it reaches a stage <stage> matches, else fail. Its id is said.
until_item() {
  local t=0
  until [[ $(item "$1" stage) =~ ^($2)$ ]]; do
    sleep 1; t=$((t + 1))
    [ "$t" -lt "$WAIT" ] || { collect; fail "$3: no predicted request ${1#-} was $2 within $WAIT s: $(tail -1 "$OUT/status.ndjson" | cut -c1-500)"; }
  done
  ID=$(tail -1 "$OUT/status.ndjson" | jq -r --arg s "^($2)$" "[.status.ahead[]? | $( case $1 in -*) echo "select(.id != \"${1#-}\")" ;; *) echo "select(.id == \"$1\")" ;; esac ) | select(.readiness.stage | test(\$s)) | .id] | first")
  say "$3: $ID is $(item "$ID" stage) ($t s): $(item "$ID" done) of $(item "$ID" total) bytes; $(item "$ID" reason)"
}
# until_held <id> <arm>: what of its weights the page cache holds stops growing (or reaches 95% of them), 180 s at most.
until_held() {
  local t=0 was=-1 now
  while now=$(item "$1" done); [ "${now:-0}" != "$was" ] || [ "$t" -lt 5 ]; do
    [ "$(( ${now:-0} * 100 ))" -lt "$(( $(item "$1" total) * 95 ))" ] || break
    [ "$t" -lt 180 ] || break
    was=${now:-0}; sleep 3; t=$((t + 3))
  done
  say "$2: $1 holds $(item "$1" done) of $(item "$1" total) bytes in RAM ($t s), $(item "$1" stage)"
}
collect() {
  cozy machine logs "$NAME" --lines 0 --since "$(( ($(date +%s) - BEGAN) / 60 + 2 ))m" > "$OUT/tensord.log" 2>&1 || say "the pod's TensorD log wasn't read"
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1 || true
}
SHOW=''
# close: the shown project closes (its guesses end), before TensorD restarts: else it lists them to the new service.
close() { [ -z "$SHOW" ] || { kill "$SHOW" 2> /dev/null; SHOW=''; sleep 3; }; }
# project <arm>: a project of its own, shown as an open page shows it (Calcifer guesses only for one).
project() {
  run_project "$OUT/project-$1" || fail "$1: no project"
  python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/shown-$1.txt" 2>&1 & SHOW=$!; BG="$BG $SHOW"
  for _ in $(seq 1 30); do grep -q '"held": \["' "$OUT/shown-$1.txt" && break; sleep 1; done
  grep -q '"held": \["' "$OUT/shown-$1.txt" || fail "$1: the project wasn't shown: $(tail -c 300 "$OUT/shown-$1.txt")"
}
# render <arm> <box>: the box rendered on the pod, its run's record and report kept.
render() {
  local asked; asked=$(date +%s%3N)
  python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" "$2" > "$OUT/render-$1.txt" 2>&1 || fail "$1: the render: $(tail -c 400 "$OUT/render-$1.txt")"
  (cd "$RUN_PROJECT" && cozy --json run ls --all --limit 10) > "$OUT/runs-$1.json" 2>&1
  local n; n=$(jq -r --argjson a "$asked" '[.runs[]? | select(.created_at_ms >= $a)] | max_by(.created_at_ms) | .number // empty' "$OUT/runs-$1.json")
  [ -n "$n" ] || fail "$1: the render's run: $(head -c 300 "$OUT/runs-$1.json")"
  (cd "$RUN_PROJECT" && cozy --json run watch "$n" --once --full) > "$OUT/run-$1.json" 2>&1 || fail "$1: run $n unread"
  [ "$(jq -r '.memo.memoized // false' "$OUT/run-$1.json")" = false ] || fail "$1: the call memo answered its render: it never ran"
  record "$1"
}
# record <arm>: its run's record (run-<arm>.json) as a report: its stages and its weights' bytes.
record() {
  jq -c --arg arm "$1" '{arm: $arm, run: .number, status, memoized: (.memo.memoized // false), accepted_to_started_ms: (.started_at_ms - .accepted_at_ms),
    bytes: ((.measurements.weights.has // 0) + (.measurements.weights.needs // 0)),
    stages: [.report.measured[0].stages[]? | {name, ms: (.ms | floor), steps: (.steps != null)}]}' "$OUT/run-$1.json" | tee -a "$R" > "$OUT/report-$1.json"
}
# called <arm> <seed>: the other tool called cold on the pod (`cozy run`, no box), its record kept.
called() {
  step "$1" run "$OTHER" --on "$NAME" prompt="a brass compass on a walnut desk" "seed:=$2" || fail "$1: the call"
  local n; n=$(runno "$1")
  (cd "$RUN_PROJECT" && cozy --json run watch "$n" --once --full) > "$OUT/run-$1.json" 2>&1 || fail "$1: run $n unread"
  [ "$(jq -r '.memo.memoized // false' "$OUT/run-$1.json")" = false ] || fail "$1: the call memo answered it: it never ran"
  record "$1"
}

# The models' weights onto the pod by a call of each, then the pod's ceilings (a call has measured them this boot).
if [ "$QWEN" = true ]; then
  step fetch-q run "$TOOL" --on "$NAME" instructions="$PROMPT" turbo:=false prompt_upsampling:=false megapixels:=1 aspect_ratio=1:1 seed:=1 || fail "the first $TOOL"
  step fetch-p run "$OTHER" --on "$NAME" prompt="a brass compass on a walnut desk" seed:=1 || fail "the first $OTHER"
fi
[ "$H3" = false ] || step fetch-h3 run "$SOURCE/minimax-h3/generate" "beats:=$SHOT" turbo:=true seed:=1 --on "$NAME" || fail "the first H3 Shot"
CEILING='import glob, json, os, subprocess
for py in sorted(glob.glob("/var/lib/cozy/tensord/generations/*/env/bin/python"), key=os.path.getmtime, reverse=True) + ["python3"]:
    out = subprocess.run([py, "-c", "import json, tensorfs; c = tensorfs.ceiling(device=0); print(json.dumps({\"disk_gbps\": c.disk_gbps, \"disk_direct\": c.disk_direct, \"h2d_gbps\": c.h2d_gbps}))"], capture_output=True, text=True)
    if out.returncode == 0 and out.stdout.strip():
        print(out.stdout.strip().splitlines()[-1]); break'
T=900 py "$CEILING" > "$OUT/ceiling.json" || true
jq -e '.h2d_gbps > 0 and .disk_gbps > 0' "$OUT/ceiling.json" > /dev/null ||
  { say "no ceilings ($(head -c 200 "$OUT/ceiling.json") $(tail -c 200 "$OUT/ssh.err")): bounds are left out"; echo '{"disk_gbps": 0, "h2d_gbps": 0}' > "$OUT/ceiling.json"; }
# The disk past the page cache: the store's eight largest files read at once, O_DIRECT.
DISK=$(T=600 pod "cd /var/lib/tensorfs && f=\$(find . -type f -size +32M -printf '%s %p\n' | sort -rn | head -8 | awk '{print \$2}'); b=\$(du -cb \$f | tail -1 | cut -f1); s=\$(date +%s.%N); for x in \$f; do dd if=\$x of=/dev/null bs=16M iflag=direct status=none & done; wait; e=\$(date +%s.%N); echo \$b \$s \$e" | tail -1)
read -r DB DS DE <<<"$DISK"
DISK_GBPS=$(python3 -c "import sys; b, s, e = map(float, sys.argv[1:4]); print(round(b / (e - s) / 1e9, 2))" $DB $DS $DE 2> /dev/null || echo 0)
jq --argjson d "${DISK_GBPS:-0}" '. + {disk_direct_read_gbps: $d}' "$OUT/ceiling.json" > "$OUT/ceiling.json.new" && mv "$OUT/ceiling.json.new" "$OUT/ceiling.json"
say "ceilings: $(cat "$OUT/ceiling.json"); $(meminfo)"

Q_BYTES=0 HELD=0 DURING=0
# free: q alone on a free GPU loads ahead as before.
if arm free; then
tensord "$(command -v cozy)" "$WANT" free
evict free
project free
run_page add q --tool "$TOOL" --fields "$(Q_FIELDS 11)" > "$OUT/box-free.json" 2>&1 || fail "free: q: $(tail -c 300 "$OUT/box-free.json")"
until_item -none loaded free
Q_BYTES=$(item "$ID" total)
render free q
fi

# staged <arm> <bin> <version> <seed>: p loaded ahead, then q, which can't go beside it.
staged() {
  tensord "$2" "$3" "$1"
  evict "$1"
  project "$1"
  run_page add p --tool "$OTHER" --fields "$(P_FIELDS "$4")" > "$OUT/box-$1-p.json" 2>&1 || fail "$1: p: $(tail -c 300 "$OUT/box-$1-p.json")"
  until_item -none loaded "$1"
  P_ID=$ID
  hold "$1"
  run_page add q --tool "$TOOL" --fields "$(Q_FIELDS "$(($4 + 1))")" > "$OUT/box-$1-q.json" 2>&1 || fail "$1: q: $(tail -c 300 "$OUT/box-$1-q.json")"
}
if arm before; then
staged before "$RC_BIN" "$RC_V" 20
until_item "-$P_ID" ready before
Q_ID=$ID; sleep 5
say "before: q $(item "$Q_ID" stage) after 5 s more: $(meminfo)"
render before q
fi

if arm after; then
staged after "$(command -v cozy)" "$WANT" 30
until_item "-$P_ID" built after
Q_ID=$ID
until_held "$Q_ID" after
[ "$Q_BYTES" != 0 ] || Q_BYTES=$(item "$Q_ID" total)
say "after: $(meminfo)"
render after q
fi

if arm disk; then
staged disk "$(command -v cozy)" "$WANT" 50
until_item "-$P_ID" built disk
Q_ID=$ID
until_held "$Q_ID" disk
evict disk
say "disk: q $(item "$Q_ID" stage), $(item "$Q_ID" done) bytes in RAM now"
render disk q
fi

if arm yield; then
  # Its control: the call cold with nothing staging.
  tensord "$(command -v cozy)" "$WANT" yield-control
  evict yield-control
  HOLD_GB=$YIELD_GB hold yield-control
  called yield-control 81
  # The same call sent the moment q's stage reads.
  tensord "$(command -v cozy)" "$WANT" yield
  evict yield
  project yield
  HOLD_GB=$YIELD_GB hold yield
  run_page add q --tool "$TOOL" --fields "$(Q_FIELDS 83)" > "$OUT/box-yield.json" 2>&1 || fail "yield: q: $(tail -c 300 "$OUT/box-yield.json")"
  # Its stage begins with its build at level ram (`loading`, into RAM) and goes on beside it: the call goes at the first sign.
  t=0; until [[ $(item -none stage) =~ ^(loading|staging|built)$ ]]; do sleep 0.2; t=$((t + 1)); [ "$t" -lt 600 ] || fail "yield: q's stage never began: $(tail -1 "$OUT/status.ndjson" | cut -c1-400)"; done
  say "yield: q is $(item -none stage), $(item -none done) of $(item -none total) bytes in RAM: the call is sent"
  called yield 82
  until_item -none 'built|staged' yield
  say "yield: after the call, q $(item "$ID" stage) with $(item "$ID" done) bytes in RAM"
fi

# back: q built at level ram and its weights in RAM, then a program takes the host's free memory a while.
if arm back; then
staged back "$(command -v cozy)" "$WANT" 40
until_item "-$P_ID" built back
Q_ID=$ID
until_held "$Q_ID" back
HELD=$(item "$Q_ID" done)
HOG='import os, sys, time
def room():
    # The tightest limit on the path of this process, as TensorD reads it: a shell can be inside the number of the machine (44640195326).
    m = {l.split(":")[0]: int(l.split()[1]) * 1024 for l in open("/proc/meminfo")}
    a = m["MemAvailable"]
    path = open("/proc/self/cgroup").read().strip().split("::")[-1]
    while True:
        d = "/sys/fs/cgroup" + path
        try:
            top = open(d + "/memory.max").read().strip()
            if top != "max":
                st = dict(l.split() for l in open(d + "/memory.stat"))
                a = min(a, int(top) - int(open(d + "/memory.current").read()) + int(st["inactive_file"]))
        except (OSError, KeyError):
            pass
        if path in ("", "/"):
            return a
        path = os.path.dirname(path)
held, piece = [], 256 << 20
while room() > (8 << 30):
    b = bytearray(piece)
    b[::4096] = b"\1" * len(b[::4096])
    held.append(b)
print(f"held {len(held) * piece / 2**30:.1f} GiB", flush=True)
time.sleep(float(sys.argv[1]))'
say "back: q holds $HELD bytes in RAM; $(meminfo)"
# It takes memory until 8 GiB are left under the tightest limit on its path (minutes on a 1 TB pod: 333 GiB in 3.5 min, 44638451261), holds it 30 s, then ends.
T=1800 py "$HOG" 30 > "$OUT/hog.txt" 2>&1 & HOG_PID=$!
until grep -q held "$OUT/hog.txt" || ! kill -0 "$HOG_PID" 2> /dev/null; do sleep 5; done
sleep 5
DURING=$(item "$Q_ID" done) STAGE=$(item "$Q_ID" stage)
say "back: while a program holds $(cat "$OUT/hog.txt"): q is $STAGE, $DURING bytes in RAM; $(meminfo)"
wait "$HOG_PID"
sleep 2
say "back: the program ended ($(cat "$OUT/hog.txt")): q $(item "$Q_ID" stage), $(item "$Q_ID" done) bytes in RAM; p $(item "$P_ID" stage); $(meminfo)"
[ "$(cozy machine show "$NAME" --json | jq -r .status.phase)" = ready ] || fail "back: $NAME isn't ready after the program"
[ "$(item "$Q_ID" stage)" = built ] || fail_if "back: q's executor didn't keep it built through the program ($(item "$Q_ID" stage))"
fi
# h3 <arm> <bin> <version> <seed>: an H3 Shot box predicted onto the pod, then rendered; its record a cold-load cell.
h3() {
  tensord "$2" "$3" "$1"
  evict "$1"
  project "$1"
  run_page add shot --tool minimax-h3/generate --fields "$(jq -nc --arg b "$SHOT" --argjson s "$4" '{beats: ($b | fromjson), turbo: true, seed: $s}')" \
    > "$OUT/box-$1.json" 2>&1 || fail "$1: the shot box: $(tail -c 300 "$OUT/box-$1.json")"
}
if arm h3-before; then
  h3 h3-before "$RC_BIN" "$RC_V" 61
  until_item -none ready h3-before
  sleep 20
  say "h3-before: shot $(item "$ID" stage) 20 s later; $(meminfo)"
  render h3-before shot
  printf '%s\t%s\t%s\t%s\t%s\t%s\t\t%s\n' h3-before rc h3 disk denoise run-h3-before.json step2 >> "$OUT/cold-load.tsv"
fi
if arm h3-after; then
  h3 h3-after "$(command -v cozy)" "$WANT" 71
  until_item -none 'built|loaded' h3-after
  until_held "$ID" h3-after
  say "h3-after: $(meminfo)"
  render h3-after shot
  printf '%s\t%s\t%s\t%s\t%s\t%s\t\t%s\n' h3-after this h3 ram denoise run-h3-after.json step2 >> "$OUT/cold-load.tsv"
fi

close; unhold; kill $BG 2> /dev/null; BG=''
collect

grep -aE "stage ahead|load ahead|loads ahead as|queue: goes after|planned in|what it needs is here|items admitted" "$OUT/tensord.log" | cut -c1-360 | tee -a "$R" > "$OUT/ahead.log"
# Each render: its time until inference starts (its stages before the first with denoising steps) against its bound.
python3 -I - "$OUT" "$Q_BYTES" "$HELD" "$DURING" <<'PY' | tee -a "$R" > "$OUT/summary.json"
import json, sys
out, q, held, during = sys.argv[1], int(sys.argv[2]), int(sys.argv[3] or 0), int(sys.argv[4] or 0)
c = json.load(open(f"{out}/ceiling.json"))
disk, h2d = (c.get("disk_direct_read_gbps") or 0) * 1e9, (c.get("h2d_gbps") or 0) * 1e9
bound = {"ram": q / h2d, "disk": max(q / disk, q / h2d)} if disk and h2d else {}
rows = {}
for arm, frm in (("free", "loaded"), ("before", "disk"), ("after", "ram"), ("disk", "disk"), ("yield-control", "disk"), ("yield", "disk")):
    try:
        r = json.load(open(f"{out}/report-{arm}.json"))
    except OSError:
        continue
    b_ = r.get("bytes") or q
    own = {"ram": b_ / h2d, "disk": max(b_ / disk, b_ / h2d)} if disk and h2d else {}
    first = next((i for i, s in enumerate(r["stages"]) if s["steps"]), len(r["stages"]))
    stepped = r["stages"][first]["name"] if first < len(r["stages"]) else None
    before = [s for i, s in enumerate(r["stages"]) if i < first or (stepped and s["name"].startswith(stepped + " / loading"))]
    tui = sum(s["ms"] for s in before) / 1000
    b = own.get(frm)
    rows[arm] = {"run": r["run"], "from": frm, "memoized": r.get("memoized"), "until_inference_s": round(tui, 2), "bound_s": b and round(b, 2),
                 "multiple": b and round(tui / b, 2), "stages": {s["name"]: s["ms"] for s in before}}
print(json.dumps({"q_bytes": q, "ceiling": c, "bounds_s": {k: round(v, 2) for k, v in bound.items()}, "renders": rows,
                  "back": {"held": held, "during": during}}, indent=1))
PY
grep -q "stage ahead: .* in RAM in" "$OUT/ahead.log" || fail_if "no stage ahead in this build's TensorD log"
grep -q "built at level ram" "$OUT/ahead.log" || fail_if "no load ahead at level ram in this build's TensorD log"
! arm free || jq -e '.renders.free.until_inference_s < 1' "$OUT/summary.json" > /dev/null || fail_if "free: the loaded render's first step waited on a load"
! { arm before && arm after; } || jq -e '.renders.after.until_inference_s < .renders.before.until_inference_s' "$OUT/summary.json" > /dev/null ||
  fail_if "after: no sooner than before"
! arm back || [ "${DURING:-0}" -lt "$HELD" ] || fail_if "back: q's RAM wasn't given back ($DURING of $HELD)"
# A stage reads no disk while a call loads: the call sent mid-stage starts no later than its control, within the 5% that is noise.
! arm yield || jq -e '.renders.yield.until_inference_s <= .renders["yield-control"].until_inference_s * 1.05' "$OUT/summary.json" > /dev/null ||
  fail_if "yield: the call sent mid-stage started later than with nothing staging"
! arm yield || grep -q "stage ahead: .*stopped" "$OUT/ahead.log" || fail_if "yield: no stage stopped for the call"
if [ "$H3" = true ]; then
  python3 -I "$(dirname "$0")/fixtures/cold-load/table.py" "$OUT" > "$OUT/h3-table.txt" 2> "$OUT/h3-table.err" || fail_if "h3: table.py: $(tail -c 300 "$OUT/h3-table.err")"
  grep '^{' "$OUT/h3-table.txt" | jq -c '{label, ready_s, bound_s, x, executor_s, construct_s, on_demand_s, bake_host_s, bake_device_s, to_step1_s, need_gb, gpu_gb, calls}' | tee -a "$R" > "$OUT/h3.json"
  grep -q "loads ahead as the calls its rules pick" "$OUT/ahead.log" || fail_if "h3: its list item wasn't prepared as its segment call"
  ! { arm h3-before && arm h3-after; } || [ "$(jq -s 'map({(.label): .ready_s}) | add | .["h3-after"] < .["h3-before"]' "$OUT/h3.json")" = true ] ||
    fail_if "h3: the Shot started no sooner than before"
fi
! { arm after && arm disk; } || jq -e '.renders.after.until_inference_s < .renders.disk.until_inference_s' "$OUT/summary.json" > /dev/null ||
  fail_if "disk: the built executor reading the disk started no later than reading RAM: the stage bought nothing"
[ "$RC" = 0 ] && say "PASS: a predicted box the GPU couldn't take was staged in RAM, its render's load read RAM; a free GPU loaded ahead; the RAM went back"
