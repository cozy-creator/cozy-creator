# estimate-place (cl-1286; Paul, 11-18: "how long will X workload take with model Y on GPU G … from a formula, with the formula
# hand-built per package based on measured times we store in the same benchmark"): one pod of `sku`, a project on it alone, shown by a
# page, and two boxes of the packages_refs checkout `tree`: BiRefNet's cut (no `estimate=`: the platform's default from its row of the
# card) rendered to its end, then an H3 shot (its `estimate=` formula over its row of the card), canceled once placed. Each run's
# `placed` (the quote: basis, formula_ms, size, finish, preparation) and its models' lanes are said. Each interface as cozy reads it
# (<box>.describe.json) and the card (card.json) are kept, for `needs-plan` to give the same plan from the same rows. It judges that
# both were placed on the pod by its rows (basis reference), the shot by its formula (formula_ms) and the cut by none.
# params:
#   tree    the packages_refs checkout (default pk)
#   shot    the H3 shot box's fields (default one 15 s beat, Turbo)
#   need_s  the pod's time the quote asks for (default 1500)
. "$(dirname "$0")/lib.sh"
TREE=$(param tree pk) NEED=$(param need_s 1500)
SHOT=$(param shot '{"summary": "A lighthouse on a cliff at dusk.", "beats": [{"duration": 15, "description": "Waves break below the lighthouse as its lamp turns."}]}')
logs() {
  cozy machine logs local calcifer --lines 20000 > "$OUT/calcifer.log" 2>&1
  [ -z "$NAME" ] || cozy machine logs "$NAME" tensord --since 2h -n 0 > "$OUT/tensord-pod.log" 2>&1
}
fail() { logs; say "FAIL: $*"; RC=1; exit 1; }
FAILS=0
judge() { if eval "$1"; then say "ok: $2"; else say "NOT: $2${3:+ ($3)}"; FAILS=$((FAILS + 1)); fi; }
for tool in jq ffmpeg python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
[[ $NEED =~ ^[0-9]+$ ]] && jq -e '.beats | length > 0' <<< "$SHOT" > /dev/null || fail "need_s $NEED, shot $SHOT"
CHECKOUT=$(cd "$PACKAGES/$TREE" 2> /dev/null && pwd) || fail "no packages_refs checkout $TREE"
# A dev home: birefnet and minimax-h3 from the checkout, every other package the Hub's.
SOURCE=$OUT/packages
mkdir -p "$SOURCE" || fail "no packages folder"
for pkg in birefnet minimax-h3; do ln -sfn "$CHECKOUT/$pkg" "$SOURCE/$pkg" || fail "no $pkg in the checkout"; done
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
awk -v source="$SOURCE" '
  function ours() { print "diagnostics = true"; print "packages = \"" source "\""; put = 1 }
  /^(diagnostics|packages) *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
  "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "[calcifer] not written"
cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
say "cozy $(cozy --version | awk '{print $2}'), birefnet and minimax-h3 from $TREE ($(git -C "$CHECKOUT" rev-parse --short HEAD 2> /dev/null))"
# Each function as cozy reads it, before any box of it: what `needs-plan` plans from.
for f in cut:birefnet/remove_background shot:minimax-h3/generate; do
  cozy --json --full run "$SOURCE/${f#*:}" --describe > "$OUT/${f%%:*}.describe.json" 2> "$OUT/${f%%:*}.describe.err" \
    || fail "cozy can't read the checkout's ${f#*:}: $(jq -c .error "$OUT/${f%%:*}.describe.json" 2> /dev/null) $(tail -c 300 "$OUT/${f%%:*}.describe.err")"
done
say "the shot's estimate: $(jq -c '[.interface.entrypoints[]? | select(.name == "generate") | .estimate.size][0]' "$OUT/shot.describe.json")"

quote "$NEED"
rent
in_project() { (cd "$RUN_PROJECT" && cozy "$@"); }
cozy --json machine show "$NAME" > "$OUT/pod.json" 2>&1 || fail "machine show $NAME"
jq '{gpu: .status.gpus[0].name, gpus: .status.serving_gpus, memory_bytes: (.status.gpus[0] | if (.memory_limit_bytes // 0) > 0 then [.memory_limit_bytes, .memory_bytes] | min else .memory_bytes end)}' \
  "$OUT/pod.json" > "$OUT/card.json" || fail "the pod's card"
