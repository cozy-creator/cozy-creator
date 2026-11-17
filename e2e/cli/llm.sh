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
step "web-llm-stream: an LLM's words grow on its box while it writes them (xs-209)"
local pw=$OUT/playwright-test web=$OUT/web chrome code=0 line
chrome=$(web_chrome)
[ -n "$chrome" ] && [ -x "$chrome" ] || { check "llm-stream: no Google Chrome on this runner (${chrome:-none})"; return; }
npm install -s --prefix "$(web_path "$pw")" @playwright/test@1.55.0 > "$OUT/web-npm.log" 2>&1 \
  || { check "llm-stream: npm install @playwright/test failed: $(tail -c 300 "$OUT/web-npm.log")"; return; }
mkdir -p "$web" && rm -f "$web/report.json"
HOLDER=$(web_path "$HOME/.cozy/calcifer/calcifer.lock.holder") COZY=$(web_path "$COZY") CHROME=$chrome OUT=$(web_path "$OUT") ARGV_LOG=$(web_path "$OUT/argv.log") \
  NODE_PATH=$(web_path "$pw/node_modules") node "$(web_path "$pw/node_modules/@playwright/test/cli.js")" test -c "$(web_path "$here/web/llm-stream.config.js")" \
  > "$web/llm-stream.log" 2>&1 || code=$?
node "$(web_path "$here/web/report.js")" "$(web_path "$web/report.json")" llm-stream > "$web/llm-stream.said" 2>&1 \
  || { check "llm-stream: no report (exit $code): $(tail -c 600 "$web/llm-stream.log")"; return; }
while IFS= read -r line; do
  case $line in FAILED\ *) check "${line#FAILED }" ;; *) echo "$line" ;; esac
done < "$web/llm-stream.said"
[ "$code" = 0 ] || grep -q '^FAILED' "$web/llm-stream.said" || check "llm-stream: Playwright exited $code: $(tail -c 600 "$web/llm-stream.log")"
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
