# lora-ahead (cl-1318; Paul 11-19: "if you have to do a LoRA merge, do it as soon as possible, prior to receiving a request"): one
# rental, one LoRA'd tool from a packages checkout (default H3's 15 s Turbo clip), a dev home whose first-party packages are that
# checkout. Each arm (`arms`, in order) starts with the machine restarted, so no executor of an earlier one serves it:
# - base: the open RC's Runtime, TensorFS and TensorD. A cold call merges the LoRA inline (`moved.bake`).
# - cold: the candidate's, the derived state cleared. The same call and seed: its merge inline as before, its pages written to
#   the OS's temporary storage (`tensorfs::derived`), the same output bytes.
# - second: a later call reads the merged version from disk (read ahead, its fills `baked`), merging nothing.
# - ahead: the derived state cleared, a box of the tool this runner's Calcifer predicts onto the pod. The pod builds it (`map`),
#   where the Runtime merges ahead (its executor's `[merge] ahead` line), and the box's render merges nothing. Its merged pages
#   are the cold call's, byte for byte (each page's sha256).
# - kill: the derived state cleared, a call whose executor is killed (kill -9) while its merge writes, then one more call: the
#   cache never past its cap, no dead writer's part left once a later process looks, one merged version however many calls.
# Each arm's record (run-<arm>.json), the cache's bytes after it (disk.tsv), and the executors' merge lines (merge-<arm>.txt).
# params: tree (a packages_refs name), tool (`<package>/<function>`), args (its call's arguments, a JSON list; each arm adds its
#   seed), fields (its box's, a JSON object; the arm sets its seed), arms (default "base cold second ahead kill"), base and cand
#   ({"runtime", "tensorfs"} wheel sources, lib.sh `wheel_from`; base's "tensord" a cozy-creator run id whose cozy-* artifact is
#   the RC's TensorD; base only for a base arm), wait_s (how long the box may take to merge, default 1800), need_s
. "$(dirname "$0")/lib.sh"
TREE=$PACKAGES/$(param tree pk) WAIT=$(param wait_s 1800) TOOL=$(param tool minimax-h3/generate) ARMS=$(param arms 'base cold second ahead kill')
SHOT='[{"description":"[Shot 1]\nA paper boat drifts down a rain-filled gutter past glowing shop windows at night. The camera follows at street level as drops ripple the water.","duration":15}]'
mapfile -t ARGS < <(param args "$(jq -nc --arg b "$SHOT" '["beats:=\($b)", "soundscape=Rain on pavement, distant traffic.", "turbo:=true"]')" | jq -r '.[]')
FIELDS=$(param fields "$(jq -nc --arg b "$SHOT" '{beats: ($b | fromjson), soundscape: "Rain on pavement, distant traffic.", turbo: true}')")
[ -f "$TREE/${TOOL%%/*}/pyproject.toml" ] || fail "no ${TOOL%%/*} in $TREE"
SOURCE=$(cd "$TREE" && pwd)
wheel() { wheel_from "$OUT/wheels/$1-$2" "$(param "$1" '{}' | jq -r --arg k "$2" '.[$k] // empty')"; }
kinds=cand; [[ " $ARMS " != *" base "* ]] || kinds="base cand"
for arm in $kinds; do
  for kind in runtime tensorfs; do
    f=$(wheel "$arm" "$kind") && [ -n "$f" ] || fail "$arm: no $kind wheel"
    declare "${arm^^}_${kind^^}=$f"
  done
done
BASE_TENSORD='' BASE_RUNTIME='' BASE_TENSORFS=''
if [[ " $ARMS " == *" base "* ]]; then
  mkdir -p "$OUT/base-cozy"
  gh run download "$(param base '{}' | jq -r .tensord)" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$OUT/base-cozy" > /dev/null 2>&1 ||
    fail "no base cozy"
  BASE_TENSORD=$(find "$OUT/base-cozy" -name cozy-linux-x86_64 | head -1); chmod +x "$BASE_TENSORD"
  say "base: ${BASE_RUNTIME##*/} ${BASE_TENSORFS##*/} TensorD $("$BASE_TENSORD" --version)"
fi
say "cand: ${CAND_RUNTIME##*/} ${CAND_TENSORFS##*/} TensorD $(cozy --version); $TOOL ${ARGS[*]}; arms $ARMS"
# A dev home, its first-party packages from the checkout (load-ahead.sh), so the box's tool is the checkout's.
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
kernels "$SOURCE/${TOOL%%/*}"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param need_s 4800)"
rent

