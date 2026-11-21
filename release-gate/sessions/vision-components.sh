# vision-components (xs-213 phase 3): a call that shows no picture fetches no vision tower. The coordinator asked Paul (11-18) "may
# a package declare its vision encoder separately, so a text-only call skips downloading it?"; Paul: "yes good idea". qwen3 declares
# `needs={"model.vision": …}` on `ask`, and a Qwen3.5 checkpoint is three components (`model`, `vision`, `mtp`). One rental of $SKU
# on this session's cozy (`cozy`: a dev-build run of the change) as its TensorD, the change's Runtime (`runtime`, `tensorfs`), qwen3
# from the packages_refs checkout `tree`:
# 1. on the cold machine, a text-only qwen3/ask (greedy, seed 0) waits for its checkpoint less its vision tower: what its record says
#    it waited for (`measurements.weights.needs`) and what the machine holds of the model after it are the checkpoint's bytes less
#    its `vision` component's (each within a header's bytes);
# 2. the same question about a picture then waits for the vision tower alone: its `vision` component's bytes.
# Both byte counts and both answers are said. Given `text_answer` and `picture_answer` (today's: this cell's answers on the open
# RC's builds with today's one-component commit), each answer must equal them.
# Red on today's RC and commits: the text-only ask fetches the whole checkpoint, its vision tower in it, so the picture ask waits
# for nothing (and today's commit has no `vision` component).
# params:
#   tree      the packages_refs checkout qwen3 runs from (required)
#   runtime   the cozy-runtime dev-build run id of the Runtime the rental runs (required)
#   tensorfs  the TensorFS wheel beside it (`artifact:<run>/<name>` or an https wheel; default: the rental's)
#   model     the checkpoint its `model` slot reads (default huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.2.0)
#   lane      its lane, for a release (default bf16)
#   text_answer, picture_answer   today's answers (default: none: said, not compared)
#   seconds   what the cap must buy (default 1800)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
PICTURE=$HERE/fixtures/ltx-images/teapot.jpg
TREE=$(param tree '') SRC=$(param runtime '') TFS=$(param tensorfs '')
MODEL=$(param model huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.2.0) LANE=$(param lane bf16)
WANT_TEXT=$(param text_answer '') WANT_PICTURE=$(param picture_answer '')
for tool in jq python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no dev build: dispatch with cozy=<a dev-build run of the change>"
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/qwen3/pyproject.toml" ] ||
  fail "qwen3 runs from a checkout: dispatch with -f packages_refs=<name>=<its branch> and params.tree=<name>"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
[ -f "$PICTURE" ] || fail "no $PICTURE"
[[ $SRC =~ ^[0-9]+$ ]] || fail "runtime: a cozy-runtime dev-build run id"
W=$(wheel_from "$OUT/wheels/runtime" "$SRC") && [ -n "$W" ] || fail "no manylinux wheel in cozy-runtime run $SRC"
UPDATE=(--runtime "$W")
if [ -n "$TFS" ]; then
  TW=$(wheel_from "$OUT/wheels/tensorfs" "$TFS") && [ -n "$TW" ] || fail "no TensorFS wheel from $TFS"
  UPDATE+=(--tensorfs "$TW")
fi
say "runtime ${W##*/}${TW:+, tensorfs ${TW##*/}}; qwen3 from $TREE $(git -C "$SOURCE" rev-parse --short=8 HEAD 2> /dev/null); model $MODEL"
# The checkpoint's components and their bytes, as the Hub lists them: what each call should wait for.
LANE_ARGS=(); [[ $MODEL == *@sha256:* ]] || LANE_ARGS=(--lane "$LANE")
cozy --json model show "$MODEL" "${LANE_ARGS[@]}" > "$OUT/model.json" 2> "$OUT/model.err" ||
  fail "model show $MODEL: $(tail -c 300 "$OUT/model.err")"
part() { jq -r --arg c "$1" '[.lanes[0].components[] | select($c == "" or .name == $c) | .bytes] | add // 0' "$OUT/model.json"; }
ALL=$(part '') VISION=$(part vision) REPO=${MODEL%@*}
say "$MODEL ($(jq -r '.lanes[0].lane + " " + .lanes[0].checkpoint' "$OUT/model.json")): $(jq -r '[.lanes[0].components[] | "\(.name) \(.bytes)"] | join(", ")' "$OUT/model.json") bytes"
[ "$ALL" -gt 0 ] || fail "$MODEL lists no bytes: $(head -c 300 "$OUT/model.json")"
kernels "$SOURCE/qwen3"
quote "$(param seconds 1800)"
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" "${UPDATE[@]}" -y || fail "the pod's TensorD and Runtime"
cozy machine show "$NAME" > "$OUT/show-update.txt" 2>&1; grep -E '^(version|runtime|tensorfs) ' "$OUT/show-update.txt" | tee -a "$R"

