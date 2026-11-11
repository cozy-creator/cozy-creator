# Suite `cli` (gate.sh --suite cli): the `run` and `model quantize` flags only a GPU pod means (e2e/coverage.toml's
# `gate` tier), each checked in its record by ./check.py. A failure this file sees itself goes to cli-fails.txt, which
# check.py reports. Sourced in gate.sh's shell: `cell`, `pkg`, `log`, $CLI, $M, $R and the scenario's $SCENE $CHAR
# $OBJECT. Every cozy call is logged (release-gate.yml runs gate.sh with --cli e2e/bin/cozy) for e2e/coverage.py.
#   --gpus 1 · --lane bf16 · --attention-kernel sdpa · --lora and --input's lora: (an SDXL LoRA by its Hugging Face
#   ref, the function's model and the UNet found from it)
#   machine update --gpu-memory-limit 12GiB, then 6GiB (one SDXL holds less than its uncapped 9 GiB), then off ·
#   model quantize --lane int8 / fp8, --package, --on · model upload --lane of a Hub model, and a family lane that is
#   not the weights' width
cli_fail() { log "cli: $*"; echo "$*" >> cli-fails.txt; }

cell cli-gpus "$(pkg sdxl generate)" prompt="$OBJECT on a marble plinth, photo" seed:=$((R + 201)) --gpus 1
cell cli-lane16 "$(pkg sdxl generate)" prompt="$SCENE in winter, photo" seed:=$((R + 202)) --lane bf16
cell cli-attn "$(pkg sdxl generate)" prompt="$CHAR reading a map, film still" seed:=$((R + 203)) --attention-kernel sdpa
cell cli-lora "$(pkg sdxl generate)" prompt="$OBJECT, studio light" seed:=$((R + 204)) steps:=8 \
  --lora hf://latent-consistency/lcm-lora-sdxl
printf 'prompt: "%s, watercolor"\nseed: %s\nsteps: 8\nlora:\n  - {ref: hf://latent-consistency/lcm-lora-sdxl, strength: 0.8}\n' \
  "$CHAR" $((R + 206)) > cli-lora.yaml
printf 'cli-lora-input\t%s, watercolor\n' "$CHAR" >> prompts.tsv  # quality.py, as cell writes it from argv
cell cli-lora-input "$(pkg sdxl generate)" --input "$PWD/cli-lora.yaml"

"$CLI" machine update "$M" --gpu-memory-limit 12GiB -y > cli-cap.log 2>&1 || cli_fail "machine update --gpu-memory-limit 12GiB: $(tail -c 300 cli-cap.log)"
cell cli-cap "$(pkg sdxl generate)" prompt="$SCENE at noon, photo" seed:=$((R + 205))
"$CLI" machine update "$M" --gpu-memory-limit 6GiB -y > cli-cap6.log 2>&1 || cli_fail "machine update --gpu-memory-limit 6GiB: $(tail -c 300 cli-cap6.log)"
cell cli-cap6 "$(pkg sdxl generate)" prompt="$CHAR at dusk, photo" seed:=$((R + 207))
"$CLI" machine update "$M" --gpu-memory-limit off -y > cli-cap-off.log 2>&1 || cli_fail "machine update --gpu-memory-limit off: $(tail -c 300 cli-cap-off.log)"

# A model's int8 and fp8 lanes by its package's recipes, from a throwaway repository's copy of anima's original
# weights, deleted at once. The copy names anima's bf16 commit: the Hub records it, and nothing moves.
q=fidika/ci-e2e-q-$R
bf16=$(model_details circlestone-labs/anima 2>/dev/null | python3 -c 'import json,sys; print(next(l["commit"] for l in json.load(sys.stdin)["releases"][0]["lanes"] if l["lane"] == "bf16"))' 2>/dev/null)
"$CLI" --json model upload "circlestone-labs/anima@$bf16" "$q" --lane bf16 > cli-copy.ndjson 2>&1 || cli_fail "model upload circlestone-labs/anima@$bf16 $q: $(tail -c 400 cli-copy.ndjson)"
qc=$(model_details "$q" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["commits"][0]["commit"])' 2>/dev/null)
"$CLI" --json model quantize "$q@$qc" --lane int8 --package anima --on "$M" > cli-quantize.ndjson 2>&1 \
  || cli_fail "model quantize --lane int8: $(tail -c 400 cli-quantize.ndjson)"
