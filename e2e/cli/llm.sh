# xs-204 (Paul, 11-16): "ideally I want LLM tools as well. Like I can create a text box, and then type text into it. Then I can
# attach an LLM and it can generate more text. Or I can attach an LLM (ideally VLLM) to an image node, or a video node, and ask it
# to produce a caption, and it'll produce text".

# se-145, Paul 11-16: "but the user can override the checkpoint used? ideally use only abliterated checkpoints of it, not the original."

# qwen3's `write` tool on the real Qwen3.5 0.8B, abliterated, picked by `--model`, on this computer's CPU through `cozy run`: words
# from words, then one answer about a picture and a clip its instructions mention. Each answer is words.
llm_write() {
[ -n "$LOCAL" ] || return 0
[ -n "$MODEL" ] || { echo "qwen3's write: not selected (ci/affected's model)"; return 0; }
step "an LLM tool writes words, and of a picture and a clip it's shown (xs-204)"
local tool=qwen3/write dir=$OUT/llm pic clip prompt
[ -z "$PACKAGES" ] || tool=$(cd "$PACKAGES" && printf '%s/qwen3/write\n' "$PWD")
local qwen=(--model model=huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0 max_tokens:=32 seed:=7)
if cell llm-words "$tool" 'instructions=Continue this in one sentence: The sea at night' "${qwen[@]}"; then
  wrote llm-words
fi
mkdir -p "$dir"
cp "$(picture dog1.jpg)" "$dir/dog.jpg"
ffmpeg -v error -y -f lavfi -i testsrc2=size=128x96:rate=8:duration=1 -c:v libx264 -pix_fmt yuv420p "$dir/clip.mp4" \
  || { check "llm: ffmpeg made no clip"; return; }
project_import "$RUN_PROJECT" "$dir/dog.jpg" llm_dog > /dev/null || { check "llm: import dog.jpg"; return; }
project_import "$RUN_PROJECT" "$dir/clip.mp4" llm_clip > /dev/null || { check "llm: import clip.mp4"; return; }
for _ in $(seq 100); do [ "$(row "$RUN_PROJECT" llm_dog '.sha256 // empty')" ] && [ "$(row "$RUN_PROJECT" llm_clip '.sha256 // empty')" ] && break; sleep 0.2; done
pic=$(row "$RUN_PROJECT" llm_dog '.sha256 // empty' | jq -r .) clip=$(row "$RUN_PROJECT" llm_clip '.sha256 // empty' | jq -r .)
[ -n "$pic" ] && [ -n "$clip" ] || { check "llm: the imports hold no files: $pic $clip"; return; }
prompt=$(jq -cn --arg a "sha256:${pic#sha256:}" --arg v "sha256:${clip#sha256:}" \
  '{text: "In one sentence: what is in <llm_dog>, and what does <llm_clip> show?",
    references: [{key: "llm_dog", type: "image", file: $a}, {key: "llm_clip", type: "video", file: $v}]}')
if cell llm-seen "$tool" "instructions:=$prompt" "${qwen[@]}"; then
  wrote llm-seen
  J '[.adjustments[]? | select(.kind == "ignored")] | length == 0' "$OUT/llm-seen.json" \
    || check "llm-seen: an input was left out: $(jq -c '[.adjustments[]? | select(.kind == "ignored")]' "$OUT/llm-seen.json")"
fi
}

# wrote <cell>: its result's `text` holds words.
wrote() {
local said
said=$(run_get "$OUT" "$1" .result.text)
echo "$1: $said"
[ -n "${said//[[:space:]]/}" ] || check "$1: the answer is no words: $(jq -c .result "$OUT/$1.json" 2> /dev/null | cut -c1-300)"
}

# xs-209 (Paul, 11-17): "we can type text into boxes and get a reply from it (ideally streaming the results), and also we can get captions
# for images (ideally streaming)": the page cell web/llm-stream.spec.js, on this shard's Qwen3.5 0.8B.
llm_stream() {
[ -n "$LOCAL" ] || return 0
web_alone llm-stream "an LLM's words grow on its box while it writes them (xs-209)"
}

# se-151 (Paul, 11-18): "add the caption tool to an image, (2) run it, it produces a caption, (3) then add the upscale tool, and use that
# caption as one of its inputs": the page cell web/caption.spec.js, qwen3's Caption on this shard's Qwen3.5 0.8B, of torchvision's dog.
llm_caption() {
[ -n "$LOCAL" ] || return 0
local dog
dog=$(picture dog1.jpg) || return 0
PICTURE=$(web_path "$dog") web_alone caption "Caption on a picture, and its words Upscale's prompt (se-151)"
}

