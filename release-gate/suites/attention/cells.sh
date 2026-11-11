# Suite `attention` (README.md, xs-171): every attention kernel this card runs, timed over the grid within this one pod
# and judged against FP32 attention, for the Runtime's attention table. The measuring is the Runtime's own (its built-in
# `cozy_runtime.bench`); drive/ only calls it. collect.py turns the gate directories into the Runtime's
# measurements/attention.json.
KD=$HERE/suites/attention/drive
# The candidate's table against the Runtime source it was built from (table.sh, which a cut also runs before renting).
attention_table=$("$HERE/suites/attention/table.sh" "${RWF:-}" "$rt" 2>&1) \
  && expect attention-table PASS "$(tail -n 1 <<< "$attention_table")" \
  || expect attention-table FAIL "$(grep -h '^FAIL' <<< "$attention_table" | head -3) $(tail -n 1 <<< "$attention_table")"
# A --corpus (the release-gate run whose attention-corpus gate captured it) goes to this pod first: the quality cells and
# the sparse speed points read its Q/K/V, which are the model's, not the card's, so one capture serves every class.
attention_corpus=
if [ -n "$CORPUS" ]; then
  rm -rf corpus-in
  attention_from=$(gh run download "$CORPUS" -R "$REPO" -p 'gate-*-attention-corpus' -D corpus-in > corpus-in.log 2>&1 &&
    find corpus-in -maxdepth 2 -type d -name corpus | head -1)
  if [ -n "$attention_from" ] && tar -C "$attention_from" -cf - . 2>> corpus-in.log |
      "$CLI" machine ssh "$M" -- 'rm -rf /tmp/attention-corpus && mkdir -p /tmp/attention-corpus && tar -C /tmp/attention-corpus -xf -' >> corpus-in.log 2>&1; then
    attention_corpus=/tmp/attention-corpus
    expect attention-corpus-in PASS "$(find "$attention_from" -name '*.safetensors' | wc -l) captures, $(du -sh "$attention_from" | cut -f1), from release gate $CORPUS on $M"
  else
    expect attention-corpus-in FAIL "no corpus from release gate $CORPUS on $M: $(tail -c 300 corpus-in.log | tr '\n' ' ')"
  fi
  rm -rf corpus-in
fi
cell attention-inventory "$KD/attention_inventory" --gpus=1
attention_inv=$(run_get "$OUT" attention-inventory .result.detail)
attention_sets=$(jq -r '.sets[] | join(",")' <<< "$attention_inv" 2>/dev/null)
[ -n "$attention_sets" ] || [ -n "$ONLY" ] || expect attention-inventory FAIL "no inventory: $(run_get "$OUT" attention-inventory .status) $(run_get "$OUT" attention-inventory .reason.message)"
attention_seed=$((R + 710)) attention_n=0
for attention_set in $attention_sets; do
  # A fault names its kernel (the bench's Fault): the set runs again without it, twice at most.
  for attention_try in 0 1 2; do
    attention_cell=attention-speed-$attention_n
    [ "$attention_try" = 0 ] || attention_cell=$attention_cell-r$attention_try
    cell "$attention_cell" "$KD/attention_speed" kernels="$attention_set" corpus="$attention_corpus" seed:=$((attention_seed++)) --gpus=1
    [ "$(run_get "$OUT" "$attention_cell" .status)" != completed ] || break
    attention_fault=$(run_get "$OUT" "$attention_cell" .reason.message | grep -oE 'fault in [^ ]+' | head -1 | cut -d' ' -f3)
    if [ -z "$attention_fault" ]; then
      # A fault that names no kernel (the GPU's own, an Xid: release gate 43112805556 on an RTX PRO 6000) took the whole
      # process: each kernel of the set runs alone, so the one that faults is named and the rest are timed (SDPA joins each).
      if [[ $(run_get "$OUT" "$attention_cell" .reason.code) == gpu_fault* || $(run_get "$OUT" "$attention_cell" .reason.message) == *Xid* ]]; then
        log "$attention_cell: the GPU faulted with no kernel named; each kernel of the set alone"
        for attention_one in ${attention_set//,/ }; do
          [ "$attention_one" != sdpa ] || continue
          cell "attention-speed-$attention_n-$attention_one" "$KD/attention_speed" kernels="$attention_one" corpus="$attention_corpus" seed:=$((attention_seed++)) --gpus=1
        done
      fi
      break
    fi
    log "$attention_cell: $attention_fault faulted; the set again without it"
    attention_set=$(tr , '\n' <<< "$attention_set" | grep -vxF "$attention_fault" | paste -sd,)
  done
  attention_n=$((attention_n + 1))
done
for attention_kernel in $(jq -r '.kernels[]' <<< "$attention_inv" 2>/dev/null); do
  cell "attention-quality-$attention_kernel" "$KD/attention_quality" kernel="$attention_kernel" corpus="$attention_corpus" seed:=$((attention_seed++)) --gpus=1
done
