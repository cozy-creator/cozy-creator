# gpuless-place (cl-1279; Paul, 11-18: "why are you trying to place jobs on a machine that cannot run them? … You can pre-fetch on
# GPU-less machines if it makes sense"): this runner's own machine, which has no GPU, a dev home whose first-party packages come from
# the packages_refs checkout `tree`, birefnet alone (its benchmarks.json has a CPU row); Qwen Image 2 is the Hub's release. No rental. A plain project
# on `local` alone, shown by a page, takes a BiRefNet box and a Qwen Image 2 box (prompt_upsampling off):
#   - BiRefNet is guessed onto this computer and made ready there, and `cozy render` runs it there, quoted from its CPU row;
#   - Qwen is guessed nowhere; `cozy render` of it sends nothing, its box and the board say this computer has no GPU with the rental,
#     and none of its weights land here;
#   - stand-g joins the project: a stand-in that says only its Status (e2e/cli/fakemachine.py), one RTX 4090, so whatever is sent
#     there is refused. The waiting render goes there by itself (its run's POST in stand-g's log), and still nothing of it lands here.
#   - With `gpu` naming another box and its package in `local` (cl-1286), the box is the checkout's, and where it declares an
#     `estimate=` formula, that times it on stand-g from its rows there (`placed.formula_ms`, basis reference); its interface as cozy
#     reads it is kept (gpu.describe.json) for `needs-plan`.
# params:
#   tree   the packages_refs checkout (default pk)
#   local  the checkout's packages this home takes, space-separated (default birefnet)
#   gpu    the box of a function with no CPU row, {tool, fields, held: a regex of its models' repositories} (default Qwen Image 2's)
. "$(dirname "$0")/lib.sh"
TREE=$(param tree pk) LOCAL=$(param local birefnet)
GPU=$(param gpu '{"tool": "qwen-image-2/image", "fields": {"instructions": "A red bicycle against a white wall at night.", "prompt_upsampling": false}, "held": "qwen"}')
GTOOL=$(jq -r .tool <<< "$GPU") GFIELDS=$(jq -c .fields <<< "$GPU") GHELD=$(jq -r .held <<< "$GPU") || fail "gpu $GPU"
[[ $GTOOL =~ ^[a-z0-9-]+/[a-z0-9_]+$ && $GHELD =~ ^[a-z0-9.|-]+$ ]] || fail "gpu $GPU"
GPKG=${GTOOL%/*}
[[ $LOCAL =~ ^[a-z0-9\ -]+$ && $LOCAL == *birefnet* ]] || fail "local $LOCAL"
logs() {
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1
  cozy machine logs local tensord --since 3h -n 0 > "$OUT/tensord-local.log" 2>&1
}
fail() { logs; say "FAIL: $*"; RC=1; exit 1; }
FAILS=0
judge() { if eval "$1"; then say "ok: $2"; else say "NOT: $2${3:+ ($3)}"; FAILS=$((FAILS + 1)); fi; }
for tool in jq ffmpeg python3 openssl curl; do command -v "$tool" > /dev/null || fail "no $tool"; done
SOURCE=$(cd "$PACKAGES/$TREE" 2> /dev/null && pwd) || fail "no packages_refs checkout $TREE"
jq -e '[.[] | select(.function == "remove_background" and .gpus == 0)] | length == 1' "$SOURCE/birefnet/benchmarks.json" > /dev/null \
  || fail "the checkout's birefnet has no CPU row"
# A dev home, `local` from the checkout (load-ahead.sh's), every other package the Hub's.
mkdir -p "$OUT/packages" || fail "no packages folder"
for pkg in $LOCAL; do ln -sfn "$SOURCE/$pkg" "$OUT/packages/$pkg" || fail "no $pkg in the checkout"; done
SOURCE=$OUT/packages
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
for _ in $(seq 60); do
  cozy --json machine show local > "$OUT/local-before.json" 2> /dev/null
  [ "$(jq -r '.status.phase // empty' "$OUT/local-before.json" 2> /dev/null)" = ready ] && break
  sleep 5
done
[ "$(jq -r '.status.phase // empty' "$OUT/local-before.json")" = ready ] || fail "local isn't ready"
[ "$(jq '.status.gpus // [] | length' "$OUT/local-before.json")" = 0 ] || fail "local has a GPU: this session is for a computer with none"
say "cozy $(cozy --version | awk '{print $2}'), $LOCAL from $TREE ($(git -C "$PACKAGES/$TREE" rev-parse --short HEAD 2> /dev/null)); this runner: $(nproc) CPUs, $(awk '/MemTotal/ { printf "%.1f", $2 / 1048576 }' /proc/meminfo) GiB RAM"

in_project() { (cd "$RUN_PROJECT" && cozy "$@"); }
# The checkout's GPU box function as cozy reads it (cl-1286), before any box of it: its interface, for `needs-plan`.
if [[ " $LOCAL " == *" $GPKG "* ]]; then
  cozy --json --full run "$SOURCE/$GTOOL" --describe > "$OUT/gpu.describe.json" 2> "$OUT/gpu.describe.err" \
    || fail "cozy can't read the checkout's $GTOOL: $(jq -c .error "$OUT/gpu.describe.json" 2> /dev/null) $(tail -c 300 "$OUT/gpu.describe.err")"
  say "the checkout's $GTOOL estimate: $(jq -c --arg f "${GTOOL#*/}" '[.interface.entrypoints[]? | select(.name == $f) | .estimate | {size, ms: (.ms | .[0:160])}][0]' "$OUT/gpu.describe.json" 2> /dev/null)"
