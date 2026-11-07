# Suite `disk` (tfs-140, cl-709): a 100 GB machine keeps working past a full disk, which tatara (11-01) did not. Run
# with `--disk 100`. Every check is an `expect` row.
#   disk-envs  sdxl, qwen-image-2 and minimax-h3, each its previous release and then its newest (an update removes the
#              previous environment, logged with its bytes): `machine show` says what fills the disk (models, package
#              environments, Runtime, other)
#   torch-layer  none of those installs, nor seedvr2's (its lock pins torch 2.14.1), brings its own Torch (cl-715): no
#              "installs its own" warning, and no install takes 1 GiB of disk
#   disk-held  a CPU SDXL call running while H3 int8 (68 GiB) and Qwen-Image int8 (17 GiB) land: none of SDXL is evicted,
#              and each download holds its own commit (canceled after: a CPU step takes minutes)
#   disk-lru   H3, Qwen, SDXL, more of H3's lanes until the store has evicted (a rental can hold more than was asked: its
#              offer's own disk, 120 or 160 GB for 100), and H3 again all land: each eviction logged (lane, bytes, idle); the uv
#              cache entries the removed environments alone linked are reclaimed (logged) before any model
#   disk-listed (cl-774, valentino 11-05) past those evictions every byte of the store is a model `model ls` names: a
#              lane goes whole (its commit and its header), so none is unlisted while its chunks stay (within 1%)
#   disk-room  a render that needs more than the disk can make free (H3 bf16 here) fails `machine_disk_full` while it
#              prepares, naming the sizes and `--disk-gb`, with nothing downloaded; SDXL again, which fits only by
#              evicting, runs and its warnings name what was evicted
# DISK_INSTALLS=no (the `disk` session) leaves out disk-envs and torch-layer: their installs are most of the suite's time.
T=$OUT/disk; mkdir -p "$T"
use() { "$CLI" --json machine show "$M" 2>/dev/null | jq -r --arg s "$1" '.status | [$s, .disk_free_bytes, .disk_models_bytes,
  .disk_environments_bytes, .disk_runtime_bytes] | @tsv' >> "$T/use.tsv"; }
step() { local n=$1; shift; run_submit "$T" "$n" "$@" > /dev/null; run_wait "$T" "$n"; use "$n"; run_get "$T" "$n" .status; }
released() { "$CLI" package info "$1" --json 2>/dev/null | jq -r '[.releases[] | select(.yanked | not) | .release][0:2] | reverse | join(" ")'; }
logs() { "$CLI" machine logs "$M" tensord -n 0 > "$T/tensord.log" 2>&1; grep -c -- "$1" "$T/tensord.log"; }

use start; bad=()
if [ "${DISK_INSTALLS:-yes}" != no ]; then
for p in sdxl qwen-image-2 minimax-h3 seedvr2; do
  for r in $(released "$p"); do
    "$CLI" package install "$p@$r" --on "$M" -y > "$T/install-$p-$r.log" 2>&1 || bad+=("$p@$r install failed"); use "$p@$r"
  done
