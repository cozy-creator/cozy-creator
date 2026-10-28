# model-upload: small public sources (LoRAs) uploaded from the runner itself as private commits, by this run's cozy (a
# dev build's converter tables, say); no rental. Without `rent`, the $0 pre-flight: the sources and the account only.
# params:
#   sources  space-separated name=source pairs; each becomes a private commit of its model
#   lane     the commits' lane (bf16)
. "$(dirname "$0")/lib.sh"
SOURCES=$(param sources) LANE=$(param lane bf16)
read -ra pairs <<<"$SOURCES"
[ "${#pairs[@]}" -gt 0 ] || fail "no sources"
say "cozy $(cozy --version); $(cozy whoami 2>&1 | grep -E '^account')"
for p in "${pairs[@]}"; do [[ $p == ?*=?* ]] || fail "not name=source: $p"; done
[ "$RENT" = true ] || { say "pre-flight only: nothing uploaded"; exit 0; }
for p in "${pairs[@]}"; do
  m=${p%%=*} n=${p%%=*}; n=${n//\//-}
  step "upload-$n" model upload "${p#*=}" "$m" --lane "$LANE" --on local || fail "upload $m"
  c=$(awk '$1=="commit"{c=$2} END{sub(/.*@/, "", c); print c}' "$OUT/upload-$n.txt")
  [ -n "$c" ] || fail "upload $m named no commit"
  cozy model info "$m" > "$OUT/info-$n.txt" 2>&1
  say "COMMIT $m $LANE $c"
done
say "PASS: ${pairs[*]%%=*}"