fi
qwen_held() { cozy --json machine show local 2> /dev/null | jq -c --arg h "$GHELD" '[.status.models[]? | select(.repository | test($h)) | .total_bytes] | add // 0'; }
nice -n 19 ffmpeg -v error -y -f lavfi -i 'testsrc2=size=1024x1024:rate=1' -frames:v 1 "$OUT/photo.png" || fail "no picture"
in_project --json machine use local > "$OUT/use.json" 2>&1 || fail "machine use local: $(tail -c 300 "$OUT/use.json")"
run_page import "$OUT/photo.png" --name photo > "$OUT/photo.json" 2>&1 || fail "import photo: $(tail -c 300 "$OUT/photo.json")"
python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/show.json" 2> "$OUT/show.err" & BG="$BG $!"
for _ in $(seq 100); do grep -q '"held"' "$OUT/show.json" 2> /dev/null && break; sleep 0.1; done
grep -q '"held"' "$OUT/show.json" || fail "no page shows the project: $(tail -c 300 "$OUT/show.err")"
QWEN0=$(qwen_held)
run_page add cut --tool birefnet/remove_background --fields '{"image": "<photo>"}' > "$OUT/add-cut.json" 2>&1 || fail "the BiRefNet box: $(tail -c 300 "$OUT/add-cut.json")"
run_page add q --tool "$GTOOL" --fields "$GFIELDS" > "$OUT/add-q.json" 2>&1 || fail "the $GTOOL box: $(tail -c 300 "$OUT/add-q.json")"

# The guesses, as the board's rows of what each box's machine has ready.
ahead() { run_page board 2> /dev/null | jq -c --arg n "$1" '[.ahead[]? | select((.names // []) | index($n))] | max_by(.run) // empty' 2> /dev/null; }
for _ in $(seq 120); do row=$(ahead cut); jq -e '.ready == true' <<< "${row:-null}" > /dev/null 2>&1 && break; sleep 5; done
say "cut's guess: ${row:-none}; q's: $(ahead q)"
judge 'jq -e ".machine == \"local\" and .ready == true" <<< "${row:-null}" > /dev/null' "BiRefNet is guessed onto this computer and made ready there" "${row:-no row}"
judge '[ -z "$(ahead q)" ]' "Qwen is guessed nowhere" "$(ahead q)"
logs
say "what Calcifer said of the guesses: $(grep -oE "(cut|q)('s guess| isn't guessed| guessed)[^\"]{0,300}" "$OUT/calcifer.log" | sort -u | paste -sd '|' | cut -c1-900)"

