# h3-parity (se-156, proof branch only): each case run our way and upstream's on one rental, for the parity audit of
# minimax-h3, minimax-h3-image and p-h3-image (Paul, 11-17: "make sure we don't have any other errors / deviations like
# that in our implementations for other packages as well"). Ours: one `cozy run` of a packages_refs checkout's tool (a dev
# home whose first-party packages are the checkout, orbit-arcs.sh's way), on the rental held on the RC's builds. Upstream:
# Diffusers 0.43's modular pipelines, bf16, SDPA, in their own environment over `cozy machine ssh` (comparators-over-ssh),
# with the prompt and seed our run used (its record's `h3 encoder prompt`, or the image result's `prompt`). The weights
# both sides need download meanwhile (fixtures/h3-parity/pod_setup.sh). Phases in order: video, then image; a phase's
# upstream weights are deleted once its upstream cases ran. Once the rental ends, compare.py scores each pair on this
# runner. Without `rent`, $0: inputs derived, the scripts compiled, the measurer checked, the quote. params (a JSON file in
# this checkout: `file`):
#   tree     the packages_refs checkout (required)
#   ours     [{name, phase, tool, input (a fixture JSON), args?: [cozy run terms]}]
#   upstream [{name, phase, trunk|model, of (an `ours` name: its prompt, seed and size), short_edge?, frames?}]
#   pairs    [[upstream-or-ours name, against-name, what], ...] for compare.py
#   fix      a second packages_refs checkout, a fix's (optional); `fix_cases`: ours names rerun from it as <name>-fix
#            once the video phase's own ran, and the image phase then runs from it too (its packages unchanged)
#   seconds  what the cap must buy (default 10800)
. "$(dirname "$0")/lib.sh"
HERE=$(cd "$(dirname "$0")" && pwd)
FIX=$HERE/fixtures/h3-parity WORK=${RUNNER_TEMP:-$OUT/..}/h3-parity
TREE=$(param tree '') FIXT=$(param fix '') FIXC=$(param fix_cases '[]') OURS=$(param ours '[]') UPS=$(param upstream '[]') PAIRS=$(param pairs '[]') NEED=$(param seconds 10800)
for tool in jq uv ffmpeg ffprobe python3; do command -v "$tool" > /dev/null || fail "no $tool"; done
[ -n "$TREE" ] && [ -f "$PACKAGES/$TREE/minimax-h3/pyproject.toml" ] || fail "tree: a packages_refs checkout with minimax-h3"
SOURCE=$(cd "$PACKAGES/$TREE" && pwd)
jq -e 'type == "array" and length > 0 and all(.[]; (.name | test("^[a-z0-9-]+$")) and (.phase == "video" or .phase == "image"))' \
  <<<"$OURS" > /dev/null || fail "ours: a list of {name, phase, tool, input}"
V=$(cozy --version | awk '{print $2}')
[[ $V == *-dev* ]] || fail "cozy $V is no RC build: dispatch with cozy=<a dev-build or rc-dev run of rc/<x>>"
say "packages from $(git -C "$SOURCE" rev-parse --short HEAD) ($TREE); cozy $V"
for p in $(jq -r '.[].tool | split("/")[0]' <<<"$OURS" | sort -u); do kernels "$SOURCE/$p"; done
FIXSRC=''
if [ -n "$FIXT" ]; then
  [ -f "$PACKAGES/$FIXT/minimax-h3/pyproject.toml" ] || fail "fix: no packages_refs checkout $FIXT"
  FIXSRC=$(cd "$PACKAGES/$FIXT" && pwd)
  say "fix from $(git -C "$FIXSRC" rev-parse --short HEAD) ($FIXT): $(jq -c . <<<"$FIXC")"
  for p in $(jq -r '.[].tool | split("/")[0]' <<<"$OURS" | sort -u); do kernels "$FIXSRC/$p"; done
fi
mkdir -p "$WORK/in" "$OUT/ours" "$OUT/upstream" "$OUT/records" "$OUT/prompts"

# The measurer's Python, proven before anything is rented; the upstream scripts compile.
uv venv -q --python 3.12 "$WORK/py" > "$OUT/py.log" 2>&1 && uv pip install -q --python "$WORK/py/bin/python" numpy pillow safetensors \
  torch torchvision piq --index-url https://download.pytorch.org/whl/cpu --extra-index-url https://pypi.org/simple >> "$OUT/py.log" 2>&1 ||
  fail "the measurer's Python: $(tail -c 300 "$OUT/py.log")"