# web_alone <cell> <what>: the page cell web/<cell>.spec.js by its own config (web/<cell>.config.js): the web suite's fixtures and Chrome,
# without its setup, on this shard's Calcifer and models. $PICTURE goes to the cell as it is.
web_alone() {
local cell=$1 pw=$OUT/playwright-test web=$OUT/web chrome code=0 line
step "web-$cell: $2"
chrome=$(web_chrome)
[ -n "$chrome" ] && [ -x "$chrome" ] || { check "$cell: no Google Chrome on this runner (${chrome:-none})"; return; }
npm install -s --prefix "$(web_path "$pw")" @playwright/test@1.55.0 > "$OUT/web-npm.log" 2>&1 \
  || { check "$cell: npm install @playwright/test failed: $(tail -c 300 "$OUT/web-npm.log")"; return; }
mkdir -p "$web" && rm -f "$web/report.json"
HOLDER=$(web_path "$HOME/.cozy/calcifer/calcifer.lock.holder") COZY=$(web_path "$COZY") CHROME=$chrome OUT=$(web_path "$OUT") ARGV_LOG=$(web_path "$OUT/argv.log") \
  PICTURE=${PICTURE:-} NODE_PATH=$(web_path "$pw/node_modules") node "$(web_path "$pw/node_modules/@playwright/test/cli.js")" test -c "$(web_path "$here/web/$cell.config.js")" \
  > "$web/$cell.log" 2>&1 || code=$?
node "$(web_path "$here/web/report.js")" "$(web_path "$web/report.json")" "$cell" > "$web/$cell.said" 2>&1 \
  || { check "$cell: no report (exit $code): $(tail -c 600 "$web/$cell.log")"; return; }
while IFS= read -r line; do
  case $line in FAILED\ *) check "${line#FAILED }" ;; *) echo "$line" ;; esac
done < "$web/$cell.said"
[ "$code" = 0 ] || grep -q '^FAILED' "$web/$cell.said" || check "$cell: Playwright exited $code: $(tail -c 600 "$web/$cell.log")"
}

# xs-209 (Paul, 11-17): "Make sure our machines are speaking the standard Responses-API (ChatGPT / openai standardized). That should make it
# easy to consume the results as a delta, including by other tools." OpenAI's own Python SDK, `client.responses.create(stream=True)`,
# against this computer's Calcifer with the CLI's bearer as its API key: Qwen3.5 0.8B on the CPU answers through the qwen3 package's Write
# tool, its words as deltas well before it completes, adding up to its text; not streamed, a picture sent as a data: URL is answered whole.
responses_sdk() {
[ -n "$LOCAL" ] || return 0
step "OpenAI's SDK streams an answer from Calcifer's Responses API (xs-209)"
local holder=$HOME/.cozy/calcifer/calcifer.lock.holder package=qwen3.5 said
[ -z "$PACKAGES" ] || [ ! -d "$PACKAGES/qwen3" ] || package=qwen3
# Its project by its folder, as a run names one.
if ! uv run -q --no-project --with 'openai>=1.80' python3 "$here/responses_sdk.py" --base "http://$(jq -r .address < "$holder")/v1/openai" \
  --key "$(jq -r .token < "$holder")" --project "$RUN_PROJECT" --model "$package/write:huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0" > "$OUT/responses.txt" 2>&1; then
  said=$(grep '^FAIL ' "$OUT/responses.txt" | sed 's/^FAIL //' | paste -sd ';' - || true)
  check "responses: ${said:-$(tail -c 600 "$OUT/responses.txt")}"
fi
tail -n 1 "$OUT/responses.txt" | cut -c1-2000
}

# xs-209 (the lead, 11-17, after Paul's "all llm-output should be streaming where possible"): `cozy run` writes an LLM's words on stdout
# as they come, its answer only, and its record after them leaves them out. Qwen3.5 0.8B on the CPU through the qwen3 package's Write tool
# (qwen3.5's before it): its first words reach stdout well before `cozy run` ends.
llm_cli_words() {
[ -n "$LOCAL" ] || return 0
step "cozy run writes an LLM's words as they come (xs-209)"
local package=qwen3.5 said
[ -z "$PACKAGES" ] || [ ! -d "$PACKAGES/qwen3" ] || package=qwen3
if ! (cd "$RUN_PROJECT" && cozy run "$package/write" 'instructions=Write eight sentences about a lighthouse at night.' \
  --model model=huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0 max_tokens:=240 seed:=7 temperature:=0 --on local 2> "$OUT/cli-words.err") \
  | python3 -I "$here/timed.py" > "$OUT/cli-words.json"; then
  check "cli-words: cozy run failed: $(tail -c 400 "$OUT/cli-words.err")"
  return
fi
said=$(jq -r '"first words at \(.first_s) s of \(.end_s) s, \(.chunks) writes"' "$OUT/cli-words.json")
echo "cli-words: $said: $(jq -r .text "$OUT/cli-words.json" | head -c 200)"
J '.chunks >= 3 and .first_s < .end_s - 2' "$OUT/cli-words.json" \
  || check "cli-words: its words came only at its end ($said): $(jq -r .text "$OUT/cli-words.json" | head -c 300)"
}

