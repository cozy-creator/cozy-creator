# h3-joins (cl-774): two H3 behaviours of the release that no run had shown, through `cozy project render` on a real project folder, on
# one rental (a 4090): the pictures imported and typed by the routes the page's boxes call, the shots written in project.yaml as `add`
# writes them (`add` reads local code here with the Runtime its uv.lock names, and minimax-h3's 0.39.6 refuses its `node=`; a render
# reads it with the pod's). minimax-h3 is local code from a packages_refs checkout `pk` at the release's commit; the session's cozy
# carries its TensorD to the pod, `runtime` the Runtime.
#   1. joins: two 8 s beats, past one segment's 15 s, so the shot crosses a join, with two characters drawn here (fixtures/h3-joins)
#      mentioned in both, `pip` typed a character and `tova` untyped. Its frames are the beats' on H3's grid, as the package planned
#      them; nothing jumps at the join (each frame against the next, the join among the 24 either side, the frames kept to look at);
#      each segment's prompt names both as characters with their retention lines; the carry was read, not the fallback (no `New take
#      from here isn't available`, and both Ref2VA segments succeed: the second refuses without its carry).
#   2. framed: a written first_frame, the 1:1 ref/seed.jpg, with both characters mentioned and aspect_ratio 16:9: the video is
#      1344x768, the centre-crop warning is given, frame 0 is the picture's middle (SSIM against it and against it squashed), the
#      characters go as references and not keyframes (`.request.input`), and Ref2VA ran, as it must: FL2VA takes no references.
# Whatever fails, every case is read and the pod ended by its id, until the Hub's record of it has ended, before the verdict. params:
#   runtime     the Runtime wheel, https://…#sha256=<hex> (default: the session's RUNTIME_WHEEL, from `runtime_ref`)
#   traffic_gb  GB the provider bills (default 80)
#   need_s      seconds the guard must buy (default 3600)
. "$(dirname "$0")/lib.sh"
FIX=$(cd "$(dirname "$0")/fixtures/h3-joins" && pwd) || fail "no fixtures"
. "$FIX/measure.sh"
URL=$(param runtime) PK=$PACKAGES/pk
H3=$PK/minimax-h3 TOOL=$PK/minimax-h3/generate
J() { jq -e "$@" > /dev/null 2>&1; }
[ -f "$H3/h3.py" ] || fail "no packages_refs checkout pk/minimax-h3"
grep -qF '_KINDS = {"": "character"' "$H3/h3.py" && grep -qF 'its middle is used for 16:9' "$H3/h3.py" ||
  fail "pk/minimax-h3 lacks the release's fixes (packages a8fc2c7f): an untyped picture a character, a shape asked for over a keyframe's"