pod() { timeout 600 cozy machine ssh "$NAME" -- "$1" < /dev/null 2> /dev/null | grep -v '^Warning: Permanently added'; }
# builds <arm> <base|cand>: the pod moved to that side's builds (a restart), when it isn't on them already.
SIDE=''
builds() {
  [ "$SIDE" != "$2" ] || return 0
  local tensord=$BASE_TENSORD runtime=$BASE_RUNTIME tensorfs=$BASE_TENSORFS
  [ "$2" = base ] || { tensord=$(command -v cozy); runtime=$CAND_RUNTIME; tensorfs=$CAND_TENSORFS; }
  step "update-$1" machine update "$NAME" --tensord "$tensord" --runtime "$runtime" --tensorfs "$tensorfs" -y || fail "$1: the update"
  say "$1: pod $(cozy machine show "$NAME" --json | jq -r '"cozy \(.status.version) Runtime \(.status.runtime) TensorFS \(.status.tensorfs)"')"
  SIDE=$2
}
restart() { step "restart-$1" machine update "$NAME" --mode dedicated -y || fail "$1: the restart"; }
# disk <arm>: the cache's bytes, pages, parts, and its cap (an eighth of /var/tmp's filesystem).
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
# call <arm> <seed>: the tool's call, its record in run-<arm>.json.
call() {
  step "$1" run "$SOURCE/$TOOL" "${ARGS[@]}" "seed:=$2" --on "$NAME" --output "$OUT/$1" || fail "$1: the call"
  record "$1" "$(runno "$1")"
}
record() {
  (cd "$RUN_PROJECT" && cozy --json run watch "$2" --once --full) > "$OUT/run-$1.json" 2>&1 || fail "$1: run $2 unread"
  jq -c --arg arm "$1" '{arm: $arm, run: .number, status, execution_ms, memoized: (.memo.memoized // false),
    bake: ([.calls[]?.measurements.memory.stats.varena[]?.stats.moved.bake] | map(select(. != null))),
    stages: [.report.measured[0].stages[]? | {(.name): (.ms | floor)}] | add}' "$OUT/run-$1.json" | tee -a "$R" > "$OUT/summary-$1.json"
  find "$OUT/$1" -type f \( -name '*.mp4' -o -name '*.png' -o -name '*.webp' -o -name '*.jpg' \) -exec sha256sum {} + 2> /dev/null | sed "s/^/$1 /" >> "$OUT/outputs.txt"
}
readiness() { cozy machine show "$NAME" --json 2> /dev/null | jq -c '[.status.ahead[]? | .readiness | {stage, reason}] | first // empty'; }

