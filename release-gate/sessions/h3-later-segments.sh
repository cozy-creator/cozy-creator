# h3-later-segments (se-130): a later segment, and a redo from a seam, are asked only for what's still to come. A shot's
# summary is the target video's in MiniMax's guide, so the whole shot's summary sent to segment 2, even said to be the whole
# shot's, played segment 1's event again (runs 47464, 47635, 47774, 47808). A 26 s shot of two segments whose summary names
# segment 1's event, then a redo from its seam (frame 362), as the page does it (e2e/page.py), from a packages_refs
# checkout `pk`. Checked: segment 1's summary is as written; segment 2's says only that it continues, under `[video
# continuation …]`, without the event; the redo's whole prompt is segment 2's (Paul 11-12); the redo keeps the take to its
# seam. Frames every second of both takes go to $OUT/frames for a person to judge that segment 2 doesn't replay the wave.
# One rental (a 48 GB card). First proof: se-130 on l40s frederica (runs 47854, 47856). params:
#   runtime     the Runtime wheel, https://…#sha256=<hex> (default: the session's RUNTIME_WHEEL, from rc/<x>)
. "$(dirname "$0")/lib.sh"
URL=$(param runtime) PK=$PACKAGES/pk
[ -f "$PK/minimax-h3/pyproject.toml" ] || fail "no packages_refs checkout pk/minimax-h3"
grep -q 'def later_part' "$PK/minimax-h3/story.py" || fail "pk's minimax-h3 has no later_part: it is older than se-130's fix"
mkdir -p "$OUT/wheel" "$OUT/frames"
if [ -n "$URL" ]; then
  [[ $URL == https://*'#sha256='* ]] || fail "runtime is not https://…#sha256=<hex>"
  W=$OUT/wheel/$(basename "${URL%%#*}")
  curl -fsSL --retry 5 --retry-all-errors "${URL%%#*}" -o "$W" || fail "cannot fetch the runtime wheel"
  [ "$(sha256sum "$W" | cut -c1-64)" = "${URL##*sha256=}" ] || fail "runtime wheel sha256"
else
  W=${RUNTIME_WHEEL:-}
  [ -n "$W" ] || fail "no Runtime: name \`runtime\`, or dispatch from rc/<x>"
fi
command -v ffprobe > /dev/null || fail "no ffprobe (pod-session.yaml installs it from the tool cache)"
[ -f "$REF/seed.jpg" ] || fail "no reference image"
J() { jq -e "$@" > /dev/null; }
newest_run() { run_page asset "$1" | jq -r '.versions[-1].runs[-1]'; }
# prompt <run> <segment> summary|digest: that segment's summary as H3 got it (from its prompt's head in the run's log), or
# the whole prompt's digest.
prompt() {
  cozy --full run watch "$1" --once | python3 -c '
import json, re, sys
seg, part = sys.argv[1], sys.argv[2]
for line in sys.stdin:
    if "h3 segment prompt" in line and re.search(rf"\bsegment={seg}\b", line):
        if part == "digest":
            print(re.search(r"digest=(\w+)", line).group(1))
        else:
            head = json.loads(re.search(r"head=(\"(?:[^\"\\]|\\.)*\")", line).group(1))
            found = re.search(r"^summary:\n(.*?)(?:\n\n[a-z_]+:\n|\Z)", head, re.S | re.M)
            print(found.group(1).strip() if found else "")
        break' "$2" "$3"
}
file_of() {
  local f t=0
  f=$(run_page asset "$1" | jq -r --arg f "$2" '.versions[-1].files[$f].path // empty')
  [ -n "$f" ] || return 1
  until [ -s "$RUN_PROJECT/$f" ] || [ "$t" -ge 600 ]; do sleep 2; t=$((t + 2)); done
  echo "$RUN_PROJECT/$f"
}
sheet() { ffmpeg -v error -y -i "$2" -vf "fps=1,scale=320:-2,tile=6x5:padding=4" -frames:v 1 "$OUT/frames/$1.jpg"; }

quote 3600
rent
step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$W" -y || fail "update"
say "pod $(cozy machine show "$NAME" --json | jq -c '{v: .status.version, rt: .status.runtime, gpu: .status.gpus[0].name}')"

cp "$REF/seed.jpg" "$OUT/lighthouse.jpg"
run_page import "$OUT/lighthouse.jpg" --name lighthouse > "$OUT/import.txt" 2>&1 || fail "import: $(cat "$OUT/import.txt")"
EVENT='a huge wave bursts over the rocks, and the lamp of <lighthouse> goes dark.'
FIELDS=$(jq -cn --arg e "$EVENT" '{summary: ("<lighthouse> stands on a sea cliff in a winter storm; " + $e),
  beats: [{duration: 15, description: "Snow blows past <lighthouse> as its lamp turns. A huge wave bursts over the rocks below and its spray hides the tower; when it clears, the lamp is dark."},
          {duration: 11, description: "The camera pushes in slowly on the dark lantern room of <lighthouse> while snow keeps falling and the sea heaves below."}],
  soundscape: "Howling wind, surf on the rocks.", seed: 130130}')
made=$(run_page add later --tool "$PK/minimax-h3/generate" --fields "$FIELDS" 2>&1)
J '.name == "later"' <<< "$made" || fail "drop later: $made"
say "a 26 s shot of two segments"
run_page render --on "$NAME" later > "$OUT/render-1.txt" 2>&1 || fail "later: $(tail -c 500 "$OUT/render-1.txt")"
first=$(newest_run later)
one=$(prompt "$first" 1 summary) two=$(prompt "$first" 2 summary)
say "segment 1's summary: $one"; say "segment 2's summary: $two"
grep -qF "$EVENT" <<< "$one" && ! grep -q "continues this shot" <<< "$one" || fail "segment 1's summary isn't as written"
grep -q '^\[video continuation' <<< "$two" && grep -q "continues this shot from where its earlier part ends" <<< "$two" \
  && ! grep -qF "$EVENT" <<< "$two" || fail "segment 2's summary isn't a later part's"
kept=$(file_of later video) || fail "no take"; sheet take-1 "$kept"

say "redo from its seam, frame 362"
run_page render --on "$NAME" --pickup '<later.1>' 362 later > "$OUT/render-2.txt" 2>&1 || fail "the redo: $(tail -c 500 "$OUT/render-2.txt")"
redo=$(newest_run later)
J '.result.kept_s > 15.08 and .result.kept_s < 15.09' <<< "$(cozy --json run watch "$redo" --once)" || fail "run $redo didn't keep the take to its seam"
again=$(prompt "$redo" 1 summary)
say "the redo's summary: $again"
[ "$(prompt "$redo" 1 digest)" = "$(prompt "$first" 2 digest)" ] || fail "the redo's prompt isn't segment 2's: $again"
vid=$(file_of later video) || fail "no redo"; sheet take-2 "$vid"
say "PASS: segment 2 and the redo are asked only to go on, with the same prompt; judge $OUT/frames for a second wave"
