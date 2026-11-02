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
bf16=$("$CLI" --json model info circlestone-labs/anima 2>/dev/null | python3 -c 'import json,sys; print(next(l["commit"] for l in json.load(sys.stdin)["releases"][0]["lanes"] if l["lane"] == "bf16"))' 2>/dev/null)
"$CLI" --json model upload "circlestone-labs/anima@$bf16" "$q" --lane bf16 > cli-copy.ndjson 2>&1 || cli_fail "model upload circlestone-labs/anima@$bf16 $q: $(tail -c 400 cli-copy.ndjson)"
qc=$("$CLI" --json model info "$q" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["commits"][0]["commit"])' 2>/dev/null)
"$CLI" --json model quantize "$q@$qc" --lane int8 --package anima --on "$M" > cli-quantize.ndjson 2>&1 \
  || cli_fail "model quantize --lane int8: $(tail -c 400 cli-quantize.ndjson)"
"$CLI" --json model quantize "$q@$qc" --lane fp8 --package anima --on "$M" > cli-recipe.ndjson 2>&1 \
  || cli_fail "model quantize --lane fp8: $(tail -c 400 cli-recipe.ndjson)"
"$CLI" --json model info "$q" > cli-quantized.json 2>&1 || cli_fail "model info $q: $(tail -c 300 cli-quantized.json)"
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
# `rename`, which status suggests. A cycle is refused
# before anything runs, and so is `--approve` with -n 2; a re-rolled character, once approved, puts what reads it out of
# date and nothing else; `--approve` approves its new version; a person's save during a render stands (cozy writes
# only project.lock); the re-cut is timed; the edit cuts by frame and writes edit.otio beside edit.mp4.
project_cell() {
  local P=$PWD/project ok
  rm -rf "$P"; "$CLI" project new "$P" > project-new.log 2>&1 || { cli_fail "project new: $(tail -c 300 project-new.log)"; return; }
  [ -f "$P/project.yaml" ] || cli_fail "project new made no project.yaml"; mkdir -p "$P/notes"
  cat > "$P/project.yaml" <<YAML
type: video
defaults: {entity: {with: $(pkg qwen-image-2 generate)}, setup: {with: $(pkg qwen-image-2 generate)},
           world_state: {with: $(pkg qwen-image-2 generate)}, shot: {with: $(pkg minimax-h3 generate)},
           clean: {with: $(pkg birefnet remove_background)}}
entities:
  - {name: lead, kind: character, description: "$CHAR"}
  - name: place
    kind: environment
    description: "$SCENE"
    setups: [{name: a, camera: "wide from one end: the nearest objects large on the left, the far end small on the right"}]
world_states:
  - {name: start, init: "<lead> stands in <place>.", blocking: "lead in the middle, facing the camera."}
shots:
  - {name: one, setup: place.a, prompt: "<lead> in <place>. $MOTION", duration_s: 5, writes: [{state: start}]}
  - {name: two, prompt: "<lead> looks back over one shoulder, after <start>.", duration_s: 5}
edit: [{shot: one, end: 96}, {shot: two, start: 24}]
YAML
  cp "$P/project.yaml" project-ok.yaml
  # lead's description naming <start>, whose init names <lead>, is a cycle.
  sed -i "0,/description: \"/s//description: \"After <start>, /" "$P/project.yaml"
  if "$CLI" project -p "$P" status > project-cycle.log 2>&1 || ! grep -q "a cycle: " project-cycle.log; then
    cli_fail "project status did not refuse a cycle: $(tail -c 300 project-cycle.log)"
  fi
  cp project-ok.yaml "$P/project.yaml"
  if "$CLI" project -p "$P" render lead -n 2 --approve --on "$M" > project-approve-n.log 2>&1 || ! grep -q approve_one project-approve-n.log; then
    cli_fail "--approve with -n 2 was not refused: $(tail -c 300 project-approve-n.log)"
  fi
  ( cd "$P/notes" && "$CLI" project render all --on "$M" ) > project-render.log 2>&1 || cli_fail "render all from notes/: $(tail -c 400 project-render.log)"
  "$CLI" --json project -p "$P" status > project-status.json 2>&1
  ok=$(jq -r '[.nodes[] | select(.kind != "upscale" and .status != "current")] | length' project-status.json 2>/dev/null)
  [ "$ok" = 0 ] || cli_fail "after render all, $ok nodes are not current: $(tail -c 400 project-status.json)"
  [ -s "$P/edit.mp4" ] || cli_fail "render all cut no edit.mp4"
  jq -e '.tracks.children[] | select(.kind == "Video") | .children[0].source_range | .start_time.value == 0 and .duration.value == 96' \
    "$P/edit.otio" > /dev/null 2>&1 || cli_fail "edit.otio does not cut one's frames 0-96: $(head -c 300 "$P/edit.otio" 2>&1)"
  "$CLI" --json project ls > project-ls.json 2>&1
  jq -e --arg p "$P" '.projects | any(.path == $p and (.missing | not))' project-ls.json > /dev/null \
    || cli_fail "project ls does not list $P: $(tail -c 300 project-ls.json)"
  "$CLI" project -p "$P" render lead --on "$M" > project-run.log 2>&1 || cli_fail "render lead: $(tail -c 400 project-run.log)"
  "$CLI" project -p "$P" approve lead 2 > project-approve.log 2>&1 || cli_fail "project approve lead 2: $(tail -c 300 project-approve.log)"
  grep -q "out of date now: one, start, two$" project-approve.log \
    || cli_fail "approving lead put out of date other than one, start, two: $(tail -c 300 project-approve.log)"
  "$CLI" project -p "$P" render place --approve --on "$M" > project-approve-new.log 2>&1 || cli_fail "render place --approve: $(tail -c 400 project-approve-new.log)"
  "$CLI" --json project -p "$P" status > project-status-2.json 2>&1
  jq -e '.nodes[] | select(.name == "place") | .approved == 2' project-status-2.json > /dev/null \
    || cli_fail "render place --approve did not approve version 2: $(jq -c '.nodes[] | select(.name == "place")' project-status-2.json)"
  # place renamed yard in project.yaml (and its mentions): status suggests the rename, which keeps its versions.
  sed -i 's/name: place$/name: yard/; s/<place>/<yard>/g; s/setup: place\.a/setup: yard.a/' "$P/project.yaml"
  "$CLI" project -p "$P" status > project-orphan.log 2>&1
  grep -q "cozy project rename place yard" project-orphan.log || cli_fail "status did not suggest the rename: $(tail -c 300 project-orphan.log)"
  "$CLI" project -p "$P" rename place yard > project-rename.log 2>&1 || cli_fail "project rename: $(tail -c 300 project-rename.log)"
  "$CLI" project -p "$P" rename place.a yard.a >> project-rename.log 2>&1 || cli_fail "project rename of the setup: $(tail -c 300 project-rename.log)"
  "$CLI" --json project -p "$P" status > project-renamed.json 2>&1
  jq -e '(.orphans // []) == [] and (.nodes[] | select(.name == "yard") | .approved == 2 and .versions == 2)' project-renamed.json > /dev/null \
    || cli_fail "after rename, yard does not hold place's versions: $(jq -c '[.orphans, (.nodes[] | select(.name == "yard"))]' project-renamed.json)"
  # A person's save while a run is in flight stands: cozy writes only project.lock.
  ( "$CLI" project -p "$P" render all --on "$M" > project-race.log 2>&1; echo $? > project-race.exit ) &
  for _ in $(seq 1 1800); do grep -q '"pending"' "$P/project.lock" 2>/dev/null && break; sleep 1; done
  sed -i 's/lead in the middle, facing the camera./lead in the middle, facing the camera, hands in pockets./' "$P/project.yaml"
  wait
  [ "$(cat project-race.exit)" = 0 ] || cli_fail "render all during a save: $(tail -c 400 project-race.log)"
  grep -q "hands in pockets" "$P/project.yaml" || cli_fail "a save made during render all was lost"
  grep -o "cut .* in [0-9.]* s" project-race.log | tail -1 >> project-recut.txt
}
[ -z "$ONLY" ] || [[ $ONLY == *",cli-project,"* ]] && project_cell