seed=1318
builds fetch "$([[ " $ARMS " == *" base "* ]] && echo base || echo cand)"
call fetch "$seed"  # its weights onto the pod: every later arm reads them from its disk
for arm in $ARMS; do
  seed=$((seed + 1))
  case $arm in
    base) builds base base; derived_state base reset; restart base; call base 1400 ;;
    cold) builds cold cand; derived_state cold reset; restart cold; call cold 1400; disk cold; sums cold ;;
    second) builds second cand; restart second; call second "$seed"; disk second ;;
    ahead)
      builds ahead cand; derived_state ahead reset; restart ahead
      # The project shown as a page shows it, and the tool's box, which Calcifer guesses onto the pod.
      python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/shown.txt" 2>&1 & BG="$BG $!"
      for _ in $(seq 1 30); do grep -q '"held"' "$OUT/shown.txt" && break; sleep 1; done
      ASK=$(date +%s)
      run_page add box --tool "$TOOL" --fields "$(jq -c --argjson s "$seed" '. + {seed: $s}' <<<"$FIELDS")" > "$OUT/box.json" 2>&1 ||
        fail "the box: $(tail -c 300 "$OUT/box.json")"
      # The guess is this pod's (placement may pick another of the account's machines: then this arm proves nothing, and it ends).
      t=0; until [ -n "$(readiness)" ]; do
        sleep 5; t=$((t + 5)); [ "$t" -lt 300 ] || { run_page board > "$OUT/board.json" 2>&1; fail "the box's guess isn't on $NAME: $(head -c 300 "$OUT/board.json")"; }
      done
      t=0; until grep -q '\[merge\] ahead' "$OUT/merge-ahead.txt" 2> /dev/null; do
        sleep 20; t=$((t + 20)); merges ahead; echo "$(date -u +%T) $(readiness)" >> "$OUT/readiness.txt"
        [ "$t" -lt "$WAIT" ] || { cozy machine logs "$NAME" tensord --lines 0 > "$OUT/tensord.log" 2>&1; fail "no merge ahead within $WAIT s: $(tail -2 "$OUT/readiness.txt")"; }
      done
      say "ahead: merged $(( $(date +%s) - ASK )) s after the box, readiness $(readiness): $(cut -c1-600 "$OUT/merge-ahead.txt")"
      disk ahead; sums ahead
      python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" box > "$OUT/render.txt" 2>&1 || fail "the render: $(tail -c 400 "$OUT/render.txt")"
      (cd "$RUN_PROJECT" && cozy --json run ls --all --limit 10) > "$OUT/runs.json" 2>&1
      record ahead "$(jq -r '[.runs[]? | select(.status == "completed")] | max_by(.created_at_ms) | .number // empty' "$OUT/runs.json")"
      if [ -s "$OUT/sums-cold.txt" ]; then
        diff <(awk '{print $2, $1}' "$OUT/sums-cold.txt") <(awk '{print $2, $1}' "$OUT/sums-ahead.txt") > "$OUT/sums.diff" &&
          say "ahead: its $(wc -l < "$OUT/sums-ahead.txt") merged pages are the cold call's, byte for byte" ||
          say "ahead: merged pages differ from the cold call's ($(wc -l < "$OUT/sums.diff") lines)"
      fi
      kill $BG 2> /dev/null; BG='' ;;
    kill)
      builds kill cand; derived_state kill reset; restart kill
      (cd "$RUN_PROJECT" && cozy run "$SOURCE/$TOOL" "${ARGS[@]}" "seed:=$seed" --on "$NAME") > "$OUT/kill.txt" 2>&1 & KP=$!
      t=0; until [ "$(pod 'find /var/tmp/cozy-derived-*/ -name ".*.part" | wc -l')" -gt 0 ] 2> /dev/null; do
        sleep 1; t=$((t + 1)); [ "$t" -lt 900 ] || fail "kill: no merge writing within 900 s"
      done
      # The newest executor (no pgrep in the image): its pid, then kill -9.
      pod 'p=$(for d in /proc/[0-9]*; do tr "\0" " " < $d/cmdline 2> /dev/null | grep -q "cozy_runtime.internal.executor" && echo ${d#/proc/}; done | sort -n | tail -1); [ -n "$p" ] && kill -9 $p && echo "killed $p"' > "$OUT/killed.txt"
      grep -q killed "$OUT/killed.txt" || fail "kill: no executor to kill: $(cat "$OUT/killed.txt")"
      say "kill: $(cat "$OUT/killed.txt"), the executor mid-write"
      disk killed
      wait "$KP"; say "kill: its call ended: $(tail -2 "$OUT/kill.txt" | tr '\n' ' ' | cut -c1-300)"
      disk after-kill
      seed=$((seed + 1)); call repeat "$seed"; disk repeat
      read -r _ once _ < <(grep -P '^(cold|ahead)\t' "$OUT/disk.tsv" | tail -1 | tr '\t' ' ')
      read -r _ again _ parts _ < <(grep -P '^repeat\t' "$OUT/disk.tsv" | tr '\t' ' ')
      [ "$parts" = 0 ] || fail "repeat: $parts parts left after a later process looked"
      [ -z "$once" ] || [ "$again" -le "$once" ] || fail "repeat: the cache grew to $again bytes from one merge's $once"
      say "disk: $((again >> 20)) MiB after kill -9 and one more call (one merge: $((${once:-0} >> 20)) MiB), no part left" ;;
    *) fail "no arm $arm" ;;
  esac
  merges "$arm"
done
sort "$OUT/outputs.txt" | tee -a "$R"
b=$(awk '$1 == "base" {print $2}' "$OUT/outputs.txt" | head -1) c=$(awk '$1 == "cold" {print $2}' "$OUT/outputs.txt" | head -1)
[ -z "$b" ] || { [ "$b" = "$c" ] && say "base and cold: the same output bytes ($b)" || say "base and cold outputs differ: $b vs $c"; }
say "done"