say "$NAME: $(jq -c . "$OUT/card.json"), cozy $(jq -r '.status.version // empty' "$OUT/pod.json")"
in_project --json machine use "$NAME" > "$OUT/use.json" 2>&1 || fail "machine use $NAME: $(tail -c 300 "$OUT/use.json")"
python3 "$RUN_PAGE" show "$RUN_PROJECT" > "$OUT/show.json" 2> "$OUT/show.err" & BG="$BG $!"
for _ in $(seq 100); do grep -q '"held"' "$OUT/show.json" 2> /dev/null && break; sleep 0.1; done
grep -q '"held"' "$OUT/show.json" || fail "no page shows the project: $(tail -c 300 "$OUT/show.err")"
nice -n 19 ffmpeg -v error -y -f lavfi -i 'testsrc2=size=1024x1024:rate=1' -frames:v 1 "$OUT/photo.png" || fail "no picture"
run_page import "$OUT/photo.png" --name photo > "$OUT/photo.json" 2>&1 || fail "import photo: $(tail -c 300 "$OUT/photo.json")"
run_page add cut --tool birefnet/remove_background --fields '{"image": "<photo>"}' > "$OUT/add-cut.json" 2>&1 || fail "the cut box: $(tail -c 300 "$OUT/add-cut.json")"
run_page add shot --tool minimax-h3/generate --fields "$SHOT" > "$OUT/add-shot.json" 2>&1 || fail "the shot box: $(tail -c 300 "$OUT/add-shot.json")"

# placed <box>: its newest run's record (runs-<box>.json), and its quote said.
job_run() { run_page board 2> /dev/null | jq -r --arg a "$1" '[.jobs[]? | select(.asset == $a) | .run // empty][-1] // empty'; }
placed() {
  in_project --json --full run watch "$1" --once > "$OUT/runs-$2.json" 2>&1
  say "$2's run: $(jq -c 'def s(a; b): if a != null and b != null then ((b - a) / 100 | round / 10) else null end;
    {number, machine, status, execution_ms, placed: (.placed | if . then {machine, basis, warm, formula_ms, size, finish_s: s(.quoted_at_ms; .finish_at_ms), prepare_s: (.prepare_ms / 1000), fetch_bytes} else null end),
     models: [.spec.models[]? | {model, lane, bytes}]}' "$OUT/runs-$2.json" 2> /dev/null)"
}
(cd "$RUN_PROJECT" && exec timeout 1200 cozy render cut) > "$OUT/render-cut.txt" 2>&1
say "cut: cozy render exited $? and said: $(tr '\n' '|' < "$OUT/render-cut.txt" | cut -c1-600)"
RUN=$(job_run cut)
[ -n "$RUN" ] && placed "$RUN" cut
judge 'jq -e --arg m "$NAME" ".machine == \$m and .placed.machine == \$m and .placed.basis == \"reference\" and .placed.formula_ms == null" "$OUT/runs-cut.json" > /dev/null 2>&1' \
  "BiRefNet is placed on $NAME by the platform's default from its row of the card (basis reference, no formula)"

(cd "$RUN_PROJECT" && exec cozy render shot) > "$OUT/render-shot.txt" 2>&1 & SP=$!
RUN=''
for _ in $(seq 180); do RUN=$(job_run shot); [ -n "$RUN" ] && break; sleep 5; done
[ -n "$RUN" ] && placed "$RUN" shot
judge 'jq -e --arg m "$NAME" ".placed.machine == \$m and .placed.basis == \"reference\" and (.placed.formula_ms // 0) > 0" "$OUT/runs-shot.json" > /dev/null 2>&1' \
  "the H3 shot is placed on $NAME by its own formula over its row of the card (basis reference, formula_ms)"
run_page cancel shot > "$OUT/cancel-shot.txt" 2>&1; kill "$SP" 2> /dev/null; wait "$SP" 2> /dev/null
logs
say "what Calcifer said of placement: $(grep -E "goes to|guessed on|isn't guessed" "$OUT/calcifer.log" | grep -vE 'GET|PUT /v1/events' | tail -12 | cut -c1-300 | paste -sd '|')"
[ "$FAILS" = 0 ] || fail "$FAILS of the judged facts did not hold"
say "PASS"
