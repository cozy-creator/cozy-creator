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
