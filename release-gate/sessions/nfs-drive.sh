# nfs-drive (cl-1282; Paul, 11-18: "you need to factor into account NFS drives as well with your estimates (runpod only)"): what a
# pod with its datacenter's NFS drive is quoted for a model's download, against what it took, by tier. One rental asked of RunPod
# (its Hub then ranks a datacenter with a drive first), from a dev home, every step through this runner's cozy; the pod runs the
# session's TensorD. A box of `tool`, whose model is `model`, rendered on the pod alone:
#   1. cold:  on the fresh pod, no page showing the project. What came from the Hub is written to the drive behind the run, and the
#             session waits for that.
#   2. drive: the pod's service restarted and `model` uninstalled, so its store holds none of it and its drive all of it; rendered
#             again on another seed.
#   3. ahead: uninstalled again, then a page shows the project: the box's guess is made ready on the pod, its row's bytes sampled;
#             then rendered.
# Each render says its quote (`placed`), what its run waited for (`measurements.weights`), what the pod says of itself
# (`cozy machine show`) and what TensorFS landed by tier (the pod's `landed` lines since the step began). It judges none of the
# numbers: it fails when the pod mounted no drive, or a step does.
# The $0 pre-flight also says what each of `candidates` costs on RunPod. params:
#   tool, fields   the box (default sdxl/generate, a prompt); model, its repository (default stabilityai/sdxl)
#   candidates     SKUs priced in the pre-flight, space-separated
#   datacenters    those the quote may name, space-separated, or nothing is ordered (default: the six with a standard drive on 11-09,
#                  xs-041; empty: any)
#   need_s         the seconds the cap must buy (default 1800)
. "$(dirname "$0")/lib.sh"
TOOL=$(param tool sdxl/generate) MODEL=$(param model stabilityai/sdxl)
FIELDS=$(param fields '{"prompt": "a brass compass on a walnut desk", "seed": 1282}')
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build run id>"
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
RENT_ARGS='--provider runpod'
for s in $(param candidates ''); do
  cozy machine rent "$s" --disk-gb "$DISK" $RENT_ARGS --json < /dev/null > "$OUT/candidate-$s.json" 2> "$OUT/candidate-$s.err" || true
  say "candidate $s on RunPod: $(jq -R -c 'fromjson? | select(.quote) | .quote' "$OUT/candidate-$s.json" | head -1 | cut -c1-400) $(grep -v quote_unagreed "$OUT/candidate-$s.err" | head -c 200)"
done
# quote, with the pre-flight ending after the check below: the quote must name one of `datacenters`.
WANT_DC=$(param datacenters 'EU-NL-1 US-NC-1 EU-CZ-1 EUR-NO-1 EU-RO-1 EUR-IS-1')
ASKED=$RENT; RENT=true
quote "$(param need_s 1800)"
RENT=$ASKED
QUOTED_DC=$(jq -r '.quote.datacenter // empty' "$OUT/quote.json")
[ -z "$WANT_DC" ] || [[ -n $QUOTED_DC && " $WANT_DC " == *" $QUOTED_DC "* ]] ||
  fail "the quote is in ${QUOTED_DC:-no datacenter it says}, none of $WANT_DC: nothing ordered"
[ "$RENT" = true ] || { say "pre-flight only: nothing rented"; exit 0; }
rent
jq -e '.rental.nfs_drive == true' "$OUT/show.json" > /dev/null ||
  fail "$NAME mounted no NFS drive: $(jq -c '{provider: .rental.provider, datacenter: .rental.datacenter}' "$OUT/show.json")"
say "$NAME: $(jq -c '{provider: .rental.provider, datacenter: .rental.datacenter, nfs_drive: .rental.nfs_drive}' "$OUT/show.json")"

