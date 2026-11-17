# xs-204 (Paul, 11-16): "ideally I want LLM tools as well. Like I can create a text box, and then type text into it. Then I can
# attach an LLM and it can generate more text. Or I can attach an LLM (ideally VLLM) to an image node, or a video node, and ask it
# to produce a caption, and it'll produce text".

# Qwen3.5's `write` tool, the real 0.8B on this computer's CPU through `cozy run`: words from words, then one answer about a
# picture and a clip its instructions mention. Each answer is words.
llm_write() {
[ -n "$LOCAL" ] || return 0
[ -n "$MODEL" ] || { echo "Qwen3.5's write: not selected (ci/affected's model)"; return 0; }
step "an LLM tool writes words, and of a picture and a clip it's shown (xs-204)"
local tool=qwen3.5/write dir=$OUT/llm pic clip prompt
[ -z "$PACKAGES" ] || tool=$(cd "$PACKAGES" && printf '%s/qwen3.5/write\n' "$PWD")
local qwen=(--model model=qwen/qwen3.5-0.8b@0.1.0 max_tokens:=32 seed:=7)
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
web_suite llm-stream llm-stream
}

# xs-209 (Paul, 11-17): "Make sure our machines are speaking the standard Responses-API (ChatGPT / openai standardized). That should make it
# easy to consume the results as a delta, including by other tools." OpenAI's own Python SDK, `client.responses.create(stream=True)`,
# against this computer's Calcifer with the CLI's bearer as its API key: Qwen3.5 0.8B on the CPU answers through the qwen3 package's Write
# tool, its words as deltas well before it completes, adding up to its text; not streamed, a picture sent as a data: URL is answered whole.
responses_sdk() {
[ -n "$LOCAL" ] || return 0
step "OpenAI's SDK streams an answer from Calcifer's Responses API (xs-209)"
local holder=$HOME/.cozy/calcifer/calcifer.lock.holder package=qwen3.5 id said
[ -z "$PACKAGES" ] || [ ! -d "$PACKAGES/qwen3" ] || package=qwen3
id=$(status "$RUN_PROJECT" | jq -r '.id // empty')
[ -n "$id" ] || { check "responses: the project has no id"; return; }
if ! uv run -q --no-project --with 'openai>=1.80' python3 "$here/responses_sdk.py" --base "http://$(jq -r .address < "$holder")/v1" \
  --key "$(jq -r .token < "$holder")" --project "$id" --model "$package/write:qwen/qwen3.5-0.8b@0.1.0" > "$OUT/responses.txt" 2>&1; then
  said=$(grep '^FAIL ' "$OUT/responses.txt" | sed 's/^FAIL //' | paste -sd ';' -)
  check "responses: ${said:-$(tail -c 600 "$OUT/responses.txt")}"
fi
tail -n 1 "$OUT/responses.txt" | cut -c1-2000
}
