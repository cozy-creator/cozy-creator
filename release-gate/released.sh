# released.sh, a --cells hook (`-f cells=release-gate/released.sh` with `-f packages_ref=…`): every cell the packages
# checkout ran, then the same request and seed from the release the Hub serves (<cell>-released) on the same pod; then
# both again on the next seeds (<cell>-again, <cell>-released-again), a pair with both executors warm. analyze.py notes
# each pair's execution, steady step and whether the outputs are identical.
RELEASED_CELLS=$(awk -F'\t' '$2 ~ /^\.\//' cells.tsv | cut -f1 | paste -sd, -)
RELEASED_PKGS=$(awk -F'\t' '$2 ~ /^\.\// { split($2, t, "/"); print t[2] }' cells.tsv | sort -u)
RELEASED_KEEP=$PKGS RELEASED_R=$R TWINS=$RELEASED_CELLS
[ -n "$RELEASED_CELLS" ] || log "released.sh: no cell ran from a packages checkout"
for RELEASED_PASS in '' -again; do
  if [ -n "$RELEASED_PASS" ]; then
    R=$((RELEASED_R + 1)) TWIN=-again PKGS=$RELEASED_KEEP
    for p in $RELEASED_PKGS; do product "$p"; done
  fi
  TWIN=-released$RELEASED_PASS PKGS=
  for p in $RELEASED_PKGS; do product "$p"; done
done
TWIN= TWINS= PKGS=$RELEASED_KEEP R=$RELEASED_R