# The RC's builds, as a dev home moves a rental at boot, then the session's TensorD, held.
for _ in $(seq 72); do [ "$(cozy machine show "$NAME" --json 2> /dev/null | jq -r '.rc.state // empty')" = current ] && break; sleep 5; done
[ "$(cozy machine show "$NAME" --json | jq -r '.rc.state // empty')" = current ] || step rc machine update "$NAME" -y || fail "$NAME didn't move to the RC"
step held machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod refused the session's TensorD"
ours() {
  local v; v=$(cozy machine show "$NAME" --json 2> /dev/null | jq -r '.status.version // empty')
  [ "$v" = "$V" ] || fail "$1: the pod runs TensorD ${v:-nothing it says}, not the session's $V"
}
ours start
(cd "$RUN_PROJECT" && cozy machine use "$NAME") > "$OUT/use.txt" 2>&1 || fail "machine use $NAME: $(tail -c 300 "$OUT/use.txt")"
run_page add draw --tool "$TOOL" --fields "$FIELDS" > "$OUT/box.json" 2>&1 || fail "the box: $(tail -c 300 "$OUT/box.json")"

# pod <file>: what the pod says of itself now; said: its link and drive rates, disk, and what its store and drive hold of the model.
pod() {
  cozy machine show "$NAME" --json > "$1" 2> /dev/null || true
  jq -c --arg m "$MODEL" '{link_bytes_per_s: .status.link_bytes_per_s, drive_bytes_per_s: .status.drive_bytes_per_s, free: .status.disk.free_bytes,
    model: [.status.models[]? | select(.repository == $m) | .checkpoints[]? | {lane, bytes, held_bytes, drive_bytes, complete}],
    reads: [.status.times[]?.reads[]? | select(.model == $m) | {bytes, missing, drive}],
    downloads: [.status.activity[]? | select(.kind == "download") | {subject, bytes_done, bytes_total, bytes_here, bytes_per_s}]}' "$1" 2> /dev/null
}
# landed <step> <since, Unix s>: what TensorFS landed on the pod since then, by tier, from its log's window.
landed() {
  cozy machine logs "$NAME" --since "$(( ($(date +%s) - $2) / 60 + 1 ))m" -n 0 > "$OUT/$1-tensord.log" 2>&1 || say "$1: the pod's log wasn't read"
  python3 -I - "$OUT/$1-tensord.log" "$2" <<'PY' | sed "s/^/$1: /" | tee -a "$R"
import datetime, re, sys
log, since = sys.argv[1], float(sys.argv[2]) - 5
tiers = {}
for line in open(log, errors="replace"):
    found = re.search(r"landed object=\w+ bytes=(\d+) kept=(\d+) tier=(\w+) seconds=([\d.e+-]+)", line)
    stamp = line.split(" ", 1)[0]
    if not found or not stamp[:4].isdigit():
        continue
    at = datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp()
    if at < since:
        continue
    size, kept, tier, took = int(found.group(1)), int(found.group(2)), found.group(3), float(found.group(4))
    tiers.setdefault(tier, []).append((at - took, at, size - kept))
if not tiers:
    print("landed nothing")
for tier, objects in sorted(tiers.items()):
    objects.sort()
    busy, end = 0.0, objects[0][0]
    for began, at, _ in objects:
        busy += max(0.0, at - max(began, end)); end = max(end, at)
    size = sum(o[2] for o in objects)
    wall = max(o[1] for o in objects) - objects[0][0]
    print(f"tier {tier}: {len(objects)} objects, {size} bytes, {wall:.1f} s first fetch to last landing, {busy:.1f} s with one in flight, "
          f"{size / max(busy, 1e-3) / 1e6:.0f} MB/s")
PY
}
# record <file>: draw's newest run's whole record.
record() {
  local n; n=$(run_page asset draw | jq -r '.versions[-1].runs[-1] // empty')
  [ -n "$n" ] && (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$1" 2>&1
}
# render <step>: draw rendered on the pod: its quote against what happened.
render() {
  local n=$1 from; from=$(date +%s)
  ours "$n"
  say "$n: before: $(pod "$OUT/$n-before.json")"
  run_page fit draw > "$OUT/$n-fit.json" 2>&1 || true
  timeout 1500 python3 "$RUN_PAGE" render "$RUN_PROJECT" --on "$NAME" draw > "$OUT/$n.txt" 2>&1 || fail "$n: $(tail -c 400 "$OUT/$n.txt")"
  record "$OUT/$n.json" || fail "$n has no run record: $(tail -c 300 "$OUT/$n.txt")"
  say "$n: fit before it $(jq -c '.estimates[0] // {}' "$OUT/$n-fit.json" 2> /dev/null | cut -c1-400)"
  say "$n: quoted $(jq -c .placed "$OUT/$n.json"), $(( ($(jq -r '.placed.finish_at_ms // 0' "$OUT/$n.json") - $(jq -r '.placed.quoted_at_ms // 0' "$OUT/$n.json")) / 1000 )) s to its finish"
  say "$n: took $(jq -c '{queued_ms: ((.started_at_ms // 0) - (.accepted_at_ms // 0)), execution_ms, whole_ms: ((.settled_at_ms // 0) - (.accepted_at_ms // 0))}' "$OUT/$n.json"); weights $(jq -c .measurements.weights "$OUT/$n.json")"
  say "$n: after: $(pod "$OUT/$n-after.json")"
  landed "$n" "$from"
}
# absent <step>: no executor holds the model (the pod's service starts again) and its store none of it.
absent() {
  step "$1-restart" machine update "$NAME" --gpu-memory-limit off -y || fail "$1: the pod's restart"
  ours "$1"
  step "$1-uninstall" model uninstall "$MODEL" --on "$NAME" || fail "$1: uninstall"
  say "$1: $(tail -2 "$OUT/$1-uninstall.txt" | tr '\n' ' '); the pod: $(pod "$OUT/$1-absent.json")"
  run_page set draw --fields "{\"seed\": $2}" > "$OUT/$1-seed.txt" 2>&1 || fail "$1: seed: $(tail -c 300 "$OUT/$1-seed.txt")"
}

say "step 1: cold, no page showing the project"
render cold
# What the Hub sent is written to the drive behind the run, two objects at a time (about 65 MB/s, xs-041).
hub=$(awk '/^cold: tier Origin:/ {print $6}' "$R" | tail -1)
wait_s=$(( ${hub:-0} / 40000000 + 30 )); [ "$wait_s" -le 600 ] || wait_s=600
say "the Hub sent ${hub:-0} bytes: ${wait_s} s for them to reach the drive"
sleep "$wait_s"

say "step 2: drive, the model on the pod's drive and none of it in its store"
absent drive 1283
render drive

say "step 3: ahead, a page shows the project"
absent ahead 1284
from=$(date +%s)
python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/shown.txt" 2>&1 & SHOW=$!; BG="$BG $SHOW"
row() { run_page board | jq -c '[.ahead[]? | select((.names // []) | index("draw"))] | max_by(.run) // empty' 2> /dev/null; }
READY=''
for _ in $(seq 600); do
  READY=$(row)
  printf '{"at_ms":%s,"row":%s,"pod":%s}\n' "$(date +%s%3N)" "${READY:-null}" "$(pod "$OUT/ahead-now.json" || echo null)" >> "$OUT/ahead.ndjson"
  jq -e --arg m "$NAME" '.ready == true and .machine == $m' <<< "${READY:-null}" > /dev/null 2>&1 && break
  sleep 0.5
done
jq -e '.ready == true' <<< "${READY:-null}" > /dev/null 2>&1 || fail "draw's guess wasn't ready on $NAME in 5 min: ${READY:-no row}"
say "ahead: the box's row, first with bytes: $(jq -c 'select((.row.total // 0) > 0) | .row' "$OUT/ahead.ndjson" | head -1); the most it counted: $(jq -s -c '[.[].row | select(. != null)] | max_by(.total) // {} | {stage, done, total}' "$OUT/ahead.ndjson"); ready: $READY"
say "ahead: the pod's download, the most it listed: $(jq -s -c '[.[].pod.downloads[]?] | max_by(.bytes_total) // {}' "$OUT/ahead.ndjson"); ready $(( $(date +%s) - from )) s after the page showed"
landed ahead "$from"
render ahead-render
kill "$SHOW" 2> /dev/null
cozy machine show "$NAME" > "$OUT/show-end.txt" 2>&1; grep -iE '^(link|download|nfs|provider|disk)' "$OUT/show-end.txt" | tee -a "$R"
cozy machine logs local calcifer --lines 4000 > "$OUT/calcifer.log" 2>&1 || true
say "PASS: quoted against actual said for cold, drive and ahead on $NAME"
