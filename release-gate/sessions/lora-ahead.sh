# lora-ahead (cl-1318; Paul 11-19: "if you have to do a LoRA merge, do it as soon as possible, prior to receiving a request"): one
# rental, H3's 15 s Turbo clip from a packages checkout, a dev home whose first-party packages are that checkout. Arms:
# - base: the open RC's Runtime, TensorFS and TensorD. A cold call merges the Turbo LoRA in its first step (`moved.bake`).
# - cold: the candidate's. The same call and seed: its merge inline as before, its pages written to the OS's temporary storage
#   (`tensorfs::derived`), the same video bytes out.
# - second: the machine restarted (no executor): the call reads the merged version from disk (`moved.derived`), merging nothing.
# - ahead: the derived state cleared, the machine restarted, a Shot box this runner's Calcifer predicts onto the pod. The pod
#   stages it and builds it at level ram, where the Runtime merges ahead (its executor's `[merge] ahead` line); the box's render
#   merges nothing. Its merged pages are the cold call's, byte for byte (each page's sha256).
# - kill: a call whose executor is killed (kill -9) while its merge writes, then one more call: the cache never past its cap, no
#   dead writer's part left once the next process looks, and one merged version on disk however many calls.
# Each arm's record (run-<arm>.json), the cache's bytes after it (disk.tsv), and the executors' merge lines (merge-<arm>.txt).
# params: tree (a packages_refs name), base and cand ({"runtime", "tensorfs"} wheel sources, lib.sh `wheel_from`; base's
#   "tensord" a cozy-creator run id whose cozy-* artifact is the RC's TensorD), wait_s (how long the box may take to be built,
#   default 1800), need_s
. "$(dirname "$0")/lib.sh"
TREE=$PACKAGES/$(param tree pk) WAIT=$(param wait_s 1800)
[ -f "$TREE/minimax-h3/pyproject.toml" ] || fail "no minimax-h3 in $TREE"
SOURCE=$(cd "$TREE" && pwd)
wheel() { wheel_from "$OUT/wheels/$1-$2" "$(param "$1" '{}' | jq -r --arg k "$2" '.[$k] // empty')"; }
for arm in base cand; do
  for kind in runtime tensorfs; do
    f=$(wheel "$arm" "$kind") && [ -n "$f" ] || fail "$arm: no $kind wheel"
    declare "${arm^^}_${kind^^}=$f"
  done
done
mkdir -p "$OUT/base-cozy"
gh run download "$(param base '{}' | jq -r .tensord)" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$OUT/base-cozy" > /dev/null 2>&1 ||
  fail "no base cozy"
BASE_TENSORD=$(find "$OUT/base-cozy" -name cozy-linux-x86_64 | head -1); chmod +x "$BASE_TENSORD"
say "base: ${BASE_RUNTIME##*/} ${BASE_TENSORFS##*/} TensorD $("$BASE_TENSORD" --version); cand: ${CAND_RUNTIME##*/} ${CAND_TENSORFS##*/} TensorD $(cozy --version)"
# A dev home, its first-party packages from the checkout (load-ahead.sh), so the box's tool is the checkout's.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
kernels "$SOURCE/minimax-h3"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param need_s 4800)"
rent