mkdir -p "$OUT/wheel" "$OUT/frames" "$OUT/art"
if [ -n "$URL" ]; then
  [[ $URL == https://*'#sha256='* ]] || fail "runtime is not https://…#sha256=<hex>"
  W=$OUT/wheel/$(basename "${URL%%#*}")
  curl -fsSL --retry 5 --retry-all-errors "${URL%%#*}" -o "$W" || fail "cannot fetch the runtime wheel"
  [ "$(sha256sum "$W" | cut -c1-64)" = "${URL##*sha256=}" ] || fail "runtime wheel sha256"
else
  W=${RUNTIME_WHEEL:-}
  [ -n "$W" ] || fail "no Runtime: name \`runtime\`, or dispatch with runtime_ref"
fi
RT=$(basename "$W" | cut -d- -f2)
say "pk $(git -C "$PK" rev-parse HEAD), Runtime $RT"
cozy --help > /dev/null || fail "cozy --help"
command -v ffprobe > /dev/null || sudo apt-get install -y -qq ffmpeg > /dev/null 2>&1 || fail "no ffmpeg"
command -v uv > /dev/null || fail "no uv to draw the characters with"
uv run -q --no-project --with pillow==11.3.0 python -I "$FIX/draw.py" "$OUT/art" > "$OUT/draw.txt" 2>&1 || fail "draw: $(tail -3 "$OUT/draw.txt")"
cp "$REF/seed.jpg" "$OUT/art/lighthouse.jpg"

# The measures, on made-up videos: a smooth one and one cut at its join; and the first frame's middle through an encode.
T=$OUT/selftest; mkdir -p "$T/smooth" "$T/cut"
ffmpeg -v error -y -f lavfi -i testsrc2=size=1344x768:rate=24 -frames:v 396 -c:v libx264 -preset ultrafast -pix_fmt yuv420p "$T/smooth.mp4" &&
  ffmpeg -v error -y -f lavfi -i testsrc2=size=1344x768:rate=24:duration=8 -f lavfi -i smptebars=size=1344x768:rate=24:duration=8.5 \
    -filter_complex '[0:v][1:v]concat=n=2:v=1[v]' -map '[v]' -c:v libx264 -preset ultrafast -pix_fmt yuv420p "$T/cut.mp4" &&
  pairs "$T/smooth.mp4" "$T/smooth" && pairs "$T/cut.mp4" "$T/cut" && fitted "$OUT/art/lighthouse.jpg" "$T/fit.png" &&
  squashed "$OUT/art/lighthouse.jpg" "$T/squash.png" || fail "the measures don't run"
say "measures: $(frames "$T/smooth.mp4") frames, $(size "$T/smooth.mp4"); smooth join $(join_step "$T/smooth" 192); cut join $(join_step "$T/cut" 192); the middle against itself $(likeness "$T/fit.png" "$T/fit.png"), against it squashed $(likeness "$T/fit.png" "$T/squash.png"); 8+8 s is $(grid 8 8 | paste -sd+) frames"

# The project, as the page makes it: the pictures imported, pip typed, the two shots as boxes of the tool.
P=$OUT/film
cozy project new "$P" > "$OUT/new.txt" 2>&1 || fail "project new: $(tail -3 "$OUT/new.txt")"
printf 'version: 3\nassets: {}\n' > "$P/project.yaml"
for art in pip.png tova.png lighthouse.jpg; do
  cozy project -p "$P" import "$OUT/art/$art" "${art%.*}" >> "$OUT/import.txt" 2>&1 || fail "import $art: $(tail -3 "$OUT/import.txt")"
done
cozy project -p "$P" set pip asset_type=character > "$OUT/set.txt" 2>&1 || fail "set pip: $(tail -3 "$OUT/set.txt")"
cat >> "$P/project.yaml" <<YAML || fail "project.yaml"
  joins:
    tool: $TOOL
    aspect_ratio: "16:9"
    soundscape: Birdsong and light wind.
    beats:
      - duration: 8
        description: "<pip> and <tova> walk side by side along a sunny park path as the camera tracks them."
      - duration: 8
        description: "<pip> stops to point at a red kite, and <tova> looks up and flaps its wings."
  framed:
    tool: $TOOL
    aspect_ratio: "16:9"
    first_frame: <lighthouse>
    soundscape: Wind and crashing waves.
    beats:
      - duration: 5
        description: "<pip> and <tova> walk into view on the snowy cliff below the lighthouse."
YAML
cozy --json project -p "$P" status > "$OUT/status.json" 2>&1 || fail "status: $(tail -c 300 "$OUT/status.json")"
J '[.assets[].name] | index("joins") and index("framed")' "$OUT/status.json" || fail "the shots aren't assets: $(jq -c '[.assets[]? | .name]' "$OUT/status.json")"
grep -qE '^ +asset_type: character' "$P/project.yaml" || fail "pip isn't typed: $(cat "$P/project.yaml")"
cp "$P/project.yaml" "$OUT/project-before.yaml"
say "project: $(jq -c '[.assets[] | {name, kind, tool}]' "$OUT/status.json"), warnings $(jq -c '.warnings // []' "$OUT/status.json")"
# An import keeps the file as it is: its hash is the picture's, and the project holds it.
sha_of() { local h; h=$(sha256sum "$OUT/art/$1".* | cut -c1-64); ls "$P/files/$h".* > /dev/null 2>&1 && echo "$h"; }
SHA_PIP=$(sha_of pip) SHA_TOVA=$(sha_of tova) SHA_LIGHT=$(sha_of lighthouse)
[ -n "$SHA_PIP" ] && [ -n "$SHA_TOVA" ] && [ -n "$SHA_LIGHT" ] || fail "an imported picture isn't in the project's files: $(ls "$P/files")"

# The pre-flight's own: the Runtime the pod gets, and minimax-h3's environment, here first (what fails here is said, for a person to judge).
if [ "$RENT" != true ]; then
  step local-update machine update local --runtime "$W" -y || say "pre-flight: machine update local --runtime failed"
  step local-install package install "$H3" --on local || say "pre-flight: package install on local failed"
fi

quote "$(param traffic_gb 80)" "$(param need_s 3600)"

# end_pod: each rental of this session ended by its id, followed until the Hub's record of it has ended, before any verdict.
hub_row() { timeout 60 cozy machine ls --since 24h --json < /dev/null 2> /dev/null | jq -c --arg r "$1" '.rentals[] | select(.rental_id == $r)'; }
ENDED=''
end_pod() {
  local r row
  ENDED=1
  for r in "${RIDS[@]}"; do
    say "END $r"
    cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1 || cozy machine end "$r" -y --force >> "$OUT/end.txt" 2>&1
    for _ in $(seq 1 30); do
      row=$(hub_row "$r")
      J '.ended_at_ms != null' <<<"$row" && gone "$r" && break
      sleep 10
    done
    say "the Hub's record of $r: $(jq -c '{state, ended_at_ms, ran_s, spend_usd_micros, spend_basis, provider_cost_usd_micros, provider_cost_basis}' <<<"$row" 2> /dev/null)"
  done
}
trap 'code=$?; [ -n "$ENDED" ] || end_pod; (exit "$code"); finish' EXIT

rent
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$W" -y || fail "update"
read -r v rt < <(cozy machine show "$NAME" --json | jq -r '"\(.status.version) \(.status.runtime)"')
[ "$v" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not the session's"
[ "$rt" = "$RT" ] || [ "$rt" = "${RT%%+*}" ] || fail "the pod runs Runtime $rt, not $RT"
say "pod TensorD $v, Runtime $rt"
step install package install "$H3" --on "$NAME" || fail "install minimax-h3: $(tail -c 400 "$OUT/install.txt")"

# shoot <name>: rendered on the pod, then its run's records read while the pod is up.
bad1=() bad2=()
shoot() {
  local n
  step "render-$1" project -p "$P" render "$1" --on "$NAME" || say "$1: the render failed: $(tail -c 400 "$OUT/render-$1.txt" | tr '\n' ' ')"
  cozy --json project -p "$P" status "$1" > "$OUT/$1-status.json" 2>&1
  n=$(jq -r '.versions[-1].runs[-1] // empty' "$OUT/$1-status.json" 2> /dev/null)
  [ -n "$n" ] || { say "$1 has no run: $(head -c 300 "$OUT/$1-status.json")"; return 0; }
  cozy --json --full run show "$n" --once > "$OUT/$1-run.json" 2>&1
  cozy --full run show "$n" --once > "$OUT/$1-run.txt" 2>&1
  say "$1: run $n $(jq -r .status "$OUT/$1-run.json"), execution $(jq -r '(.execution_ms // 0) / 1000 | floor' "$OUT/$1-run.json") s, calls $(calls "$1")"
}
calls() { jq -r '[.calls[]? | "\(.function) \(.status)"] | join(", ")' "$OUT/$1-run.json" 2> /dev/null; }
video() { jq -r --arg P "$P" '.versions[-1].files.video.path // empty | $P + "/" + .' "$OUT/$1-status.json" 2> /dev/null; }
# What a shot said: its run's record and log, its version's warnings and what its render printed.
said() { { jq -r '.. | strings' "$OUT/$1-run.json" "$OUT/$1-status.json" 2> /dev/null; cat "$OUT/$1-run.txt" "$OUT/render-$1.txt" 2> /dev/null; }; }
# cast <name> <bad array> <segments>: each segment's prompt (its log's head and tail, 400 characters each) names pip and tova as
# characters, with a retention line each, and sends both.
cast() {
  local shot=$1 want=$3 line k chars refs name seen=0
  local -n into=$2
  while IFS= read -r line; do
    seen=$((seen + 1))
    k=$(grep -oE ' segment=[0-9]+' <<<"$line" | cut -d= -f2) chars=$(grep -oE ' chars=[0-9]+' <<<"$line" | cut -d= -f2)
    refs=$(grep -oE 'references=("[^"]*"|[^ ]*)' <<<"$line" | cut -d= -f2-)
    say "$shot segment $k: $chars characters ($([ "${chars:-0}" -le 800 ] && echo 'all of it seen' || echo 'its middle unseen')), references $refs"
    for name in pip tova; do
      grep -qE "(^|[\" ,])$name([\" ,]|$)" <<<"$refs" || into+=("$shot segment $k doesn't send <$name>")
      grep -qF "<$name> is the character shown in" <<<"$line" || into+=("$shot segment $k doesn't define <$name> as a character")
      grep -qF "<$name>: keep the look shown in the reference." <<<"$line" || into+=("$shot segment $k has no retention line for <$name>")
    done
    grep -qF 'retention_analysis:' <<<"$line" || into+=("$shot segment $k has no retention_analysis")
    grep -oE 'head="[^"]{0,120}' <<<"$line" | sed 's/^/    /' >> "$R"
  done < <(grep -F 'h3 segment prompt' "$OUT/$shot-run.txt" 2> /dev/null)
  [ "$seen" = "$want" ] || into+=("$shot's log has $seen segment prompts, not $want")
}

shoot joins
shoot framed
end_pod

say "1. joins: two 8 s beats, one join, pip and tova in both"
IN1=$(jq -c '.request.input // {}' "$OUT/joins-run.json" 2> /dev/null)
say "joins' input: $(jq -c '{references: [.references[]? | {name, asset_type, image}], first_frame, last_frame, beats: [.beats[]? | .duration], aspect_ratio}' <<<"$IN1")"
J --arg p "sha256:$SHA_PIP" --arg t "sha256:$SHA_TOVA" '[.references[]? | [.name, .image]] == [["pip", $p], ["tova", $t]] and (has("first_frame") | not) and (has("last_frame") | not)' <<<"$IN1" ||
  bad1+=("joins' input isn't pip and tova as references with no keyframe")
J '[.references[]? | .asset_type // ""] == ["character", ""]' <<<"$IN1" || bad1+=("joins' references aren't pip typed and tova untyped")
# A log line's fields are in key order: the plan is the line with boundaries_s, the end the one with turbo.
plan=$(grep -F 'h3 shot segments ' "$OUT/joins-run.txt" 2> /dev/null | grep -F ' boundaries_s=' | head -1)
segments=$(grep -oE ' segments=[0-9]+' <<<"$plan" | cut -d= -f2) seconds=$(grep -oE ' seconds=[0-9,]+' <<<"$plan" | cut -d= -f2)
planned=$(grep -oE ' delivered_frames=[0-9]+' <<<"$plan" | cut -d= -f2) boundary=$(grep -oE ' boundaries_s=[0-9.,]+' <<<"$plan" | cut -d= -f2)
say "the package's plan: ${plan:-none}"
say "the run's end: $(grep -F 'h3 shot ' "$OUT/joins-run.txt" 2> /dev/null | grep -F ' turbo=' | tail -1 | cut -c1-300)"
[ "${segments:-0}" = 2 ] || bad1+=("the shot is ${segments:-no} segments, not 2: it crosses no join")
VID1=$(video joins)
if [ -f "$VID1" ]; then
  n1=$(frames "$VID1") s1=$(size "$VID1") grid1=$(( $(grid ${seconds//,/ } | paste -sd+) ))
  say "joins: $s1, $n1 frames ($(awk -v n="$n1" 'BEGIN { printf "%.3f", n / 24 }') s); the beats ask 16 s (384 frames), the package planned ${planned:-?} ($seconds s), the grid gives $grid1"
  [ "$n1" = "${planned:-x}" ] && [ "$n1" = "$grid1" ] || bad1+=("joins is $n1 frames, the package planned ${planned:-?}, the grid $grid1")
  [ "$n1" -ge 384 ] && [ "$n1" -lt 408 ] || bad1+=("joins is $n1 frames for 16 s of beats")
  [ "$s1" = 1344x768 ] || bad1+=("joins is $s1")
  has_sound "$VID1" || bad1+=("joins has no sound")
  JOIN=$(awk -v b="${boundary%%,*}" 'BEGIN { printf "%d", b * 24 + 0.5 }')
  mkdir -p "$OUT/joins-pairs"
  if [ "$JOIN" -gt 24 ] && pairs "$VID1" "$OUT/joins-pairs"; then
    read -r _ jp _ mp _ lp _ js _ ms _ lw _ jl _ ml _ hl <<<"$(join_step "$OUT/joins-pairs" "$JOIN")"
    say "the join, frame $((JOIN - 1)) to $JOIN, against the 24 steps either side: $(join_step "$OUT/joins-pairs" "$JOIN")"
    awk -v j="$jp" -v m="$mp" -v l="$lp" 'BEGIN { exit !(j >= m - 3 || j >= l) }' || bad1+=("the join's PSNR $jp dB is under its neighbours' (median $mp, low $lp)")
    awk -v j="$js" -v m="$ms" -v l="$lw" 'BEGIN { exit !(j >= m - 0.05 || j >= l) }' || bad1+=("the join's SSIM $js is under its neighbours' (median $ms, low $lw)")
    awk -v j="$jl" -v h="$hl" 'BEGIN { exit !(j <= h || j <= 2) }' || bad1+=("the join's luma step $jl is over its neighbours' (high $hl)")
    stills "$VID1" "$OUT/frames" joins 0 $((JOIN - 2)) $((JOIN - 1)) "$JOIN" $((JOIN + 1)) $((n1 - 1))
    around "$VID1" "$JOIN" "$OUT/frames/joins-around-$JOIN.png"
  else
    bad1+=("no join to measure (boundary ${boundary:-none})")
  fi
else
  bad1+=("joins made no video")
fi
cast joins bad1 2
# The carry: the second segment reads the first's (it refuses without one), and a carry that can't be saved says so (the fallback).
fallback=$(said joins | grep -F "New take from here isn't available" | sort -u | head -3)
mixed=$(said joins | grep -F 'the join may show' | sort -u | head -3)
J '[.calls[]? | select(.function | test("ref2va_segment")) | .status] == ["succeeded", "succeeded"]' "$OUT/joins-run.json" ||
  bad1+=("joins' calls: $(calls joins)")
[ -z "$fallback" ] || bad1+=("the fallback: $fallback")
[ -z "$mixed" ] || bad1+=("the join may show: $mixed")
J '.versions[-1].files.latents.sha256' "$OUT/joins-status.json" || bad1+=("joins kept no latents")
say "carry: $([ -z "$fallback" ] && J '[.calls[]? | select(.function | test("ref2va_segment")) | .status] == ["succeeded", "succeeded"]' "$OUT/joins-run.json" &&
  echo "read: segment 2 succeeded on segment 1's carry, and nothing says New take from here isn't available" || echo "not shown: ${fallback:-$(calls joins)}")"

say "2. framed: a 1:1 first_frame, pip and tova, aspect_ratio 16:9"
IN2=$(jq -c '.request.input // {}' "$OUT/framed-run.json" 2> /dev/null)
say "framed's input: $(jq -c '{references: [.references[]? | {name, asset_type, image}], first_frame, last_frame, aspect_ratio, beats: [.beats[]? | .duration]}' <<<"$IN2")"
J --arg p "sha256:$SHA_PIP" --arg t "sha256:$SHA_TOVA" --arg l "sha256:$SHA_LIGHT" \
  '[.references[]? | [.name, .image]] == [["pip", $p], ["tova", $t]] and .first_frame == $l and (has("last_frame") | not) and .aspect_ratio == "16:9"' <<<"$IN2" ||
  bad2+=("framed's input isn't pip and tova as references, lighthouse as first_frame, no last_frame, 16:9")
crop=$(said framed | grep -oF 'first_frame is 1:1; its middle is used for 16:9' | head -1)
say "the crop's warning: ${crop:-none}; framed's warnings: $(jq -c '.versions[-1].warnings // []' "$OUT/framed-status.json" 2> /dev/null)"
[ -n "$crop" ] || bad2+=("no centre-crop warning")
say "framed's calls: $(calls framed)"
J '[.calls[]? | .function | select(test("_segment"))] | length == 1 and (.[0] | test("ref2va_segment"))' "$OUT/framed-run.json" ||
  bad2+=("framed didn't run on Ref2VA alone: $(calls framed)")
grep -F 'h3 segment prompt' "$OUT/framed-run.txt" 2> /dev/null | grep -qF 'keyframe completion + reference generation' ||
  bad2+=("framed's prompt isn't keyframe completion with its references")
VID2=$(video framed)
if [ -f "$VID2" ]; then
  n2=$(frames "$VID2") s2=$(size "$VID2")
  say "framed: $s2, $n2 frames; a 5 s beat is $(grid 5)"
  [ "$s2" = 1344x768 ] || bad2+=("framed is $s2, not 16:9")
  [ "$n2" = "$(grid 5)" ] || bad2+=("framed is $n2 frames")
  stills "$VID2" "$OUT/frames" framed 0 1 $((n2 / 2)) $((n2 - 1))
  fitted "$OUT/art/lighthouse.jpg" "$OUT/frames/lighthouse-middle.png"
  squashed "$OUT/art/lighthouse.jpg" "$OUT/frames/lighthouse-squashed.png"
  side "$OUT/frames/framed-first.png" "$OUT/frames/framed-0.png" "$OUT/frames/lighthouse-middle.png" "$OUT/frames/lighthouse-squashed.png"
  read -r _ fs _ fp <<<"$(likeness "$OUT/frames/framed-0.png" "$OUT/frames/lighthouse-middle.png")"
  read -r _ qs _ qp <<<"$(likeness "$OUT/frames/framed-0.png" "$OUT/frames/lighthouse-squashed.png")"
  say "frame 0 against the picture's middle: SSIM $fs, PSNR $fp dB; against it squashed: SSIM $qs, PSNR $qp dB"
  awk -v f="$fs" -v q="$qs" 'BEGIN { exit !(f + 0 >= 0.5 && f + 0 > q + 0) }' || bad2+=("frame 0 isn't the first frame's middle (SSIM $fs, squashed $qs)")
else
  bad2+=("framed made no video")
fi
cast framed bad2 1

cp "$P/project.yaml" "$P/project.lock" "$OUT/" 2> /dev/null
[ ${#bad1[@]} = 0 ] && say "PASS 1 joins" || say "FAIL 1 joins: $(printf '%s; ' "${bad1[@]}")"
[ ${#bad2[@]} = 0 ] && say "PASS 2 framed" || say "FAIL 2 framed: $(printf '%s; ' "${bad2[@]}")"
[ ${#bad1[@]} = 0 ] && [ ${#bad2[@]} = 0 ] || fail "see FAIL above; judge $OUT/frames"
say "PASS: both; judge $OUT/frames"