"$CLI" --json model quantize "$q@$qc" --lane fp8 --package anima --on "$M" > cli-recipe.ndjson 2>&1 \
  || cli_fail "model quantize --lane fp8: $(tail -c 400 cli-recipe.ndjson)"
model_details "$q" > cli-quantized.json 2>&1 || cli_fail "model metadata for $q: $(tail -c 300 cli-quantized.json)"
grep -q '"lane": *"int8"' cli-quantized.json || cli_fail "model quantize --lane int8: $q has no int8 commit"
grep -q '"lane": *"fp8"' cli-quantized.json || cli_fail "model quantize --lane fp8: $q has no fp8 commit"
# `model upload` of a Hub model by name copies the lane --lane names, as `download --lane` reads it: bf16 weights.
# A lane naming a precision family is its weights' width: the int8 commit as local/, labelled bf16, is refused.
"$CLI" --json model upload circlestone-labs/anima "$q" --lane bf16 --on "$M" > cli-upload-lane.ndjson 2>&1 \
  || cli_fail "model upload circlestone-labs/anima --lane bf16: $(tail -c 400 cli-upload-lane.ndjson)"
held=$(python3 -c 'import json,sys; r=[json.loads(l) for l in open(sys.argv[1]) if l.startswith("{")]; m=[e for e in r if e.get("type") == "run.completed"][-1]["run"]["result"]["models"][0]; print(m["lane"], m["precision"])' cli-upload-lane.ndjson 2>/dev/null)
[ "$held" = "bf16 bf16" ] || cli_fail "model upload circlestone-labs/anima --lane bf16 copied lane/precision '${held:-none}', not bf16"
q8=$(python3 -c 'import json,sys; print(next(c["commit"] for c in json.load(open(sys.argv[1]))["commits"] if c["lane"] == "int8"))' cli-quantized.json 2>/dev/null)
"$CLI" --json model download "$q@$q8" "local/ci-e2e-q8-$R" --on "$M" > cli-alias.ndjson 2>&1 \
  || cli_fail "model download $q@$q8 local/ci-e2e-q8-$R: $(tail -c 300 cli-alias.ndjson)"
if "$CLI" --json model upload "local/ci-e2e-q8-$R" "$q" --lane bf16 --on "$M" > cli-mislabel.ndjson 2>&1 \
  || ! grep -q commit_lane_mismatch cli-mislabel.ndjson; then
  cli_fail "model upload of int8 weights as lane bf16 was not refused commit_lane_mismatch: $(tail -c 300 cli-mislabel.ndjson)"
fi
"$CLI" --json model delete "$q" -y > /dev/null 2>&1 || cli_fail "model delete $q failed: delete it by hand"