# rendered <box> <limit s>: `cozy render <box>` followed until it ends or the limit; its run's record kept as runs-<box>.json.
rendered() {
  local p
  (cd "$RUN_PROJECT" && exec timeout "$2" cozy render "$1") > "$OUT/render-$1.txt" 2>&1 & p=$!
  wait "$p"; say "$1: cozy render exited $? and said: $(tr '\n' '|' < "$OUT/render-$1.txt" | cut -c1-900)"
}
rendered cut 900
RUN=$(in_project --json run ls --limit 0 2> /dev/null | jq -r '[.runs[]? | select(.target | test("remove_background"))][0].number // empty')
[ -n "$RUN" ] && in_project --json --full run watch "$RUN" --once > "$OUT/runs-cut.json" 2>&1
say "cut's run: $(jq -c 'def s(a; b): if a != null and b != null then ((b - a) / 100 | round / 10) else null end;
  {number, machine, status, execution_ms, ran_s: s(.started_at_ms; .settled_at_ms), placed: (.placed | if . then {machine, basis, warm, finish_s: s(.quoted_at_ms; .finish_at_ms), prepare_s: (.prepare_ms / 1000), fetch_bytes} else null end)}' "$OUT/runs-cut.json" 2> /dev/null)"
judge 'jq -e ".status == \"completed\" and .machine == \"local\" and .placed.basis == \"reference\"" "$OUT/runs-cut.json" > /dev/null 2>&1' \
  "BiRefNet renders on this computer, quoted from its CPU row (basis reference)"

# Qwen: rendered, nothing is sent.
(cd "$RUN_PROJECT" && exec cozy render q) > "$OUT/render-q.txt" 2>&1 & QP=$!
for _ in $(seq 60); do
  run_page state > "$OUT/status-q.json" 2> /dev/null
  jq -e '[.queue[]? | select(.name == "q") | .no_gpu.line // empty] | length > 0' "$OUT/status-q.json" > /dev/null 2>&1 && break
  sleep 2
done
sleep 30
run_page state > "$OUT/status-q.json" 2> /dev/null; jq '.board' "$OUT/status-q.json" > "$OUT/board-q.json"
LINE='This computer has no GPU, and nothing measured says this runs without one: render it on a new rental.'
say "q's box: $(jq -c '[.queue[]? | select(.name == "q")]' "$OUT/status-q.json"); the board said: $(jq -c '[.said[]?.message]' "$OUT/board-q.json" | cut -c1-600); cozy render said: $(tr '\n' '|' < "$OUT/render-q.txt" | cut -c1-600)"
judge 'jq -e --arg l "$LINE" "[.queue[]? | select(.name == \"q\") | .no_gpu.line] | index(\$l) != null" "$OUT/status-q.json" > /dev/null' "Qwen's box says this computer has no GPU, with the rental"
judge 'jq -e --arg l "q: $LINE" "[.said[]?.message] | map(select(. == \$l)) | length == 1" "$OUT/board-q.json" > /dev/null' "the board says it once"
judge 'grep -qF "$LINE" "$OUT/render-q.txt"' "cozy render says it"
judge '[ -z "$(in_project --json run ls --limit 0 | jq -r --arg p "$GPKG" "[.runs[]? | select(.target | contains(\$p))][0].number // empty")" ]' "no run of $GTOOL was sent"
judge '[ "$(qwen_held)" = "$QWEN0" ]' "none of Qwen's weights landed here" "$QWEN0 bytes, then $(qwen_held)"