# held: the machine's bytes of the model now (`cozy machine show`).
held() { cozy --json machine show "$NAME" 2> /dev/null | jq -r --arg m "$REPO" '[.status.models[]? | select(.repository == $m) | .total_bytes // 0] | add // 0'; }
# ask <label> [picture]: the question, greedy at seed 0, its record in <label>.json; WAITED is what it waited for, ANSWER its answer.
ask() {
  local files=() prompt='In one sentence, what is a lighthouse for?' n
  [ -z "${2:-}" ] || { files=(--file "images=$PICTURE"); prompt='Describe this picture in one sentence.'; }
  step "$1" run "$SOURCE/qwen3/ask" "prompt=$prompt" max_tokens:=48 temperature:=0 seed:=0 --model "model=$MODEL" "${LANE_ARGS[@]}" \
    "${files[@]}" --on "$NAME" || fail "$1: $(tail -c 400 "$OUT/$1.txt")"
  n=$(runno "$1"); [ -n "$n" ] || fail "$1 names no run"
  (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/$1.json" 2> "$OUT/$1.err" || fail "$1: no record of run $n"
  jq -e '.status == "completed"' "$OUT/$1.json" > /dev/null || fail "$1: run $n $(jq -r '.status // "?"' "$OUT/$1.json"): $(jq -c '.error // empty' "$OUT/$1.json" | cut -c1-300)"
  # A memoized answer reads no weights, so it measures nothing.
  jq -e '(.memoized // false) | not' "$OUT/$1.json" > /dev/null || fail "$1: run $n was answered from the memo: it fetched nothing to measure"
  WAITED=$(jq -r '.measurements.weights.needs // 0' "$OUT/$1.json") ANSWER=$(jq -r '.result.texts[0] // empty' "$OUT/$1.json")
  say "$1: run $n waited for $WAITED bytes ($(jq -c '.measurements.weights // {}' "$OUT/$1.json")); answer: $ANSWER"
}
# near <a> <b>: within a header's bytes (16 MiB) of each other.
near() { local d=$(($1 - $2)); [ "${d#-}" -le $((16 << 20)) ]; }
MISMATCH=()

say "step 1: a text-only ask on the cold $NAME"
[ "$(held)" -le $((16 << 20)) ] || fail "$NAME already holds $(held) bytes of $REPO: no cold machine"
ask text
TEXT_WAITED=$WAITED TEXT_ANSWER=$ANSWER TEXT_HELD=$(held)
say "text-only: waited for $TEXT_WAITED bytes, $NAME holds $TEXT_HELD of $REPO; the checkpoint is $ALL, its vision tower $VISION"
[ "$VISION" -gt 0 ] || MISMATCH+=("$MODEL has no vision component: its vision tower is in its model component")
near "$TEXT_WAITED" $((ALL - VISION)) || MISMATCH+=("the text-only ask waited for $TEXT_WAITED bytes, not the $((ALL - VISION)) of its checkpoint less its vision tower")
near "$TEXT_HELD" $((ALL - VISION)) || MISMATCH+=("after the text-only ask $NAME holds $TEXT_HELD bytes, not $((ALL - VISION))")

say "step 2: a picture ask on $NAME"
ask picture with
PICTURE_WAITED=$WAITED PICTURE_ANSWER=$ANSWER
say "picture: waited for $PICTURE_WAITED bytes, $NAME holds $(held) of $REPO"
[ "$PICTURE_WAITED" -gt $((16 << 20)) ] && near "$PICTURE_WAITED" "$VISION" ||
  MISMATCH+=("the picture ask waited for $PICTURE_WAITED bytes, not its vision tower's $VISION")
say "BYTES text-only $TEXT_WAITED, picture $PICTURE_WAITED (checkpoint $ALL, vision $VISION)"
say "ANSWERS text-only: $(jq -Rn --arg a "$TEXT_ANSWER" '$a'); picture: $(jq -Rn --arg a "$PICTURE_ANSWER" '$a')"
[ -z "$WANT_TEXT" ] || [ "$TEXT_ANSWER" = "$WANT_TEXT" ] || MISMATCH+=("the text-only answer is not today's: $TEXT_ANSWER")
[ -z "$WANT_PICTURE" ] || [ "$PICTURE_ANSWER" = "$WANT_PICTURE" ] || MISMATCH+=("the picture answer is not today's: $PICTURE_ANSWER")
[ -n "$WANT_TEXT$WANT_PICTURE" ] || say "no text_answer or picture_answer given: the answers are said, not compared"
cozy machine logs "$NAME" tensord -n 4000 > "$OUT/tensord-$NAME.log" 2>&1 || true
[ ${#MISMATCH[@]} = 0 ] || fail "what didn't match: $(printf '%s; ' "${MISMATCH[@]}")"
say "PASS: the text-only ask fetched no vision tower ($TEXT_WAITED bytes), the picture ask fetched it alone ($PICTURE_WAITED bytes)"