PY=$WORK/py/bin/python
for f in upstream_h3.py upstream_image.py compare.py; do python3 -m py_compile "$FIX/$f" || fail "$f does not compile"; done
"$PY" -I "$FIX/compare.py" selftest "$WORK/selftest" > "$OUT/selftest.txt" 2>&1 || fail "compare.py selftest: $(tail -c 400 "$OUT/selftest.txt")"
# The inputs, all public: the gates' lighthouse (release-gate/ref/seed.jpg), at the first frame's canvas; alibaba-pai's own
# Ref2VA test picture (MiniMax-H3-Acc-LoRAs asset/ref2va_test_5_1.jpg), as published.
cp "$FIX"/*.json "$WORK/in/" && cp "$REF/seed.jpg" "$WORK/in/lighthouse.jpg" || fail "inputs"
"$PY" -I -c 'import sys; from PIL import Image; Image.open(sys.argv[1]).convert("RGB").resize((768, 768), Image.Resampling.LANCZOS).save(sys.argv[2])' \
  "$WORK/in/lighthouse.jpg" "$WORK/in/lighthouse-768.png" || fail "lighthouse-768"
curl -fsSL --retry 5 -o "$WORK/in/fighter.jpg" https://huggingface.co/alibaba-pai/MiniMax-H3-Acc-LoRAs/resolve/main/asset/ref2va_test_5_1.jpg ||
  fail "the Ref2VA test picture"
cp "$WORK/in/"*.jpg "$WORK/in/"*.png "$OUT/" 2> /dev/null
for i in $(seq 0 $(($(jq length <<<"$OURS") - 1))); do
  f=$(jq -r ".[$i].input" <<<"$OURS"); [ -f "$WORK/in/$f" ] || fail "no input $f"
done

# A dev home, its first-party packages from the checkout (orbit-arcs.sh).
cfg=$HOME/.cozy/config.toml
mkdir -p "${cfg%/*}" && touch "$cfg" || fail "no $cfg"
{ echo 'dev = true'; grep -v '^dev *=' "$cfg" || true; } > "$cfg.new" && mv "$cfg.new" "$cfg" || fail "dev = true not written"
# home <checkout>: this runner's Calcifer takes its first-party packages from the checkout.
home() {
  awk -v source="$1" '
    function ours() { print "packages = \"" source "\""; put = 1 }
    /^packages *=/ { next } { print } /^\[calcifer\]/ { ours() } END { if (!put) { printf "\n[calcifer]\n"; ours() } }' \
    "$cfg" > "$cfg.next" && mv "$cfg.next" "$cfg" || fail "calcifer.packages not written"
  cozy restart > "$OUT/restart.txt" 2>&1 || fail "cozy restart: $(tail -c 300 "$OUT/restart.txt")"
}
home "$SOURCE"
[ -z "$(own)" ] || fail "this runner's Calcifer already holds a rental: $(own | paste -sd' ')"
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
trap 'code=$?; claim; (exit "$code"); finish' EXIT

quote "$NEED"
order
ready
for _ in $(seq 180); do
  cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
  [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] && [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && break
  sleep 5
done
[ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ] || fail "$NAME isn't on the RC's builds in 15 min: $(jq -c .rc "$OUT/show-rc.json")"
say "on the RC: $(jq -r '"cozy \(.status.version) · Runtime \(.status.runtime) · " + ([.status.gpus[] | "GPU \(.index) \(.name) \(.memory_bytes / 1073741824 | floor) GiB"] | join(", "))' "$OUT/show-rc.json")"
for try in 1 2 3; do
  step "hold-$try" machine update "$NAME" --runtime "$(jq -r '.status.runtime' "$OUT/show-rc.json")" -y
  for _ in $(seq 180); do
    cozy machine show "$NAME" --json > "$OUT/show-rc.json" 2> /dev/null
    [ "$(jq -r '.state // empty' "$OUT/show-rc.json")" = ready ] && { [ -n "$(jq -r '.held // empty' "$OUT/show-rc.json")" ] || [ "$(jq -r '.rc.state // empty' "$OUT/show-rc.json")" = current ]; } && break
    sleep 5
  done
  [ -z "$(jq -r '.held // empty' "$OUT/show-rc.json")" ] || break
done
[ -n "$(jq -r '.held // empty' "$OUT/show-rc.json")" ] || fail "$NAME isn't held on the RC's builds: $(tail -c 300 "$OUT/hold-$try.txt")"
say "held: $(jq -r '.held' "$OUT/show-rc.json")"

pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
# SSH work is no job: the rental's 15-minute idle clock is reset meanwhile (comparators-over-ssh).
( while sleep 240; do cozy machine keepalive "$NAME" > /dev/null 2>&1; done ) & BG=$!
tar -C "$FIX" -cz . | T=300 pod "mkdir -p /root/parity && tar -xz -C /root/parity" || fail "copy the fixtures"
tar -C "$WORK/in" -cz . | T=300 pod "mkdir -p /root/parity/in && tar -xz -C /root/parity/in" || fail "copy the inputs"
# The pod has no curl: uv goes from this runner, and Python asks whether Hugging Face and PyPI answer.
tar -C "$(dirname "$(command -v uv)")" -c uv | T=300 pod "mkdir -p /root/bin && tar -x -C /root/bin" || fail "copy uv"
pod "free -g; df -h /root; nvidia-smi --query-gpu=name,memory.total --format=csv,noheader; command -v python3; for u in https://huggingface.co https://pypi.org/simple/; do python3 -c 'import sys, urllib.request; urllib.request.urlopen(sys.argv[1], timeout=30)' \$u && echo \"reach \$u ok\"; done" \
  < /dev/null > "$OUT/pod.txt" 2>&1
say "pod: $(tr '\n' ' ' < "$OUT/pod.txt" | cut -c1-400)"
grep -q 'reach https://huggingface.co ok' "$OUT/pod.txt" || fail "$NAME cannot reach Hugging Face"
ram=$(awk '/^Mem:/{print $2; exit}' "$OUT/pod.txt")
[ "${ram:-0}" -ge 96 ] || fail "$NAME has ${ram:-?} GiB of RAM; the upstream side needs about 96 or more"
# The upstream environment and both phases' weights, downloading beside our runs.
# `a && b &` would background the whole list, its shell holding SSH's output open: only the launch is backgrounded.
pod "cd /root/parity || exit 1; setsid nohup bash pod_setup.sh > setup.out 2>&1 < /dev/null &" < /dev/null || fail "pod_setup not started"

# prompt <name>: the encoder prompt of our H3 run (its log's head and tail, whole while it is at most 800 characters, checked
# against its digest), else the image result's own `prompt`.
prompt() {
  "$PY" -I - "$OUT/records/$1.json" "$OUT/records/$1.log" <<'EOF'
import hashlib, json, re, sys
record = json.load(open(sys.argv[1]))
for line in open(sys.argv[2], errors="replace"):
    if "h3 encoder prompt" not in line:
        continue
    head = json.loads(re.search(r'head=("(?:[^"\\]|\\.)*")', line).group(1))
    tail = json.loads(re.search(r'tail=("(?:[^"\\]|\\.)*")', line).group(1))
    chars, digest = int(re.search(r"chars=(\d+)", line).group(1)), re.search(r"digest=(\w+)", line).group(1)
    text = head if chars <= 400 else head + tail[800 - chars:] if chars <= 800 else ""
    if hashlib.sha256(text.encode()).hexdigest() != digest:
        sys.exit(f"the encoder prompt ({chars} characters) is not whole in the log")
    print(text, end="")
    sys.exit(0)
text = (record.get("result") or {}).get("prompt")
if not text:
    sys.exit("no prompt in the record")
print(text, end="")
EOF
}

# ours <phase>: each of our cases of the phase, its outputs and record kept.
bad=0
ours() {
  local i r name tool input
  for i in $(seq 0 $(($(jq length <<<"$OURS") - 1))); do
    r=$(jq -c ".[$i]" <<<"$OURS"); [ "$(jq -r .phase <<<"$r")" = "$1" ] || continue
    name=$(jq -r .name <<<"$r") tool=$(jq -r .tool <<<"$r") input=$(jq -r .input <<<"$r")
    mapfile -t args < <(jq -r '(.args // [])[]' <<<"$r")
    mkdir -p "$OUT/ours/$name"
    step "$name" run "$tool" --input "$WORK/in/$input" "${args[@]}" --on "$NAME" -y -o "$OUT/ours/$name"
    local code=$? n; n=$(runno "$name")
    if [ "$code" != 0 ] && grep -q 'attention_kernel' "$OUT/$name.txt" && [[ " ${args[*]} " == *" --attention-kernel "* ]]; then
      # A pin a site refuses: `--lane bf16` alone still serves every site at 16 bits (Runtime `_attention_pin`).
      say "$name: the SDPA pin was refused ($(grep -m1 -o 'attention_kernel[^ ]*.\{0,160\}' "$OUT/$name.txt")); again at the lane's 16 bits"
      local kept=() skip=0 a
      for a in "${args[@]}"; do [ "$skip" = 1 ] && { skip=0; continue; }; [ "$a" = --attention-kernel ] && { skip=1; continue; }; kept+=("$a"); done
      step "$name" run "$tool" --input "$WORK/in/$input" "${kept[@]}" --on "$NAME" -y -o "$OUT/ours/$name"
      code=$? n=$(runno "$name")
    fi
    if [ -n "$n" ]; then
      (cd "$RUN_PROJECT" && cozy --json --full run watch "$n" --once) > "$OUT/records/$name.json" 2> /dev/null
      (cd "$RUN_PROJECT" && cozy --full run watch "$n" --once) > "$OUT/records/$name.log" 2> /dev/null
    fi
    if [ "$code" != 0 ] || [ -z "$(find "$OUT/ours/$name" -type f \( -name '*.mp4' -o -name '*.png' \) | head -1)" ]; then
      say "FAIL $name: run ${n:-none}, exit $code: $(tail -c 400 "$OUT/$name.txt" | tr '\n' ' ')"; bad=1; continue
    fi
    prompt "$name" > "$OUT/prompts/$name.txt" 2> "$OUT/prompts/$name.err" || { say "FAIL $name: $(cat "$OUT/prompts/$name.err")"; bad=1; }
    say "$name: run $n $(jq -r '"\(.status), exec \((.execution_ms // 0) / 1000) s, peak reserved \((.measurements.memory.peak_reserved_bytes // 0) / 1073741824 * 10 | floor / 10) GiB, warnings \(.result.warnings // [] | length)"' "$OUT/records/$name.json" 2> /dev/null); lanes $(grep -oE '(lane|lanes)[=:] ?[A-Za-z0-9_,.-]+' "$OUT/records/$name.log" | sort -u | head -5 | paste -sd' ')"
  done
}

# upstream <phase>: the phase's upstream cases, one detached process per trunk or model on the pod, once its weights are in.
upstream() {
  local phase=$1 mark t=0 groups g spec
  mark=$([ "$phase" = video ] && echo dl-video || echo dl-image)
  until pod "test -f /root/parity/$mark.fail -o -f /root/parity/env.fail -o \\( -f /root/parity/$mark.ok -a -f /root/parity/env.ok \\)" < /dev/null 2> /dev/null; do
    sleep 30; t=$((t + 30))
    [ $((t % 300)) = 0 ] && say "waiting for the $phase weights: $(pod "tail -1 /root/parity/download.log 2>/dev/null | tr '\r' '\n' | tail -1 | cut -c1-200; df -BG --output=avail /root | tail -1" < /dev/null 2> /dev/null | tr '\n' ' ')"
    [ "$t" -lt 7200 ] || { say "FAIL upstream $phase: no weights in 2 h"; bad=1; return; }
  done
  pod "test -f /root/parity/$mark.ok -a -f /root/parity/env.ok" < /dev/null || { say "FAIL upstream $phase: $(pod 'tail -5 /root/parity/setup.log' < /dev/null | tr '\n' ' ')"; bad=1; return; }
  groups=$(jq -c --arg p "$phase" '[.[] | select(.phase == $p)] | group_by(.trunk // .model)[]' <<<"$UPS")
  while read -r g; do
    [ -n "$g" ] || continue
    local key; key=$(jq -r '.[0].trunk // .[0].model' <<<"$g")
    spec=$(jq -c --arg k "$key" --arg out "/root/parity/out/$key" '{out: $out, cases: []} + (if .[0].trunk then {trunk: $k} else {model: $k} end)' <<<"$g")
    local c
    while read -r c; do
      local of; of=$(jq -r .of <<<"$c")
      [ -s "$OUT/prompts/$of.txt" ] || { say "SKIP $(jq -r .name <<<"$c"): no prompt from $of"; bad=1; continue; }
      local seed size; seed=$(jq -r '.seed' "$WORK/in/$(jq -r --arg o "$of" '.[] | select(.name == $o) | .input' <<<"$OURS")")
      size=$(jq -c '{width: (.result.width // null), height: (.result.height // null)}' "$OUT/records/$of.json" 2> /dev/null || echo '{}')
      spec=$(jq -c --argjson c "$c" --rawfile p "$OUT/prompts/$of.txt" --argjson seed "$seed" --argjson size "$size" \
        '.cases += [$c + {prompt: $p, seed: $seed} + (if $c.trunk or $c.size == false then {} else $size end)]' <<<"$spec")
    done < <(jq -c '.[]' <<<"$g")
    jq . <<<"$spec" > "$OUT/upstream/$key.json"
    pod "cat > /root/parity/$key.json" < "$OUT/upstream/$key.json" || { say "FAIL upstream $key: spec not sent"; bad=1; continue; }
    local script; script=$([ "$phase" = video ] && echo upstream_h3.py || echo upstream_image.py)
    say "upstream $key: $(jq -c '[.cases[] | {name, seed} + (if .short_edge then {short_edge} else {} end)]' <<<"$spec")"
    pod "cd /root/parity || exit 1; rm -f $key.done; setsid nohup bash -c 'HF_HUB_OFFLINE=1 /root/up/bin/python $script $key.json > $key.log 2>&1; echo \$? > $key.done' > /dev/null 2>&1 < /dev/null &" < /dev/null ||
      { say "FAIL upstream $key: not started"; bad=1; continue; }
    t=0
    until pod "test -f /root/parity/$key.done" < /dev/null 2> /dev/null; do
      sleep 30; t=$((t + 30))
      [ $((t % 300)) = 0 ] && say "upstream $key: $(pod "tail -2 /root/parity/$key.log | tr '\r' '\n' | tail -1 | cut -c1-200" < /dev/null 2> /dev/null)"
      [ "$t" -lt 5400 ] || { say "FAIL upstream $key: over 90 min"; bad=1; break; }
    done
    pod "cat /root/parity/$key.log" < /dev/null > "$OUT/upstream/$key.log" 2>&1
    T=1800 pod "cd /root/parity/out && tar -c $key" < /dev/null 2> /dev/null | tar -x -C "$OUT/upstream" 2> /dev/null
    say "upstream $key: exit $(pod "cat /root/parity/$key.done" < /dev/null 2> /dev/null); $(grep -E ': \{|FAILED' "$OUT/upstream/$key.log" | cut -c1-300 | tr '\n' ' ')"
  done <<<"$groups"
}

ours video
if [ -n "$FIXSRC" ] && [ "$(jq length <<<"$FIXC")" -gt 0 ]; then
  home "$FIXSRC"
  OURS=$(jq -c --argjson f "$FIXC" '[.[] | select(.name as $n | $f | index($n)) | .name += "-fix" | .phase = "fix"] + .' <<<"$OURS")
  ours fix
fi
upstream video
# MiniMax-H3's DiTs and VAEs make room; its text encoder stays for MiniMax-H3-Image.
pod 'cd /root/.cache/huggingface/hub/models--MiniMaxAI--MiniMax-H3/snapshots/* && for f in transformer*/*.safetensors vae/* audio_vae/*; do rm -f "$(readlink -f "$f")" "$f"; done; df -BG --output=avail /root | tail -1' \
  < /dev/null >> "$OUT/pod.txt" 2>&1
ours image
upstream image
pod "cat /root/parity/setup.log /root/parity/setup.out" < /dev/null > "$OUT/setup.log" 2>&1
cozy machine show "$NAME" --json > "$OUT/show-end.json" 2>&1
cozy machine logs "$NAME" --since 6h -n 0 > "$OUT/machine.log" 2>&1
kill "$BG" 2> /dev/null; BG=''
for r in "${RIDS[@]}"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1; done
for r in "${RIDS[@]}"; do for _ in $(seq 60); do gone "$r" && break; sleep 10; done; say "$r: $(gone "$r" && echo gone || echo 'still listed')"; done

# Each pair scored here: PSNR, DISTS, latents, sound; crops of each at 100%.
"$PY" -I "$FIX/compare.py" pairs "$OUT" "$SOURCE" "$PAIRS" > "$OUT/compare.txt" 2> "$OUT/compare.err" || { say "compare: $(tail -c 400 "$OUT/compare.err")"; bad=1; }
cat "$OUT/compare.txt" >> "$R"
{ echo "## h3-parity on $NAME ($SKU)"; echo '```'; grep -E '^[0-9:]+ (FAIL|SKIP|upstream|[a-z0-9-]+: run)' "$R"; cat "$OUT/compare.txt"; echo '```'; } >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
[ "$bad" = 0 ] || RC=1
exit "$RC"
