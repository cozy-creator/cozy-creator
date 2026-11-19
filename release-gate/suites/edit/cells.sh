# Suite `edit` (se-153): a chain of Qwen Image 2 edits keeps the photo it started from, and what its edits drew. Paul, 11-17, on
# edits of a photo: "if you use images as references, like A -> B -> C and keep editing them, it introduces noise"; "see if you
# can do anything about chained loss".
# Run when a release changes qwen-image-2 or the Runtime: escaped.tsv's `packages=qwen-image-2` rows require it then.
# The bench (sessions/fixtures/qwen-edit, sources.tsv): a Wikimedia Commons photo kept in the private benchmarks repo's
# edit/originals (the workflow's checkout, EDIT_ORIGINALS), sha256-checked, built on this runner by make_set.py.
# edit-chain-g1..g<N>: CHAIN's small edits from the 1280x1600 photo (chain/miyara-0.png), each output the next one's picture, as a
# box edits a take's file: qwen-image-2/generate_turbo, the Image tool's defaults (Turbo, 2 MP), prompt upsampling off so the words
# are the cell's, each at a fixed seed (as suite upscale's): the same code makes the same pictures and gets the same verdict every
# run. An edit's prompt is an instruction, so each cell says what its picture should show (SHOWS, its `caption=`: quality.py).
# A link that did not run (--only names other cells, --skip-unchanged found it unchanged, known-failing.json) makes every row SKIP; one that ran
# and made no picture is a FAIL.
# holds.py scores the chain on this runner (CPU pyiqa, OpenCV). The pair it was set on, both at 2dd607694b on one rtx-4090:
# release-gate 44243798792 (packages rc/0.27.0 d172b4fc, no paste-back) and 44244918172 (#992's 346b3d21, keep_unchanged).
# - edit-chain-hold: the box no edit touches (sources.tsv `still`: her eyes), aligned to generation 0, its DISTS to generation 0
#   at most CHAIN_BAR: 0.165 without paste-back, 0.026 with it.
# - edit-chain-crisp: the box the first edit drew (`made`: her shirt), its fine detail (the variance of luminance's Laplacian) at
#   most CRISP_BAR times generation 1's: later edits must not re-render what an earlier one drew. Re-rendered, it sharpens every
#   time (crackled, haloed stripes by the fifth edit, 3.3x): 2.21x without paste-back, 1.06x with it.
# - edit-chain-lands: each edit's own boxes (`lands`: the shirt, the ears, the backdrop's top corners) in its generation, aligned
#   to the one before, at least LANDS_BAR of their pixels moved by more than 10 dE: a paste-back that keeps too much leaves them
#   as they were. 93.7% / 22.3% / 100.0% without paste-back, 94.0% / 22.4% / 100.0% with it; 0.0% for an edit replaced by its
#   own picture (generation 0 scored as its own first edit, and likewise the second and third).
CHAIN=(
  "Edit <photo>: make her striped shirt red. Keep everything else exactly the same."
  "Edit <photo>: add small gold hoop earrings. Keep everything else exactly the same."
  "Edit <photo>: change the pink backdrop to a sunny park with green trees. Keep the woman exactly the same."
)
SHOWS=(
  "A studio portrait photo of a smiling woman with curly dark hair in a red striped shirt, pink background"
  "A studio portrait photo of a smiling woman with curly dark hair and gold hoop earrings in a red striped shirt, pink background"
  "A photo of a smiling woman with curly dark hair and gold hoop earrings in a red striped shirt, in a sunny park with green trees"
)
CHAIN_BAR=0.08 CRISP_BAR=1.5 LANDS_BAR=0.10
EDIT=$HERE/sessions/fixtures/qwen-edit EDIT_PY=${RUNNER_TEMP:-/tmp}/edit-iqa
EDIT_ROWS=(edit-chain-hold edit-chain-crisp edit-chain-lands)
ref() { jq -cn --arg i "$1" '[{name: "photo", image: $i}]'; }
# The bench first (Pillow alone, seconds); the scorer's torch, pyiqa and OpenCV install while the cells run.
( uv venv -q --python 3.12 "$EDIT_PY" && uv pip install -q --python "$EDIT_PY/bin/python" pillow &&
  "$EDIT_PY/bin/python" -I "$EDIT/make_set.py" --from "${EDIT_ORIGINALS:-/nonexistent}" ) > edit-bench.log 2>&1
EDIT_READY=$?
uv pip install -q --python "$EDIT_PY/bin/python" --torch-backend cpu pyiqa==0.1.16 'setuptools<81' opencv-python-headless > edit-scorer.log 2>&1 &
EDIT_SCORER=$!
edit_chain=() unrun='' unmade=''
if [ "$EDIT_READY" = 0 ]; then
  edit_chain=("$EDIT/chain/miyara-0.png")
  for g in "${!CHAIN[@]}"; do
    c=edit-chain-g$((g + 1))
    cell "$c" "$(pkg qwen-image-2 generate_turbo)" prompt="${CHAIN[$g]}" caption="${SHOWS[$g]}" "references:=$(ref "${edit_chain[-1]}")" \
      seed:=$((1551 + g)) prompt_upsampling:=false
    grep -q "^$c"$'\t' cells.tsv || { unrun="$c not run here (--only, --skip-unchanged or known-failing.json)"; break; }
    picture=$(output "$c" 'webp|png')
    [ -n "$picture" ] || { unmade="$c made no picture"; log "$c: no picture"; break; }
    edit_chain+=("$picture")
  done
fi
if ! wait "$EDIT_SCORER" || [ "$EDIT_READY" != 0 ]; then
  why="no bench or scorer: $(tail -c 300 edit-bench.log) $(tail -c 300 edit-scorer.log)"
  for edit_row in "${EDIT_ROWS[@]}"; do expect "$edit_row" FAIL "$why"; done
elif [ -n "$unrun" ]; then
  for edit_row in "${EDIT_ROWS[@]}"; do expect "$edit_row" SKIP "qwen-image-2: $unrun"; done
elif [ -n "$unmade" ]; then
  for edit_row in "${EDIT_ROWS[@]}"; do expect "$edit_row" FAIL "qwen-image-2: $unmade"; done
else
  line=$("$EDIT_PY/bin/python" -I "$EDIT/holds.py" chain miyara "$CHAIN_BAR" "${edit_chain[0]}" "${edit_chain[-1]}" 2>&1 | tail -1)
  expect edit-chain-hold "$([[ $line == PASS* ]] && echo PASS || echo FAIL)" "qwen-image-2: ${line#* }"
  line=$("$EDIT_PY/bin/python" -I "$EDIT/holds.py" crisp miyara "$CRISP_BAR" "${edit_chain[1]}" "${edit_chain[-1]}" 2>&1 | tail -1)
  expect edit-chain-crisp "$([[ $line == PASS* ]] && echo PASS || echo FAIL)" "qwen-image-2: ${line#* }"
  line=$("$EDIT_PY/bin/python" -I "$EDIT/holds.py" lands miyara "$LANDS_BAR" "${edit_chain[@]}" 2>&1 | tail -1)
  expect edit-chain-lands "$([[ $line == PASS* ]] && echo PASS || echo FAIL)" "qwen-image-2: ${line#* }"
fi
