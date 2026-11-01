# hub-outage (cl-708): a call's model lookup waits out an unreachable Hub. Escaped on cozy 0.15.4: run 24951, a 3-piece
# H3 shot, failed after 224 s when one DNS failure for tensorhub.com refused a child call's model resolution. One pod
# with SSH (`cpu` is enough: every call is deviceless), the session's cozy as TensorD, the root fixtures/hub-outage
# calling `piece` once per model, `pause_s` apart. TensorD restarts first (no resolver answer held), the fixture is
# installed (its run redeems the capability), then a root shell points the pod's resolver at a nameserver nothing
# answers, and mends it `outage_s` after the run says it retries:
#   piece 1  `model`: its lookup fails on DNS as 24951's did; it waits, the run says `retrying`, it completes
#   piece 2  `model` again: piece 1 resolved it, so it asks the Hub nothing (seconds)
#   piece 3  a model the Hub has not: refused at once (`model_resolution_failed`, under 10 s), reported by the root
# The run completes. Before the fix piece 1 fails at once and the root with it.
# params:
#   model      a small public model the pieces read raw (default proomptengineer/balloon-lora-sdxl, 176 MB)
#   outage_s   seconds the resolver stays broken after the run says it retries (default 20)
#   traffic_gb GB the provider bills: the image, the model, the fixture's environment (default 8)
. "$(dirname "$0")/lib.sh"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
FIX=$(cd "$(dirname "$0")/fixtures" && pwd)
[ -f "$FIX/hub-outage/package.toml" ] || fail "no fixture checkout"
MODEL=$(param model proomptengineer/balloon-lora-sdxl) OUTAGE=$(param outage_s 20) ABSENT=fidika/cl708-absent-$RANDOM
quote "$(param traffic_gb 8)" 1800
rent

sh() { cozy machine ssh "$NAME" -- "$*" < /dev/null 2>> "$OUT/ssh.err"; }
mend() { sh 'test -f /root/resolv.conf.cl708 && cat /root/resolv.conf.cl708 > /etc/resolv.conf'; }
trap 'mend; finish' EXIT
step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
cd "$FIX" || fail "no fixtures"
step install package install ./hub-outage --on "$NAME" -y || fail "install"
sh 'cp /etc/resolv.conf /root/resolv.conf.cl708 && printf "nameserver 127.0.0.9\n" > /etc/resolv.conf && ! getent hosts tensorhub.com' \
  || fail "the resolver did not break"
say "the pod's resolver is broken"
cozy run ./hub-outage/pieces --on "$NAME" models:="[\"$MODEL\",\"$MODEL\",\"$ABSENT\"]" pause_s:=10 > "$OUT/run.txt" 2>&1 & follower=$!
for _ in $(seq 120); do RUN=$(awk '$1=="run" && $2 ~ /^[0-9]+$/ && /\/pieces/ {print $2}' "$OUT/run.txt" | tail -1); [ -n "$RUN" ] && break; sleep 1; done
[ -n "$RUN" ] || fail "no run: $(tail -3 "$OUT/run.txt")"
seen=0 mended=0 t0=$(date +%s)
while kill -0 "$follower" 2>/dev/null; do
  t=$(( $(date +%s) - t0 ))
  said=$(cozy run show "$RUN" --once --json 2>/dev/null | jq -r '[.waiting, .progress.stage] | map(select(. != null)) | join(" | ")')
  printf '%s\t%s\n' "$t" "$said" >> "$OUT/timeline.tsv"
  [ "$seen" != 0 ] || ! grep -q retrying <<<"$said" || { seen=$t; say "t=$t the run says: ${said:0:160}"; }
  if [ "$mended" = 0 ] && { { [ "$seen" != 0 ] && [ $((t - seen)) -ge "$OUTAGE" ]; } || [ "$t" -ge 300 ]; }; then
    mend; mended=$t; say "t=$t the resolver is mended"
  fi
  [ "$t" -lt 1200 ] || { cozy run cancel "$RUN" -y; fail "run $RUN did not settle"; }
  sleep 2
done
mend
cozy run show "$RUN" --once --json > "$OUT/run.json" 2>&1
cozy machine logs "$NAME" tensord -n 0 2>/dev/null | grep 'unreachable, retrying' > "$OUT/retries.log"
verdict=$(python3 -I - "$OUT/run.json" "$seen" "$MODEL" <<'PY'
import json, sys
rec, seen, model = json.load(open(sys.argv[1])), int(sys.argv[2]), sys.argv[3]
pieces = (rec.get("result") or {}).get("pieces", [])
fails = []
if rec.get("status") != "completed": fails.append(f"the run is {rec.get('status')}: {rec.get('reason')}")
if not seen: fails.append("the run never said it retries")
if len(pieces) == 3:
    one, two, three = pieces
    if not one["checkpoint"]: fails.append(f"piece 1: {one}")
    if two["checkpoint"] != one["checkpoint"] or two["seconds"] > 30: fails.append(f"piece 2 resolved again: {two}")
    if three["code"] != "model_resolution_failed" or three["seconds"] > 10: fails.append(f"piece 3 not refused at once: {three}")
else: fails.append(f"pieces: {pieces}")
print(json.dumps({"fails": fails, "pieces": pieces}))
PY
)
say "verdict: $verdict ($(wc -l < "$OUT/retries.log") TensorD retry lines)"
jq -e '.fails == []' <<<"$verdict" > /dev/null || fail "run $RUN: $(jq -c .fails <<<"$verdict")"
say "PASS: run $RUN waited out the Hub, reused piece 1's resolution and refused an absent model at once"
