# h3-kitchen-carry: one rental, the Runtime/TensorFS/cozy candidate cohort, and the production-width H3 DiT gate
# (suites/execution/h3-kitchen-carry): eager twice (run-to-run control), then native resident segments with the
# Kitchen producer's previous-step summaries from step 0 (S1), after one eager step (S2) and seeded at 15 (S4).
# The checkpoint is written and uploaded on the pod as a private model, deleted when the session ends.
# params:
#   tree           packages_refs name holding the candidate minimax-h3 (h3)
#   runtime, tensorfs, tensord   the cohort the machine fetches (0.34.0, 0.17.1, 0.13.0)
#   layers         DiT layers at H3's widths (4: blocks 0-1 dense paths, 2-3 Kitchen sites)
#   latent_frames  107 is the 15 s, 362-frame request; lower only if the card cannot hold it
#   cases          native starts to compare (S1,S2,S4)
#   model          private model name prefix (fidika/ci-h3-kitchen-carry)
#   traffic_gb     GB the provider bills (30)
#   need_s         seconds the cap must buy (3600)
. "$(dirname "$0")/lib.sh"
SUITE=$(cd "$(dirname "$0")/../suites/execution/h3-kitchen-carry" && pwd) || fail "no h3-kitchen-carry suite in this checkout"
TREE=$(param tree h3) RT=$(param runtime 0.34.0) TFS=$(param tensorfs 0.17.1) TD=$(param tensord 0.13.0)
LAYERS=$(param layers 4) LF=$(param latent_frames 107) CASES=$(param cases S1,S2,S4)
MODEL=$(param model fidika/ci-h3-kitchen-carry)-$(date -u +%m%d%H%M%S) SEED=$(( $(date +%s) % 100000000 ))
H3=$PACKAGES/$TREE/minimax-h3
[ -d "$H3/minimax_h3" ] || fail "packages/$TREE has no minimax-h3"
python3 "$SUITE/prepare.py" "$H3" "$OUT/build" > "$OUT/prepare.txt" 2>&1 || fail "prepare: $(tail -2 "$OUT/prepare.txt")"
BUILD=$OUT/build
[ "$(jq -r .package_commit "$BUILD/ci_h3_kitchen_carry/source.json")" = "$(git -C "$H3" rev-parse HEAD)" ] || fail "fixture is not the checkout"
say "fixture from packages $(git -C "$H3" rev-parse --short HEAD): $(jq -c .files "$BUILD/ci_h3_kitchen_carry/source.json")"
[[ $LAYERS =~ ^[0-9]+$ && $LF =~ ^[0-9]+$ && $CASES =~ ^(S1|S2|S4)(,(S1|S2|S4))*$ ]] || fail "bad layers/latent_frames/cases"
if [ "$RENT" = true ]; then
  [ "$(cozy --version | awk '{print $2}')" = "$TD" ] || fail "the controller's cozy is $(cozy --version), not $TD"
fi
UPLOADED=''
drop_model() {
  [ -n "$UPLOADED" ] || return 0
  if cozy model delete "$MODEL" -y >> "$OUT/model-delete.txt" 2>&1; then say "deleted $MODEL"
  else say "model delete $MODEL failed: $(tail -1 "$OUT/model-delete.txt")"; fi
}
trap 'st=$?; drop_model; (exit $st); finish' EXIT
# record <step>: the run it named, as JSON; detail <step> <jq>: a field of its result's JSON detail.
record() { local n; n=$(runno "$1"); [ -n "$n" ] || return 1; cozy run show "$n" --once --json > "$OUT/$1.json" 2>&1; }
detail() { jq -r ".result.detail | fromjson | $2" "$OUT/$1.json"; }
quote "$(param traffic_gb 30)" "$(param need_s 3600)"
rent

step update machine update "$NAME" --runtime "$RT" --tensorfs "$TFS" --tensord "$TD" -y || fail "machine update"
cozy machine show "$NAME" --json > "$OUT/show-updated.json" 2>&1
jq -e --arg r "$RT" --arg t "$TFS" --arg d "$TD" '.status.runtime==$r and .status.tensorfs==$t and .status.version==$d' \
  "$OUT/show-updated.json" >/dev/null || fail "machine runs $(jq -c '.status | {version, runtime, tensorfs}' "$OUT/show-updated.json")"
say "ASSERT $NAME runs cozy $TD, Runtime $RT, TensorFS $TFS"

step source run "$BUILD/source" "seed=$SEED" "layers=$LAYERS" --on "$NAME" || fail "source run"
record source && [ "$(jq -r .status "$OUT/source.json")" = completed ] || fail "source did not complete"
step upload model upload "run:$(runno source)#model" "$MODEL" --lane bf16 --on "$NAME" -y || fail "upload"
UPLOADED=1
COMMIT=$(cozy model info "$MODEL" --json 2>/dev/null | jq -r '.commits[0].commit // empty')
[ -n "$COMMIT" ] || fail "no commit for $MODEL"
say "checkpoint $MODEL@$COMMIT ($LAYERS layers at H3 widths)"
GPU=("$BUILD/gpu" "model.model=$MODEL@$COMMIT" "latent_frames=$LF") ON=(--on "$NAME" --gpus=1)

# The first calls build what they serve; the gate needs the Kitchen producer on every sparse block.
want=$((LAYERS - 2))
for try in 1 2 3; do
  step "warm-$try" run "${GPU[@]}" mode=warm "seed=$((SEED + try))" "${ON[@]}" || fail "warm-$try"
  record "warm-$try" || fail "warm-$try has no run"
  have=$(detail "warm-$try" '.producer_sites | length' 2>/dev/null); have=${have:-0}
  say "warm-$try: Kitchen producer on $have/$want sparse blocks; $(detail "warm-$try" '.backends | to_entries | map(.value) | unique | join(",")')"
  [ "$have" -ge "$want" ] && break
  [ "$try" = 3 ] && fail "the Kitchen producer never served every sparse block"
  sleep 120
done

step gate run "${GPU[@]}" mode=gate "seed=$((SEED + 10))" "cases=$CASES" "${ON[@]}" || fail "gate run"
record gate || fail "gate has no run"
detail gate . > "$OUT/gate-report.json"
say "gate: eager deterministic $(detail gate .eager_deterministic) (A/B max abs $(detail gate .eager_ab_max_abs)); first summaries at step $(detail gate '.first_stats_step | join(",")')"
for c in ${CASES//,/ }; do
  say "$c: $(detail gate ".native.$c | \"\(.segments) video_exact=\(.video_exact) audio_exact=\(.audio_exact) max_abs=\(.max_abs_vs_eager) summaries_exact=\([.final_summaries_exact[]] | all) prepare \(.prepare_s)s execute \(.execute_s)s\"")"
done
[ "$(jq -r .status "$OUT/gate.json")" = completed ] && [ "$(detail gate .passed)" = true ] || fail "gate did not pass"
