# run-cancel (xs-213): one run cancelled by its id from a runner's sign-in (`cozy run cancel`), as a person cancels one.
# params: run (its id, run-<hex>[~n]). Rents nothing.
. "$(dirname "$0")/lib.sh"
RUN=$(param run '')
[[ $RUN =~ ^run-[0-9a-f]+(~[0-9]+)?$ ]] || fail "run: a run id (run-<hex>[~n])"
step cancel run cancel "$RUN" || fail "cozy run cancel $RUN: $(tail -c 600 "$OUT/cancel.txt")"
say "cancelled $RUN: $(tail -n 4 "$OUT/cancel.txt" | tr '\n' ' ' | cut -c1-400)"