# cr-346 (Paul, 11-18: "our runtime needs to support this for sure. Any sort of drafting / top-k 20 we need to support"; and models
# run as their authors built them): `lm.generate` picks each token as vLLM 0.19.1 does, penalties before top-k. e2e/cli/sampler decodes
# the real Qwen3.5 0.8B on this computer's CPU through `cozy run`: Qwen-Image-2.1's t2i enhancer settings (presence 1.5, top-k 20,
# top-p 0.95) at seeds 7 and 8, a top-k that binds at every step on long prose at seeds 1-4 (`order-*`: top-k 5, presence 1.5), and
# every setting (`knobs`). Each
# step's token is scored against vLLM's own sampler functions on the logits the Runtime saw, with its noise. Each claim fails on its own:
# a token off vLLM's draw (the order), a call that can't decode (the knobs), the Runtime's kept set not vLLM's (the shared sampler), and
# the `order-*` calls must reach a step where the RC's order (top-k before the penalty) draws another token, or they prove nothing.
llm_sampling() {
[ -n "$LOCAL" ] || return 0
step "lm.generate samples as vLLM does: penalties, then temperature, min-p, top-k and top-p (cr-346)"
local said bad f=$OUT/llm-order.json
cell llm-order "$here/sampler/order" --model model=huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0 || return 0
said=$(jq -c '[.result.calls[] | {name, seed, steps, mismatches, first_mismatch, kept_equal, old_kept_differs, old_draw_differs,
  kept_mean, error}]' "$f")
echo "llm-order: $said"
bad=$(jq -c '[.result.calls[] | select(.error == "" and .mismatches != 0) | {name, steps, mismatches, first_mismatch}]' "$f")
[ "$bad" = '[]' ] || check "llm-order: tokens off vLLM's draw from their logits (the order): $bad"
bad=$(jq -c '[.result.calls[] | select(.error != "" or .steps <= 20) | {name, steps, error}]' "$f")
[ "$bad" = '[]' ] || check "llm-order: a call did not decode (the settings): $bad"
bad=$(jq -c '[.result.calls[] | select(.error == "" and .kept_equal != .steps) | {name, steps, kept_equal}]' "$f")
[ "$bad" = '[]' ] || check "llm-order: the Runtime's kept tokens are not vLLM's at every step (cozy_runtime.sampling): $bad"
J '.result.calls | length == 7 and ([.[] | select(.name | startswith("order-")) | .old_draw_differs] | add > 0)' "$f" \
  || check "llm-order: the RC's order never draws another token in the order calls, so they show nothing: $said"
}

# cr-351 (the coordinator, 11-19, from xs-222's audit: MiniMax-H3-Image's expander thinks freely, then answers to a strict JSON
# schema upstream): a constraint that starts after a marker. Qwen3.5 0.8B on this computer's CPU through `cozy run` writes a free
# first line, then answers held to a small JSON schema (`VisionLM.ask`'s `allowed` as `lm.After` the newline over an `lm.Grammar`
# of outlines' index), at three seeds: each first line is prose and each answer after it parses exactly. A thinking model's path,
# the same `After` on `</think>` that `ask` wraps itself, runs on a GPU (release-gate/sessions/llm-sampling.sh): the 0.8B thinks
# past any budget a CPU affords. Qwen's precise profile (T 0.6, top_p 0.95, top_k 20). Then a conversation (`vlm.Turn`s before
# the question, the way PAN2's correction turn and qwen-image-2's enhancer need): the answer names what only the earlier turns said.
llm_think_schema() {
[ -n "$LOCAL" ] || return 0
step "an LLM thinks freely, then answers to a schema; a conversation's earlier turns are read (cr-351)"
local qwen=(--model model=huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0) f=$OUT/llm-think.json g=$OUT/llm-turns.json
if cell llm-think "$here/sampler/free_line_then_schema" "${qwen[@]}"; then
  echo "llm-think: $(jq -c '.result | {answers: [.answers[] | .[:200]], free, parsed, error}' "$f")"
  J '.result.error == ""' "$f" || check "llm-think: not run: $(jq -r .result.error "$f")"
  J '.result.free | length == 3 and all' "$f" || check "llm-think: a seed's first line isn't free prose (the constraint started before its marker): $(jq -c '.result.answers | map(.[:120])' "$f" | head -c 600)"
  J '.result.parsed | length == 3 and all' "$f" || check "llm-think: a seed's answer after its marker doesn't parse exactly to the schema: $(jq -c '[.result.parsed, (.result.answers | map(.[:120]))]' "$f" | head -c 600)"
fi
if cell llm-turns "$here/sampler/conversation" "${qwen[@]}"; then
  echo "llm-turns: $(jq -c '.result | {answer, recalled, error}' "$g")"
  J '.result.error == ""' "$g" || check "llm-turns: not run: $(jq -r .result.error "$g")"
  J '.result.recalled' "$g" || check "llm-turns: the answer doesn't name what the earlier turns said: $(jq -r .result.answer "$g")"
fi
}