# cli-project (xs-136, cl-716): a project through every verb (`cozy project new|ls|render|approve|status|rename`), its
# renders on $M and its pictures' cleans and the edit on this computer. The project is found from a folder inside it
# and named with -p from elsewhere, and `ls` lists it; a node renamed in project.yaml keeps its versions through
# `rename`, which status suggests. Two entities that mention each other warn and both render.
# `--approve` with -n 2 is refused before anything runs; a re-rolled character, as the newest, puts what reads it out of
# date and nothing else; `--approve` approves its new version; a person's save during a render stands (cozy writes
# only project.lock); the re-cut is timed; the edit cuts by frame and writes edit.otio beside edit.mp4.
project_status() { "$CLI" --json --full project -p "$1" status > "$2" 2> "$2.err" || cli_fail "project status of $1: $(tail -c 300 "$2.err")"; }
project_cell() {
  local P=$PWD/project ok
  rm -rf "$P"; "$CLI" project new "$P" > project-new.log 2>&1 || { cli_fail "project new: $(tail -c 300 project-new.log)"; return; }
  [ -f "$P/project.yaml" ] || cli_fail "project new made no project.yaml"; mkdir -p "$P/notes"
  cat > "$P/project.yaml" <<YAML
version: 3
assets:
  lead: {tool: $(pkg qwen-image-2 generate), instructions: "$CHAR", asset_type: character}
  place: {tool: $(pkg qwen-image-2 generate), instructions: "$SCENE, wide from one end: the nearest objects large on the left, the far end small on the right", asset_type: environment}
  one:
    tool: $(pkg minimax-h3 generate)
    summary: "<lead> in <place>. $MOTION"
    beats: [{duration: 5}]
  start: {tool: cozy/frame, video: <one>, at: 4}
  two:
    tool: $(pkg minimax-h3 generate)
    summary: "<lead> looks back over one shoulder, after <start>, lead in the middle, facing the camera."
    beats: [{duration: 5}]
  edit:
    tool: cozy/edit
    tracks:
      - clips:
          - {video: <one>, out: 96}
          - {video: <two>, in: 24}
YAML
  # Two assets that mention each other: the mention that closes the cycle stays plain text, said, and both render.
  local C=$PWD/project-cycle
  rm -rf "$C"; "$CLI" project new "$C" > project-cycle-new.log 2>&1 || cli_fail "project new for the cycle: $(tail -c 300 project-cycle-new.log)"
  cat > "$C/project.yaml" <<YAML
version: 3
assets:
  hut:
    tool: $(pkg qwen-image-2 generate)
    instructions: A red hut at the end of a wooden pier, as in <pier>.
  pier:
    tool: $(pkg qwen-image-2 generate)
    instructions: A wooden pier with a red hut at its end, as in <hut>.
YAML
  "$CLI" project -p "$C" status > project-cycle.log 2>&1 || cli_fail "project status refused a cycle of mentions: $(tail -c 300 project-cycle.log)"
  grep -q "pier mentions hut, which is made from it: that mention stays plain text" project-cycle.log \
    || cli_fail "project status did not warn of the cycle: $(tail -c 300 project-cycle.log)"
  "$CLI" project -p "$C" render all --on "$M" > project-cycle-render.log 2>&1 || cli_fail "render all with a cycle of mentions: $(tail -c 400 project-cycle-render.log)"
  project_status "$C" project-cycle.json
  jq -e '[.assets[] | select(.standing != "current")] | length == 0' project-cycle.json > /dev/null \
    || cli_fail "after render all, a cycle's assets are not current: $(tail -c 400 project-cycle.json)"
  if "$CLI" project -p "$P" render lead -n 2 --approve --on "$M" > project-approve-n.log 2>&1 || ! grep -q approve_one project-approve-n.log; then
    cli_fail "--approve with -n 2 was not refused: $(tail -c 300 project-approve-n.log)"
  fi
  ( cd "$P/notes" && "$CLI" project render all --on "$M" ) > project-render.log 2>&1 || cli_fail "render all from notes/: $(tail -c 400 project-render.log)"
  project_status "$P" project-status.json
  ok=$(jq -r '[.assets[] | select(.standing != "current")] | length' project-status.json)
  [ "$ok" = 0 ] || cli_fail "after render all, $ok assets are not current: $(tail -c 400 project-status.json)"
  [ -s "$P/edit.mp4" ] || cli_fail "render all cut no edit.mp4"
  jq -e '.tracks.children[] | select(.kind == "Video") | .children[0].source_range | .start_time.value == 0 and .duration.value == 96' \
    "$P/edit.otio" > /dev/null 2>&1 || cli_fail "edit.otio does not cut one's frames 0-96: $(head -c 300 "$P/edit.otio" 2>&1)"
  "$CLI" --json project ls > project-ls.json 2>&1
  jq -e --arg p "$P" '.projects | any(.path == $p and (.missing | not))' project-ls.json > /dev/null \
    || cli_fail "project ls does not list $P: $(tail -c 300 project-ls.json)"
  "$CLI" project -p "$P" render lead --new --on "$M" > project-run.log 2>&1 || cli_fail "render lead: $(tail -c 400 project-run.log)"
  project_status "$P" project-newest.json
  jq -e '[.stale[].name] | sort == ["one", "start", "two"]' project-newest.json > /dev/null \
    || cli_fail "a new newest lead put out of date other than one, start, two: $(jq -c .stale project-newest.json)"
  "$CLI" project -p "$P" approve lead 2 > project-approve.log 2>&1 || cli_fail "project approve lead 2: $(tail -c 300 project-approve.log)"
  "$CLI" project -p "$P" render place --new --approve --on "$M" > project-approve-new.log 2>&1 || cli_fail "render place --approve: $(tail -c 400 project-approve-new.log)"
  project_status "$P" project-status-2.json
  jq -e '.assets[] | select(.name == "place") | .approved == 2' project-status-2.json > /dev/null \
    || cli_fail "render place --approve did not approve version 2: $(jq -c '.assets[] | select(.name == "place")' project-status-2.json)"
  # place renamed yard in project.yaml (and its mentions): status suggests the rename, which keeps its versions.
  sed -i 's/^  place: /  yard: /; s/<place>/<yard>/g' "$P/project.yaml"
  "$CLI" project -p "$P" status > project-orphan.log 2>&1
  grep -q "cozy project rename place yard" project-orphan.log || cli_fail "status did not suggest the rename: $(tail -c 300 project-orphan.log)"
  "$CLI" project -p "$P" rename place yard > project-rename.log 2>&1 || cli_fail "project rename: $(tail -c 300 project-rename.log)"
  project_status "$P" project-renamed.json
  jq -e '(.orphans // []) == [] and (.assets[] | select(.name == "yard") | .approved == 2 and .versions == 2)' project-renamed.json > /dev/null \
    || cli_fail "after rename, yard does not hold place's versions: $(jq -c '[.orphans, (.assets[] | select(.name == "yard"))]' project-renamed.json)"
  # A person's save while a run is in flight stands: cozy writes only project.lock.
  ( "$CLI" project -p "$P" render all --on "$M" > project-race.log 2>&1; echo $? > project-race.exit ) &
  for _ in $(seq 1 1800); do grep -q '"pending"' "$P/project.lock" 2>/dev/null && break; sleep 1; done
  sed -i 's/lead in the middle, facing the camera\./lead in the middle, facing the camera, hands in pockets./' "$P/project.yaml"
  wait
  [ "$(cat project-race.exit)" = 0 ] || cli_fail "render all during a save: $(tail -c 400 project-race.log)"
  grep -q "hands in pockets" "$P/project.yaml" || cli_fail "a save made during render all was lost"
  grep -o "cut .* in [0-9.]* s" project-race.log | tail -1 >> project-recut.txt
}
[ -z "$ONLY" ] || [[ $ONLY == *",cli-project,"* ]] && project_cell

