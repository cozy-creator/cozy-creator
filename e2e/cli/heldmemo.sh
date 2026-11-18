# held_memo (cl-1225). Found 11-18 in se-153's qwen-rerender session (44246133433, an RTX 4090): PRO-24 asked exactly what an
# earlier run had (prompt, picture, seed), so its machine answered it from that run's held memo, and it failed at its first step:
# `object_storage_failed: object bookkeeping: attempt to write a readonly database`. #3843 gave the journal's reads a second
# connection; recalling a memo records its use, a write, and ran on it. A memo held for no project is in its machine's journal: every
# run on a machine other than this computer's (a rental, an added machine). Paul, 11-18: "you can only memoize when you have a fixed
# seed … Memoize based on exact input (including seed)". Here a stand-in (machineset.sh's: another user's cozy on this runner, added
# here) runs one memoized weightless function with an optional seed: the same words twice with no seed both run, neither answered
# from a memo (each takes a seed of its own from Calcifer, so two computations); then three times with one seed: the first runs, the
# second and third are answered from its memo (`memo.memoized`), and each completes. Linux, a GitHub runner.
held_memo() {
step "a run repeated on another computer's machine is answered from its held memo only with its seed (cl-1225)"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "held memo: its stand-in is another user of a fresh GitHub runner"; return; }
local tool here_memo at memoized why words="held memo $GITHUB_RUN_ID"
here_memo=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
tool=$here_memo/project/counted.py/count
cozy --json machine show local > "$OUT/set-local.json" || { check "held memo: machine show local"; return; }
stand_in stand-m || return
for at in 1 2; do
  run_cell "$OUT" "unseeded-$at" "$tool" "text=$words" --on stand-m \
    || check "held memo: run $at of the same words with no seed on stand-m $(run_get "$OUT" "unseeded-$at" .status): $(run_get "$OUT" "unseeded-$at" .reason.message | head -c 400)"
  # Calcifer gives a request that names no seed its own (`runs::bind` `seed`, from the run's key): its computation is its own.
  memoized=$(run_get "$OUT" "unseeded-$at" .memo.memoized) why=$(run_get "$OUT" "unseeded-$at" .memo.computation)
  echo "held memo: unseeded run $at on $(run_get "$OUT" "unseeded-$at" .machine): $(run_get "$OUT" "unseeded-$at" .status), memoized ${memoized:-false}, computation ${why:-none}"
  [ "$memoized" != true ] || check "held memo: run $at of the same words with no seed was answered from a memo"
done
[ -z "$(run_get "$OUT" unseeded-1 .memo.computation)" ] || [ "$(run_get "$OUT" unseeded-1 .memo.computation)" != "$(run_get "$OUT" unseeded-2 .memo.computation)" ] \
  || check "held memo: two requests naming no seed were keyed as one computation: the second could be answered from the first"
for at in 1 2 3; do
  run_cell "$OUT" "memo-$at" "$tool" "text=$words" seed:=7 --on stand-m \
    || check "held memo: run $at of the same words and seed on stand-m $(run_get "$OUT" "memo-$at" .status): $(run_get "$OUT" "memo-$at" .reason.message | head -c 400)"
  memoized=$(run_get "$OUT" "memo-$at" .memo.memoized)
  echo "held memo: seeded run $at on $(run_get "$OUT" "memo-$at" .machine): $(run_get "$OUT" "memo-$at" .status), memoized ${memoized:-false}, computation $(run_get "$OUT" "memo-$at" .memo.computation | cut -c1-19)"
  [ "$(run_get "$OUT" "memo-$at" .status)" != completed ] || [ -n "$(run_get "$OUT" "memo-$at" .memo.computation)" ] \
    || check "held memo: seeded run $at wasn't keyed for a memo"
  case $at in
    1) [ "$memoized" != true ] || check "held memo: the first seeded run was answered from a memo: nothing ran" ;;
    *) [ "$(run_get "$OUT" "memo-$at" .status)" != completed ] || [ "$memoized" = true ] \
         || check "held memo: run $at, the same words and seed again, ran instead of being answered from the first's memo" ;;
  esac
done
timeout 60 "$COZY" --json machine end stand-m --force -y > "$OUT/stand-m-end.json" 2>&1 || true
}