pod() { timeout 600 cozy machine ssh "$NAME" -- "$1" < /dev/null 2> /dev/null | grep -v '^Warning: Permanently added'; }
# arm <name> <base|cand>: the pod moved to that arm's builds, which restarts it (no executor kept).
arm() {
  local tensord=$BASE_TENSORD runtime=$BASE_RUNTIME tensorfs=$BASE_TENSORFS
  [ "$2" = base ] || { tensord=$(command -v cozy); runtime=$CAND_RUNTIME; tensorfs=$CAND_TENSORFS; }
  step "update-$1" machine update "$NAME" --tensord "$tensord" --runtime "$runtime" --tensorfs "$tensorfs" -y || fail "$1: the update"
  say "$1: pod $(cozy machine show "$NAME" --json | jq -r '"cozy \(.status.version) Runtime \(.status.runtime) TensorFS \(.status.tensorfs)"')"
}
restart() { step "restart-$1" machine update "$NAME" --mode dedicated -y || fail "$1: the restart"; }
clear_derived() { derived_state "$1" reset; }
# disk <arm>: the cache's bytes, its parts, its cap (an eighth of /var/tmp's filesystem) and the store's reserve.
printf 'arm\tbytes\tpages\tparts\tcap\n' > "$OUT/disk.tsv"
disk() {
  pod 'b=0; p=0; q=0; for d in /var/tmp/cozy-derived-*/*/; do [ -d "$d" ] || continue; b=$((b + $(du -sb "$d" | cut -f1)));
    p=$((p + $(find "$d" -type f ! -name ".*" | wc -l))); q=$((q + $(find "$d" -type f -name ".*.part" | wc -l))); done
    echo "$b $p $q $(( $(df -B1 --output=size /var/tmp | tail -1) / 8 ))"' > "$OUT/disk-$1.txt"
  read -r b p q cap < "$OUT/disk-$1.txt"
  printf '%s\t%s\t%s\t%s\t%s\n' "$1" "$b" "$p" "$q" "$cap" >> "$OUT/disk.tsv"
  say "$1: derived cache $((${b:-0} >> 20)) MiB in $p pages, $q parts, cap $((${cap:-0} >> 20)) MiB"
  [ -n "$cap" ] && [ "${b:-0}" -le "$cap" ] || fail "$1: the derived cache holds ${b:-?} bytes, past its cap ${cap:-?}"
}
# sums <arm>: each merged page's sha256.
sums() { pod 'cd /var/tmp && find cozy-derived-*/*/ -type f ! -name ".*" -exec sha256sum {} + | sed "s| cozy-derived-[^/]*/[^/]*/| |" | sort -k2' > "$OUT/sums-$1.txt"; }
# merges <arm>: the merge lines of the executors started since the last derived-state reset, in merge-<arm>.txt.
merges() { pod 'grep -h "\[merge\] ahead" $(find /var/lib/cozy -name stderr.log -newer /var/tmp/derived-state.sh 2> /dev/null) 2> /dev/null' > "$OUT/merge-$1.txt"; }
# shot <arm> <seed>: the 15 s Turbo clip, its record in run-<arm>.json.
SHOT='[{"description":"[Shot 1]\nA paper boat drifts down a rain-filled gutter past glowing shop windows at night. The camera follows at street level as drops ripple the water.","duration":15}]'
shot() {
  step "$1" run "$SOURCE/minimax-h3/generate" "beats:=$SHOT" "soundscape=Rain on pavement, distant traffic." turbo:=true "seed:=$2" --on "$NAME" \
    --output "$OUT/$1" || fail "$1: the call"
  record "$1" "$(runno "$1")"
}
record() {
  (cd "$RUN_PROJECT" && cozy --json run watch "$2" --once --full) > "$OUT/run-$1.json" 2>&1 || fail "$1: run $2 unread"
  jq -c --arg arm "$1" '{arm: $arm, run: .number, status, execution_ms, memoized: (.memo.memoized // false),
    bake: ([.calls[]?.measurements.memory.stats.varena[]?.stats.moved.bake] | map(select(. != null))),
    derived: ([.calls[]?.measurements.memory.stats.varena[]?.stats.moved.derived] | map(select(. != null))),
    stages: [.report.measured[0].stages[]? | {(.name): (.ms | floor)}] | add}' "$OUT/run-$1.json" | tee -a "$R" > "$OUT/summary-$1.json"
  find "$OUT/$1" -type f -name '*.mp4' -exec sha256sum {} + 2> /dev/null | sed "s/^/$1 /" >> "$OUT/videos.txt"
}

# The weights onto the pod by a base call, which downloads them; every later arm reads them from its disk.
arm fetch base
shot fetch 1318
arm base base; clear_derived base; restart base
shot base 1319
arm cold cand; clear_derived cold; restart cold
shot cold 1319
disk cold; sums cold
restart second
shot second 1320
disk second
# The prediction: the project shown as a page shows it, and a Shot box Calcifer guesses onto the pod.
clear_derived ahead; restart ahead
python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/shown.txt" 2>&1 & BG="$BG $!"
for _ in $(seq 1 30); do grep -q '"held"' "$OUT/shown.txt" && break; sleep 1; done
ASK=$(date +%s)
run_page add shot --tool minimax-h3/generate --fields "$(jq -nc --arg b "$SHOT" '{beats: ($b | fromjson), soundscape: "Rain on pavement, distant traffic.", turbo: true, seed: 1321}')" \
  > "$OUT/box.json" 2>&1 || fail "the box: $(tail -c 300 "$OUT/box.json")"