done
read -r _ free models envs runtime < <(tail -1 "$T/use.tsv"); removed=$(logs 'removed package environment')
# A Calcifer older than this machine reports no disk use (version skew): the removals judge it then.
shown=$([ -n "$envs" ] && echo "models $models, environments $envs, Runtime $runtime" || echo "this cozy's Calcifer reports no disk use")
[ ${#bad[@]} -eq 0 ] && [ "$removed" -gt 0 ] && { [ -z "$envs" ] || [ "$envs" -gt 0 ]; } \
  && expect disk-envs PASS "installs and updates done, $removed previous environments removed (logged with bytes); $shown; free $free" \
  || expect disk-envs FAIL "${bad[*]:-}; $removed environments removed; $shown"

own=$(cat "$T"/install-*.log | grep -c 'installs its own'); own=$((own + $(logs 'installs its own')))
most=$(awk -F'\t' 'NR > 1 && p - $2 > m {m = p - $2; w = $1} {p = $2} END {printf "%d %s", m, w}' "$T/use.tsv")
[ "$own" = 0 ] && [ "${most%% *}" -lt $((1 << 30)) ] \
  && expect torch-layer PASS "every install took the machine's Torch; the largest took $(( ${most%% *} >> 20 )) MiB (${most#* })" \
  || expect torch-layer FAIL "$own installs brought their own Torch; the largest took $(( ${most%% *} >> 20 )) MiB (${most#* }); $(grep -h -m2 'installs its own' "$T"/install-*.log "$T/tensord.log" | tr '\n' ' ')"
fi

held=$(basename "$(mktemp -u "$T/run-XXXXXX")")
run_submit "$T" "$held" run "$(pkg sdxl generate)" --on "$M" prompt="$OBJECT, held through a full disk" steps:=8 seed:=$((R + 700)) > /dev/null
run_wait "$T" "$held" in_progress  # accepted: its preparation holds SDXL from here
h3=$(step h3 model download minimaxai/minimax-h3 --lane int8 --on "$M" -y)
qwen=$(step qwen model download qwen/qwen-image-2.1 --lane int8 --on "$M" -y)
# On a CPU the call is minutes a step: it held SDXL while both landed, and it is canceled once they have.
run_show "$T" "$held" "$(cat "$T/$held.number")"; during=$(run_get "$T" "$held" .status)
"$CLI" run cancel "$(cat "$T/$held.number")" -y > "$T/held-cancel.log" 2>&1; run_wait "$T" "$held"
evicted_sdxl=$(logs 'tensorfs: evicted stabilityai/sdxl')
[ "$h3$qwen$during" = completedcompletedin_progress ] && [ "$evicted_sdxl" = 0 ] \
  && expect disk-held PASS "SDXL run $(cat "$T/$held.number") ran while H3 and Qwen landed; SDXL never evicted" \
  || expect disk-held FAIL "h3 $h3, qwen $qwen $(run_get "$T" qwen .reason.message), SDXL run $during, SDXL evicted $evicted_sdxl times"

sdxl=$(step sdxl model download stabilityai/sdxl --lane int8 --on "$M" -y)
# Not fp8: a lane few runs read has cold objects in R2, and one dripped at 52 KB/s here for as long as we watched (tfs-138).
for lane in nvfp4 bf16; do
  [ "$(logs 'tensorfs: evicted')" = 0 ] && step "h3-$lane" model download minimaxai/minimax-h3 --lane "$lane" --on "$M" -y > /dev/null
done
again=$(step h3-again model download minimaxai/minimax-h3 --lane int8 --on "$M" -y)
evictions=$(logs 'tensorfs: evicted'); garbage=$(grep -cE 'reclaimed .*(uv cache entries|package environments)' "$T/tensord.log")
silent=$(grep -cE 'tensorfs: freed .* \(0 evicted' "$T/tensord.log")
[ "$sdxl$again" = completedcompleted ] && [ "$evictions" -gt 0 ] \
  && expect disk-lru PASS "$evictions lanes evicted, $garbage garbage sweeps, $silent sweeps of unreferenced objects; $(grep -m3 'tensorfs: evicted' "$T/tensord.log" | sed 's/.*tensorfs: //' | paste -sd';')" \
  || expect disk-lru FAIL "sdxl $sdxl, h3 again $again; $evictions evictions, $garbage garbage sweeps; free $(tail -1 "$T/use.tsv" | cut -f2); $(run_get "$T" h3-again .reason.message)"

read -r stored listed < <("$CLI" --json machine show "$M" 2>/dev/null | jq -r '.status | "\(.disk_models_bytes // 0) \(.models_bytes // 0)"')
unlisted=$(( stored > listed ? stored - listed : 0 ))
"$CLI" model ls --on "$M" > "$T/model-ls.txt" 2>&1
[ "$stored" -gt 0 ] && [ $((unlisted * 100)) -le "$stored" ] \
  && expect disk-listed PASS "the store's $((stored >> 20)) MiB: $((listed >> 20)) MiB in listed models, $((unlisted >> 20)) MiB other; $(tail -1 "$T/model-ls.txt")" \
  || expect disk-listed FAIL "the store holds $((stored >> 20)) MiB, listed models $((listed >> 20)) MiB: $((unlisted >> 20)) MiB no model names; $(tail -1 "$T/model-ls.txt")"

total=$("$CLI" --json machine show "$M" 2>/dev/null | jq -r '.status.disk_total_bytes // 0'); before=$(tail -1 "$T/use.tsv" | cut -f3)
big=$(step h3-bf16-render run "$(pkg minimax-h3 generate)" --on "$M" --lane bf16 \
  beats:="$(jq -cn --arg d "$OBJECT, too big for this disk" '[{description: $d, duration: 5}]')" seed:=$((R + 702)))
code=$(run_get "$T" h3-bf16-render .reason.code); why=$(run_get "$T" h3-bf16-render .reason.message)
after=$(tail -1 "$T/use.tsv" | cut -f3); grew=$(( after > before ? after - before : 0 ))
fits=$(step sdxl-room run "$(pkg sdxl generate)" --on "$M" prompt="$OBJECT, after a full disk" steps:=1 seed:=$((R + 703)))
said=$("$CLI" --json --full run show "$(cat "$T/sdxl-room.number")" --once 2>/dev/null | jq -r '[.warnings[]?, .result.warnings[]?] | map(if type == "object" then .message else . end) | .[]' | grep -m1 'evicted' || true)
evicted_sdxl=$(logs 'tensorfs: evicted stabilityai/sdxl')
# H3 bf16 is 130 GiB: a disk that could hold it whole once all else is evicted (a CPU pod gets 160 GB for 100) judges SDXL's half only.
if [ "$total" -gt $((135 << 30)) ]; then
  [ "$fits" = completed ] && expect disk-room PASS "a $((total >> 30)) GiB disk holds H3 bf16 (not judged); SDXL ran: ${said:-no eviction needed}" \
    || expect disk-room FAIL "SDXL $fits: $(run_get "$T" sdxl-room .reason.message)"
elif [ "$big$code" = failedmachine_disk_full ] && [[ $why == *--disk-gb* ]] && [ "$grew" -lt $((1 << 30)) ] && [ "$fits" = completed ] \
  && { [ "$evicted_sdxl" = 0 ] || [ -n "$said" ]; }; then
  expect disk-room PASS "H3 bf16 refused while it prepared ($why), its store grew $((grew >> 20)) MiB; SDXL ran: ${said:-no eviction needed}"
else
  expect disk-room FAIL "H3 bf16 $big $code ($why), store grew $((grew >> 20)) MiB; SDXL $fits $(run_get "$T" sdxl-room .reason.message); said: ${said:-nothing}"
fi
