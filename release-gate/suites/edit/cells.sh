# Suite `edit` (se-153): a chain of Qwen Image 2 edits keeps the photo it started from, and what its edits drew. Paul, 11-17, on
# edits of a photo: "if you use images as references, like A -> B -> C and keep editing them, it introduces noise"; "see if you
# can do anything about chained loss".
# Run when a release changes qwen-image-2 or the Runtime: escaped.tsv's `packages=qwen-image-2` rows require it then.
# The bench (sessions/fixtures/qwen-edit, sources.tsv): a Wikimedia Commons photo, sha256-checked, built on this runner by make_set.py.
# edit-chain-g1..g<N>: CHAIN's small edits from the 1280x1600 photo (chain/miyara-0.png), each output the next one's picture, as a
# box edits a take's file: qwen-image-2/generate_turbo, the Image tool's defaults (Turbo, 2 MP), prompt upsampling off so the words
# are the cell's, each at a fixed seed (as suite upscale's): the same code makes the same pictures and gets the same verdict every
# run. An edit's prompt is an instruction, so each cell says what its picture should show (SHOWS, its `caption=`: quality.py).
# holds.py scores the last generation on this runner (CPU pyiqa, OpenCV):
# - edit-chain-hold: the box no edit touches (sources.tsv `still`: her eyes), aligned to generation 0, its DISTS to generation 0
#   at most CHAIN_BAR. At these seeds rc/0.27.0 measured 0.164 (0.168 at other seeds, gate 44240527400), keep_unchanged 0.023.
# - edit-chain-crisp: the box the first edit drew (`made`: her shirt), its fine detail (the variance of luminance's Laplacian) at
#   most CRISP_BAR times generation 1's: later edits must not re-render what an earlier one drew. Re-rendered, it sharpens every
#   time (crackled, haloed stripes by the fifth edit, 3.3x): at these seeds rc/0.27.0 measured 2.30x, keep_unchanged 1.05x.
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
CHAIN_BAR=0.08 CRISP_BAR=1.5
EDIT=$HERE/sessions/fixtures/qwen-edit EDIT_PY=${RUNNER_TEMP:-/tmp}/edit-iqa
ref() { jq -cn --arg i "$1" '[{name: "photo", image: $i}]'; }
# The bench first (Pillow alone, seconds); the scorer's torch, pyiqa and OpenCV install while the cells run.
( uv venv -q --python 3.12 "$EDIT_PY" && uv pip install -q --python "$EDIT_PY/bin/python" pillow &&
  "$EDIT_PY/bin/python" -I "$EDIT/make_set.py" --fetch ) > edit-bench.log 2>&1
EDIT_READY=$?
uv pip install -q --python "$EDIT_PY/bin/python" --torch-backend cpu pyiqa==0.1.16 'setuptools<81' opencv-python-headless > edit-scorer.log 2>&1 &
EDIT_SCORER=$!
picture='' first=''
if [ "$EDIT_READY" = 0 ]; then
  picture=$EDIT/chain/miyara-0.png
  for g in "${!CHAIN[@]}"; do
    c=edit-chain-g$((g + 1))
    cell "$c" "$(pkg qwen-image-2 generate_turbo)" prompt="${CHAIN[$g]}" caption="${SHOWS[$g]}" "references:=$(ref "$picture")" \
      seed:=$((1551 + g)) prompt_upsampling:=false
    picture=$(output "$c" 'webp|png')
    [ -n "$picture" ] || { log "$c: no picture"; break; }
    [ -n "$first" ] || first=$picture
  done
fi
if ! wait "$EDIT_SCORER" || [ "$EDIT_READY" != 0 ]; then
  why="no bench or scorer: $(tail -c 300 edit-bench.log) $(tail -c 300 edit-scorer.log)"
  expect edit-chain-hold FAIL "$why"; expect edit-chain-crisp FAIL "$why"
elif [ -z "$picture" ]; then
  expect edit-chain-hold FAIL "qwen-image-2: the chain made no last picture"; expect edit-chain-crisp FAIL "qwen-image-2: the chain made no last picture"
else
  line=$("$EDIT_PY/bin/python" -I "$EDIT/holds.py" chain miyara "$CHAIN_BAR" "$EDIT/chain/miyara-0.png" "$picture" 2>&1 | tail -1)
  expect edit-chain-hold "$([[ $line == PASS* ]] && echo PASS || echo FAIL)" "qwen-image-2: ${line#* }"
  line=$("$EDIT_PY/bin/python" -I "$EDIT/holds.py" crisp miyara "$CRISP_BAR" "$first" "$picture" 2>&1 | tail -1)
  expect edit-chain-crisp "$([[ $line == PASS* ]] && echo PASS || echo FAIL)" "qwen-image-2: ${line#* }"
fi