readiness() { cozy machine show "$NAME" --json 2> /dev/null | jq -r '[.status.ahead[]? | .readiness.stage // empty] | first // ""'; }
# The guess is this pod's (placement may pick another of the account's machines: then this arm proves nothing, and it ends).
t=0; until [ -n "$(readiness)" ]; do
  sleep 5; t=$((t + 5)); [ "$t" -lt 300 ] || { run_page board > "$OUT/board.json" 2>&1; fail "the box's guess isn't on $NAME: $(head -c 300 "$OUT/board.json")"; }
done
t=0; until grep -q '\[merge\] ahead' "$OUT/merge-ahead.txt" 2> /dev/null; do
  sleep 20; t=$((t + 20)); merges ahead; echo "$(date -u +%T) $(readiness)" >> "$OUT/readiness.txt"
  [ "$t" -lt "$WAIT" ] || { cozy machine logs "$NAME" --lines 0 > "$OUT/tensord.log" 2>&1; fail "no merge ahead within $WAIT s: $(tail -3 "$OUT/readiness.txt")"; }
done
say "ahead: merged $(( $(date +%s) - ASK )) s after the box, readiness $(readiness): $(cut -c1-500 "$OUT/merge-ahead.txt")"
disk ahead; sums ahead
python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" shot > "$OUT/render.txt" 2>&1 || fail "the render: $(tail -c 400 "$OUT/render.txt")"
(cd "$RUN_PROJECT" && cozy --json run ls --all --limit 10) > "$OUT/runs.json" 2>&1
record ahead "$(jq -r '[.runs[]? | select(.status == "completed")] | max_by(.created_at_ms) | .number // empty' "$OUT/runs.json")"
diff <(awk '{print $2, $1}' "$OUT/sums-cold.txt") <(awk '{print $2, $1}' "$OUT/sums-ahead.txt") > "$OUT/sums.diff" &&
  say "ahead: its $(wc -l < "$OUT/sums-ahead.txt") merged pages are the cold call's, byte for byte" || say "ahead: merged pages differ from the cold call's ($(wc -l < "$OUT/sums.diff") lines)"
kill $BG 2> /dev/null; BG=''
# kill -9 while a merge writes: a part on disk, its executor killed, then the call goes on as TensorD gives it.
clear_derived kill; restart kill
(cd "$RUN_PROJECT" && cozy run "$SOURCE/minimax-h3/generate" "beats:=$SHOT" turbo:=true seed:=1322 --on "$NAME") > "$OUT/kill.txt" 2>&1 & KP=$!
t=0; until [ "$(pod 'find /var/tmp/cozy-derived-*/ -name ".*.part" | wc -l')" -gt 0 ] 2> /dev/null; do
  sleep 2; t=$((t + 2)); [ "$t" -lt 900 ] || fail "kill: no merge writing within 900 s"
done
pod 'kill -9 $(pgrep -n -f cozy_runtime.internal.executor) && echo killed' > "$OUT/killed.txt"; say "kill: $(cat "$OUT/killed.txt") the executor mid-write"
disk killed
wait "$KP"; say "kill: its call ended: $(tail -2 "$OUT/kill.txt" | tr '\n' ' ' | cut -c1-300)"
disk after-kill
shot repeat 1323
disk repeat
read -r _ cold _ < <(grep -P '^cold\t' "$OUT/disk.tsv" | tr '\t' ' ')
read -r _ again _ parts _ < <(grep -P '^repeat\t' "$OUT/disk.tsv" | tr '\t' ' ')
[ "$parts" = 0 ] || fail "repeat: $parts parts left after a later process looked"
[ "$again" -le "$cold" ] || fail "repeat: the cache grew to $again bytes from the cold call's $cold"
say "disk: one merged version, $((again >> 20)) MiB after kill -9 and two more calls ($((cold >> 20)) MiB after the first), no part left"
merges repeat
cozy machine logs "$NAME" --lines 0 > "$OUT/tensord.log" 2>&1 || true
sort "$OUT/videos.txt" | tee -a "$R"
b=$(awk '$1 == "base" {print $2}' "$OUT/videos.txt" | head -1) c=$(awk '$1 == "cold" {print $2}' "$OUT/videos.txt" | head -1)
[ -n "$b" ] && [ "$b" = "$c" ] && say "base and cold: the same video bytes ($b)" || say "base and cold videos differ: $b vs $c"
say "done"