# stand-g joins: a stand-in with one RTX 4090 that only says its Status.
version=$(jq -r '.status.version' "$OUT/local-before.json") runtime=$(jq -r '.status.runtime // empty' "$OUT/local-before.json") tensorfs=$(jq -r '.status.tensorfs // empty' "$OUT/local-before.json") now=$(date +%s%3N)
jq -n --arg v "$version" --arg rt "$runtime" --arg tfs "$tensorfs" --argjson now "$now" '
  {worker_id: "stand-in-g", boot_id: "stand-g", version: $v, phase: "ready", platform: "linux/x86_64", runtime: $rt, tensorfs: $tfs,
   started_at_unix_ms: ($now - 60000), serving_gpus: 1, link_bytes_per_s: 1000000000, disk: {total_bytes: 322122547200, free_bytes: 300000000000},
   gpus: [{index: 0, name: "NVIDIA GeForce RTX 4090", uuid: "GPU-stand-g-0", memory_bytes: 25757220864, family: "sm89", memory_class: "discrete"}],
   models: [], queue: [], times: []}' > "$OUT/stand-g.frame.json"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/CN=stand-g" -days 2 -keyout "$OUT/stand-g.key" -out "$OUT/stand-g.pem" > /dev/null 2>&1 || fail "stand-g's certificate"
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
python3 "$(dirname "$0")/../../e2e/cli/fakemachine.py" --port "$port" --cert "$OUT/stand-g.pem" --key "$OUT/stand-g.key" --frame "$OUT/stand-g.frame.json" > "$OUT/stand-g.log" 2>&1 & BG="$BG $!"
for _ in $(seq 50); do curl -skf "https://127.0.0.1:$port/v1/status" > /dev/null 2>&1 && break; sleep 0.2; done
fp=$(openssl x509 -in "$OUT/stand-g.pem" -outform der | sha256sum | cut -c1-64)
cozy --json machine add stand-g "127.0.0.1:$port" --fingerprint "sha256:$fp" > "$OUT/stand-g-add.json" 2>&1 || fail "machine add stand-g: $(tail -c 300 "$OUT/stand-g-add.json")"
in_project --json machine use local,stand-g > "$OUT/use-g.json" 2>&1 || fail "machine use local,stand-g: $(tail -c 300 "$OUT/use-g.json")"
# Placed there: its run's POST, which stand-g refuses.
went() { grep -q '"POST /v1/runs' "$OUT/stand-g.log"; }
for _ in $(seq 60); do went && break; sleep 2; done
say "stand-g was asked: $(grep -oE '"(POST|PUT) [^ ]+' "$OUT/stand-g.log" | sort | uniq -c | tr -s ' ' | paste -sd ';')"
say "q's job on the board: $(run_page board 2> /dev/null | jq -c '[.jobs[]? | select(.asset == "q")][-1] // {} | {state, machine, run, why}')"
judge went "the waiting Qwen render went to stand-g by itself once it joined"
if [[ " $LOCAL " == *" $GPKG "* ]] && jq -e --arg f "${GTOOL#*/}" '[.interface.entrypoints[]? | select(.name == $f) | .estimate.ms // empty] | length > 0' "$OUT/gpu.describe.json" > /dev/null 2>&1; then
  QRUN=$(run_page board 2> /dev/null | jq -r '[.jobs[]? | select(.asset == "q") | .run // empty][-1] // empty')
  [ -n "$QRUN" ] && in_project --json --full run watch "$QRUN" --once > "$OUT/runs-q.json" 2>&1
  say "q's run on stand-g: $(jq -c 'def s(a; b): if a != null and b != null then ((b - a) / 100 | round / 10) else null end;
    {number, machine, placed: (.placed | if . then {machine, basis, formula_ms, size, finish_s: s(.quoted_at_ms; .finish_at_ms), prepare_s: (.prepare_ms / 1000)} else null end),
     models: [.spec.models[]? | {model, lane, bytes}]}' "$OUT/runs-q.json" 2> /dev/null)"
  judge 'jq -e ".placed.machine == \"stand-g\" and .placed.basis == \"reference\" and (.placed.formula_ms // 0) > 0" "$OUT/runs-q.json" > /dev/null 2>&1' \
    "$GTOOL is quoted on stand-g by its own formula over its RTX 4090 rows (basis reference)"
fi
run_page cancel q > "$OUT/cancel-q.txt" 2>&1; kill "$QP" 2> /dev/null; wait "$QP" 2> /dev/null
sleep 20
judge '[ "$(qwen_held)" = "$QWEN0" ]' "still none of Qwen's weights landed here" "$QWEN0 bytes, then $(qwen_held)"
timeout 60 cozy --json machine end stand-g --force -y > "$OUT/stand-g-end.json" 2>&1 || say "stand-g end: $(tail -c 200 "$OUT/stand-g-end.json")"
logs
say "what Calcifer said of placement: $(grep -E "goes to|waits for a machine|isn't planned|guess" "$OUT/calcifer.log" | grep -vE 'GET|PUT /v1/events' | tail -20 | cut -c1-300 | paste -sd '|')"
[ "$FAILS" = 0 ] || fail "$FAILS of the judged facts did not hold"
say "PASS"
