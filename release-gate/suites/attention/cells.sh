# Suite `attention` (README.md, xs-171): every attention kernel this card runs, timed over the grid within this one pod
# and judged against FP32 attention, for the Runtime's attention table. The measuring is the Runtime's own (its built-in
# `cozy_runtime.bench`); drive/ only calls it. collect.py turns the gate directories into the Runtime's
# measurements/attention.json.
KD=$HERE/suites/attention/drive
# The candidate's table against the Runtime source it was built from (table.sh, which a cut also runs before renting).
attention_table=$("$HERE/suites/attention/table.sh" "${RWF:-}" "$rt" 2>&1) \
  && expect attention-table PASS "$(tail -n 1 <<< "$attention_table")" \
  || expect attention-table FAIL "$(grep -h '^FAIL' <<< "$attention_table" | head -3) $(tail -n 1 <<< "$attention_table")"
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
    cell "$attention_cell" "$KD/attention_speed" kernels="$attention_set" seed:=$((attention_seed++)) --gpus=1
    [ "$(run_get "$OUT" "$attention_cell" .status)" != completed ] || break
    attention_fault=$(run_get "$OUT" "$attention_cell" .reason.message | grep -oE 'fault in [^ ]+' | head -1 | cut -d' ' -f3)
    [ -n "$attention_fault" ] || break
    log "$attention_cell: $attention_fault faulted; the set again without it"
    attention_set=$(tr , '\n' <<< "$attention_set" | grep -vxF "$attention_fault" | paste -sd,)
  done
  attention_n=$((attention_n + 1))
done
for attention_kernel in $(jq -r '.kernels[]' <<< "$attention_inv" 2>/dev/null); do
  cell "attention-quality-$attention_kernel" "$KD/attention_quality" kernel="$attention_kernel" seed:=$((attention_seed++)) --gpus=1
done
