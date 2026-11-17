# released.sh, a --cells hook (`-f cells=release-gate/released.sh` with `-f packages_ref=…`): every cell the packages
# checkout ran, then the same request and seed from the release the Hub serves (<cell>-released) on the same pod; then
# both again on the next seeds, the release first (<cell>-released-again, <cell>-again): a pair with both executors warm,
# in the other order. analyze.py notes whether each pair's outputs are identical, and the warm pair's execution and
# steady step. The first pair's times are not compared: the checkout cell ran among the suites, the release's after it
# on a pod the suites had warmed (bakes, memos, kernels; tfs-299).
RELEASED_CELLS=$(awk -F'\t' '$2 ~ /^\.\//' cells.tsv | cut -f1 | paste -sd, -)
RELEASED_PKGS=$(awk -F'\t' '$2 ~ /^\.\// { split($2, t, "/"); print t[2] }' cells.tsv | sort -u)
RELEASED_KEEP=$PKGS RELEASED_R=$R TWINS=$RELEASED_CELLS
[ -n "$RELEASED_CELLS" ] || log "released.sh: no cell ran from a packages checkout"
for RELEASED_PASS in '' -again; do
  [ -z "$RELEASED_PASS" ] || R=$((RELEASED_R + 1))
  TWIN=-released$RELEASED_PASS PKGS=
  for p in $RELEASED_PKGS; do product "$p"; done
  if [ -n "$RELEASED_PASS" ]; then
    TWIN=-again PKGS=$RELEASED_KEEP
    for p in $RELEASED_PKGS; do product "$p"; done
  fi
done
TWIN= TWINS= PKGS=$RELEASED_KEEP R=$RELEASED_R