# A render and a run that stop moving (cl-783): `project wait` and a followed `cozy run` each end with 4 once Calcifer says
# so (10 minutes with no progress), each naming how to end it, and both go on until they're canceled. The job stands
# stalled after `wait` left, in status and on the page's stream (`job.updated`), and a real move clears it. About 13
# minutes, the two at once. Weightless: tool.py's `slow` sleeps 700 s a step, weightless `silent` 20 minutes.
stall_cell() {
  local S=$PWD/stall e2e=$HERE/../e2e/cli n waiting running lock=$HOME/.cozy/calcifer/calcifer.lock.holder stream=$PWD/stall-events.sse page
  held_job() { "$CLI" --json --full project -p "$S" status | jq -c '[.board.jobs[]? | select(.asset == "held")][-1] // {}'; }
  # What the cell needs, in seconds, before its 10-minute wait: a test's own cause shows here, not at the end.
  for tool in curl jq; do command -v "$tool" > /dev/null || { cli_fail "stall: no $tool here"; return; }; done
  [ -s "$lock" ] || { cli_fail "stall: no Calcifer lock record at $lock"; return; }
  rm -rf "$S"; "$CLI" project new "$S" > stall-new.log 2>&1 || { cli_fail "stall: project new: $(tail -c 300 stall-new.log)"; return; }
  printf 'version: 4\nassets:\n  held:\n    tool: %s/project/tool.py/slow\n    instructions: A still that stops moving, %s.\n    seconds: 7000\n' "$e2e" "$R" > "$S/project.yaml"
  # What the page hears, from before the render.
  curl -sS -N --max-time 1500 -H "Authorization: Bearer $(jq -r .token "$lock")" "http://$(jq -r .address "$lock")/v1/events" < /dev/null > "$stream" 2> stall-events.err & page=$!
  for _ in $(seq 50); do [ -s "$stream" ] && break; sleep 0.2; done
  [ -s "$stream" ] || { cli_fail "stall: the page's stream said nothing: $(tail -c 200 stall-events.err)"; kill "$page" 2> /dev/null; return; }
  "$CLI" project -p "$S" render held --on "$M" --queue > stall-render.log 2>&1 || cli_fail "stall: render --queue: $(tail -c 300 stall-render.log)"
  # Each exit kept whether or not the shell stops at a command that fails (`set -e`); nothing in the background reads stdin.
  ( code=0; "$CLI" project -p "$S" wait > stall-wait.log 2>&1 || code=$?; echo "$code" > stall-wait.exit ) < /dev/null & waiting=$!
  ( code=0; "$CLI" --json run -p "$S" "$e2e/weightless/silent" seconds:=1200 --on "$M" > stall-run.ndjson 2> stall-run.err || code=$?; echo "$code" > stall-run.exit ) < /dev/null & running=$!
  wait "$waiting" "$running"
  [ "$(cat stall-wait.exit)" = 4 ] || cli_fail "stall: project wait exited $(cat stall-wait.exit), not 4: $(tail -c 300 stall-wait.log)"
  grep -q 'cozy project cancel held' stall-wait.log || cli_fail "stall: wait didn't say how to end it: $(tail -c 300 stall-wait.log)"
  [ "$(cat stall-run.exit)" = 4 ] || cli_fail "stall: cozy run exited $(cat stall-run.exit), not 4: $(tail -c 300 stall-run.err)"
  held_job | jq -e '.stalled != null' > /dev/null || cli_fail "stall: held doesn't stand stalled after wait left: $(held_job)"
  grep '"type":"job.updated"' "$stream" | grep '"asset":"held"' | grep -q '"stalled"' || cli_fail "stall: the page heard no job.updated with held stalled"
  # A real move clears it: slow's next step, 700 s after its last.
  for _ in $(seq 60); do held_job | jq -e '.stalled == null' > /dev/null && break; sleep 5; done
  held_job | jq -e '.stalled == null' > /dev/null || cli_fail "stall: held still stalled after it moved: $(held_job)"
  grep '"type":"job.updated"' "$stream" | grep '"asset":"held"' | tail -1 | grep -vq '"stalled"' || cli_fail "stall: the page's last job.updated for held still says stalled"
  kill "$page" 2> /dev/null || true
  n=$(run_number stall-run.ndjson)
  [ -n "$n" ] && [ "$("$CLI" --json run watch "$n" --once | jq -r .status)" = in_progress ] || cli_fail "stall: run ${n:-?} isn't going on after its follower left"
  "$CLI" project -p "$S" cancel held > stall-cancel.log 2>&1 || cli_fail "stall: project cancel held: $(tail -c 300 stall-cancel.log)"
  [ -z "$n" ] || "$CLI" run cancel "$n" > stall-run-cancel.log 2>&1 || cli_fail "stall: run cancel $n: $(tail -c 300 stall-run-cancel.log)"
}
[ -z "$ONLY" ] || [[ $ONLY == *",cli-stall,"* ]] && stall_cell
