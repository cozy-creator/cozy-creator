#!/usr/bin/env bash
# The `cli-release` suite: cozy's signed-in commands before a release (e2e/coverage.toml's `release` tier), on Linux,
# macOS or Windows. Signed in with the CI key (or, by hand with --installed, as this computer is), it reads the
# account's balance and checks its buy page answers, then rents ONE machine of --sku (default: the catalog's cheapest
# 1-GPU offer; out of stock, the cheapest in stock with at least its VRAM and compute capability; nothing dearer than
# --max-hourly an hour; each step is refused once its GPU time reached --max-usd, which ends
# it: the CLI's quote names no storage or traffic, so the cap leaves them headroom). Synthetic package/model
# publication and retained-output checks run separately on a disposable Hub (synthetic.sh). This live suite
# requires that proof log and first puts --pod-tensord on the rental. On the rental: package install and list,
# BiRefNet from a YAML request and with --file, each output landed here (one in a folder named with spaces and
# non-ASCII), SDXL with a LoRA and its other function, a weights output read after TensorD restarts, machine ls, show and logs, keepalive, a run followed and canceled, an end while a run still goes,
# checked gone, and the ended rental in `machine ls --history`. The rental is ended in an EXIT
# trap whatever fails; this script never creates a test repository on the live Hub. --quote stops at the rental's quote and rents nothing: the $0 pre-flight of every step before it.
#   e2e/release/run.sh --out DIR [--key FILE | --installed] [--sku SKU] [--max-hourly USD] [--max-usd USD] [--quote] [--rent-wait S]
#                      [--rent-lock FILE] [--release DIR] [--pins N@R,...] [--publication-log FILE]
#                      [--tensord F|V] [--pod-tensord F|V] [--tensorfs F|V] [--runtime F|V]
. "$(dirname "$0")/../lib.sh"
parse "$@"
here=$(cd "$(dirname "$0")" && pwd)
J() { jq -e "$@" > /dev/null; }
run=$(date -u +%m%d%H%M)$RANDOM
M='' ended=''

# The rental `machine rent` ordered: its id from `rental.accepted`, which names it whether or not the order has a
# machine yet, so the trap ends an order given up on too.
ordered() { jq -r 'select(.type? == "rental.accepted") | .rental_id // empty' "$1" 2> /dev/null | tail -1; }
# Whether `machine ls` lists machine $1, by name or rental id.
listed() { cozy --json machine ls | J --arg m "$1" '.machines | any(.name == $m or .rental_id == $m)'; }
cleanup() {
  set +e
  local rental=$M
  # Until the suite ended its rental itself.
  [ -n "$ended" ] || [ ! -s "$OUT/rent.ndjson" ] || rental=$(ordered "$OUT/rent.ndjson")
  [ -n "$rental" ] || rental=$M
  # `machine end` returns once the Hub affirms the pod gone.
  if [ -n "$rental" ]; then
    cozy --json machine end "$rental" --force -y > "$OUT/end-trap.json" 2>&1
    ! listed "$rental" || echo "::error::the trap ended $rental, but machine ls still lists it: end it by hand"
  fi
  # Calcifer's log holds each Hub answer about the rental (a boot that failed, why).
  mkdir -p "$OUT/cozy-logs"
  [ -n "$INSTALLED" ] || (cd ~/.cozy && find . -name '*.log' -size -20M | tar -cf - -T - | (cd "$OUT/cozy-logs" && tar -xf -)) > /dev/null 2>&1
}
trap cleanup EXIT
# A runner's cancel or timeout signals the script: it exits, so the trap ends the rental.
trap 'exit 1' INT TERM HUP
usd() { awk -v m="$1" 'BEGIN { printf "%.2f", m / 1e6 }'; }
# What the Hub bills the rental so far, or its provider if that's more, in USD: read, as an order is placed afresh and can
# land elsewhere than its quote (42018791462, 42024605457). Unread, its GPU time at the quoted rate from ready, when billing
# starts: an order can wait an hour for a card (cli-release windows-2025, run 41586979774).
rented_at='' hourly=0
spent() {
  local id billed
  id=$(jq -r '.rental_id // empty' "$OUT/rental.json" 2> /dev/null)
  billed=$([ -z "$id" ] || cozy --json machine ls --since 24h 2> /dev/null | jq -er --arg r "$id" \
    '[.rentals[] | select(.rental_id == $r) | [.spend_usd_micros // 0, .provider_cost_usd_micros // 0] | max][0] / 1e6' 2> /dev/null)
  [ -n "$billed" ] && { printf '%.3f' "$billed"; return; }
  awk -v h="$hourly" -v s="$(( $(date +%s) - ${rented_at:-$(date +%s)} ))" 'BEGIN { printf "%.3f", h / 1e6 * s / 3600 }'
}
# Every step after the order first holds the rental to --max-usd: past it the suite fails and the trap ends the pod.
step() {
  echo "::endgroup::"
  [ -z "$M" ] || awk -v s="$(spent)" -v max="$MAX_USD" 'BEGIN { exit !(s < max) }' \
    || fail "the rental's GPU time reached \$$(spent) of --max-usd \$$MAX_USD before \"$*\": ended"
  echo "::group::$*"
}

# watched <name> <cozy args...>: a verb that makes a numbered run, followed to its end by the CLI's own events (run.sh);
# its events in $OUT/<name>.ndjson, its record in <name>.json. 0: it completed.
watched() {
  local name=$1; shift
  [ -n "$(run_submit "$OUT" "$name" "$@")" ] && run_wait "$OUT" "$name" && [ "$(run_get "$OUT" "$name" .status)" = completed ]
}

# A full live run includes the independent, isolated publication proof. Check before installation or sign-in.
if [ -z "$QUOTE" ]; then
  [ -n "${PUBLICATION_LOG:-}" ] && [ -s "$PUBLICATION_LOG" ] || fail "Run the isolated publication suite first and pass --publication-log FILE"
fi
if [ -z "$INSTALLED" ]; then step install; install_cozy; user_services; step candidates; candidates; fi
run_project "$OUT/project" || fail "project initialization failed"

step sign-in
if [ -n "$KEY" ]; then sign_in "$KEY" > "$OUT/login.json" || fail "cozy login --key failed: $(head -c 300 "$OUT/login.json")"; fi
cozy --json whoami > "$OUT/whoami.json"
J '.logged_in == true and .account == "fidika"' "$OUT/whoami.json" || fail "whoami: not signed in as fidika ($(head -c 300 "$OUT/whoami.json"))"

step balance
J --arg hub "${COZY_HUB_URL:-https://tensorhub.com}" '(.billing | length > 0) and (.buy_credits | startswith($hub))' "$OUT/whoami.json" || check "whoami: no balance or buy page ($(head -c 300 "$OUT/whoami.json"))"
curl -fsS -o /dev/null "$(jq -r .buy_credits "$OUT/whoami.json")" || check "whoami: the buy page $(jq -r .buy_credits "$OUT/whoami.json") does not answer"

step "the rental's quote"
# In stock: the catalog offers it with 1 GPU, at that price.
one='def one: [.widths[] | select(.accelerator_count == 1) | .price_usd_micros_per_hour] | min;
  def cc: .compute_capability | split(".") | map(tonumber) | .[0] * 100 + .[1];'
cozy --json machine rent > "$OUT/catalog.json" 2>&1 || fail "machine rent: no catalog: $(head -c 300 "$OUT/catalog.json")"
[ -n "$SKU" ] || SKU=$(jq -r "$one"'[.[] | select(.vram_gb > 0 and one != null)] | sort_by(one)[0].name // empty' "$OUT/catalog.json")
[ -n "$SKU" ] || fail "machine rent: the catalog lists no GPU"
quote() { code=0; cozy --json machine rent "$SKU" --disk-gb 60 > "$OUT/quote.ndjson" 2>&1 || code=$?; }
quote
# Out of stock: the cheapest in-stock SKU with at least its VRAM and compute capability.
if [ "$code" != 2 ] && grep -q sku_out_of_stock "$OUT/quote.ndjson"; then
  read -r sub floor < <(jq -r --arg s "$SKU" "$one"'(map(select(.name == $s))[0]) as $c
    | if $c == null or $c.vram_gb == 0 then "- -" else ($c | {v: .vram_gb, k: cc, f: "≥\(.vram_gb) GB, sm_\(.compute_capability | sub("\\."; ""))+"}) as $f
      | ([.[] | select(.name != $s and .compute_capability != "" and .vram_gb >= $f.v and cc >= $f.k and one != null)]
         | sort_by(one)[0].name // "-") + " " + $f.f end' "$OUT/catalog.json") || fail "machine rent: the catalog does not parse"
  [ "$floor" != - ] || fail "sku $SKU is out of stock and the catalog lists no GPU floor for it"
  [ "$sub" != - ] || fail "sku $SKU is out of stock and nothing in stock has $floor"
  echo "sku $SKU is out of stock; using $sub (cheapest in stock with $floor)"
  SKU=$sub; quote
fi
hourly=$(jq -r 'select(.quote) | .quote.usd_micros_per_hour' "$OUT/quote.ndjson" 2> /dev/null | head -1) || true
[ "$code" = 2 ] && grep -q quote_unagreed "$OUT/quote.ndjson" && [ -n "$hourly" ] \
  || fail "machine rent $SKU without --yes: exit $code, not its quote alone: $(head -c 400 "$OUT/quote.ndjson")"
awk -v h="$hourly" -v max="$MAX_HOURLY" 'BEGIN { exit !(h / 1e6 <= max) }' || fail "machine rent $SKU: \$$(usd "$hourly")/h, over --max-hourly \$$MAX_HOURLY"
minutes=$(awk -v h="$hourly" -v max="$MAX_USD" 'BEGIN { printf "%.0f", max / (h / 1e6) * 60 }')
echo "quote: $SKU at \$$(usd "$hourly")/h; --max-usd \$$MAX_USD buys $minutes min"
if [ -n "$QUOTE" ]; then
  ! listed "ci-e2e-quote-$run" || check "machine ls: the gone check by name lists a machine never rented"
  rented_at=$(($(date +%s) - 60)); [ "$(spent)" != 0.000 ] || check "the spend guard reads \$0 after a minute at \$$(usd "$hourly")/h"
  finish "cli-release --quote: $SKU at \$$(usd "$hourly")/h ($minutes min under --max-usd \$$MAX_USD), nothing rented"
  exit 0
fi

step rental
before=$(cozy --json machine ls | jq '[.machines[] | select(.kind == "rental")] | length') || true
! cozy --json machine rent "$SKU" --gpus 64 -y > "$OUT/rent-too-many.json" 2>&1 || check "machine rent --gpus 64 bought something"
[ "$(cozy --json machine ls | jq '[.machines[] | select(.kind == "rental")] | length')" = "$before" ] || check "machine rent --gpus 64: a rental appeared"
# No pod --rent-wait s after the Hub accepted the order is a not-run, as gate.sh's: an order the Hub never acquires
# (41977686738's sat 56 min) must not hold the run. The wait counts from `rental.accepted`: its rental id is what the trap
# ends the order by, named or not (cl-789), and a rent stopped before it leaves an order no id here names.
# Other renters' lock (fd 9) is held only while ordering: nothing it starts keeps it.
[ -z "$RENT_LOCK" ] || { exec 9> "$RENT_LOCK"; flock 9; }
cozy --json machine rent "$SKU" --disk-gb 60 -y > "$OUT/rent.ndjson" 9>&- & rent_pid=$!
accepted=''
unlock() { [ -z "$RENT_LOCK" ] || { flock -u 9 2> /dev/null; exec 9>&-; }; }
while kill -0 "$rent_pid" 2> /dev/null; do
  [ -n "$accepted" ] || [ -z "$(ordered "$OUT/rent.ndjson")" ] || { accepted=$SECONDS; unlock; }
  if [ -n "$accepted" ] && (( SECONDS - accepted > RENT_WAIT )); then
    kill "$rent_pid" 2> /dev/null; wait "$rent_pid" 2> /dev/null || true
    fail "machine rent $SKU: no pod $(( (SECONDS - accepted) / 60 )) min after the Hub accepted the order: $(tail -c 300 "$OUT/rent.ndjson")"
  fi
  sleep 5
done
unlock
wait "$rent_pid" || fail "machine rent $SKU failed: $(tail -c 400 "$OUT/rent.ndjson")"
rented_at=$(date +%s)
M=$(jq -r '.. | objects | select(.kind? == "rental") | .name // empty' "$OUT/rent.ndjson" 2> /dev/null | tail -1) || true
[ -n "$M" ] || fail "machine rent: no machine named"
echo "rented $M ($SKU)"
cozy --json machine show "$M" > "$OUT/rental.json" && J '.rental.disk_gb >= 60' "$OUT/rental.json" || check "machine rent --disk-gb 60: $(jq .rental.disk_gb "$OUT/rental.json") GB"
cozy --json machine ls | J --arg m "$M" '.machines | any(.name == $m and .kind == "rental")' || check "machine ls does not list $M"
cozy machine logs "$M" --lines 5 > "$OUT/rental-log.txt" && [ -s "$OUT/rental-log.txt" ] || check "machine logs $M --lines 5: nothing"
# A release's TensorD reaches rentals in its image: the rental takes the candidate's first (gate.sh does the same).
if [ -n "$POD_TENSORD" ]; then
  step "the candidate TensorD on $M"
  cozy --json machine update "$M" --tensord "$POD_TENSORD" -y > "$OUT/update-rental.json" 2>&1 \
    || fail "machine update $M --tensord $POD_TENSORD: $(tail -c 400 "$OUT/update-rental.json")"
  echo "$M runs cozy $(cozy --json machine show "$M" | jq -r .status.version)"
  # The rest of the set, as gate.sh puts it on: an update the candidate TensorD itself carries out (cl-841).
  if [ -n "$CAND_TFS$CAND_RT" ]; then
    step "the candidate TensorFS and Runtime on $M"
    was=$(cozy --json machine show "$M" | jq -c '.status | {version, runtime, tensorfs}')
    cozy --json machine update "$M" ${CAND_TFS:+--tensorfs "$CAND_TFS"} ${CAND_RT:+--runtime "$CAND_RT"} -y > "$OUT/update-rental-set.json" 2>&1 \
      || fail "machine update $M --tensorfs --runtime: $(tail -c 400 "$OUT/update-rental-set.json")"
    now=$(cozy --json machine show "$M" | jq -c '.status | {version, runtime, tensorfs}')
    jq -en --argjson a "$was" --argjson b "$now" '$a.version == $b.version and $b.runtime != null and $a != $b' > /dev/null \
      || check "machine update $M --tensorfs --runtime changed nothing, or its TensorD: $was then $now"
    echo "$M: $was then $now"
  fi
fi
# Synthetic publication checks run against the isolated Hub in cli-publication.

step "BiRefNet on $M"
watched install-birefnet package install "$(pinned birefnet)" --on "$M" \
  || check "package install $(pinned birefnet) --on $M failed: $(run_get "$OUT" install-birefnet '.reason.message' | head -c 300)"
cozy --json package ls --on "$M" > "$OUT/packages-rental.json" && grep -q birefnet "$OUT/packages-rental.json" || check "package ls --on $M: no birefnet"
# A request file beside its image, in a folder named with spaces and non-ASCII; its output lands in a project in another such folder.
far="$OUT/in dir é ü" landing="$OUT/out dir é ü"
mkdir -p "$far" && cp "$(picture dog1.jpg)" "$far/chien é.jpg" && printf 'image: chien é.jpg\n' > "$far/request é.yaml"
mkdir -p "$landing" && (cd "$landing" && cozy --json status > /dev/null) || check "project initialization '$landing' failed"
if RUN_PROJECT=$landing watched birefnet-yaml run "$(pinned birefnet remove_background)" --input "$far/request é.yaml" --on "$M"; then
  # Downloaded by its digest (cl-819), it is the file the project holds under that digest.
  out=$(run_output "$OUT" birefnet-yaml webp) sha=''
  [ ! -s "$out" ] || sha=$(sha256sum "$out" | cut -c1-64)
  [ -n "$sha" ] && ls "$landing/files/$sha".* > /dev/null 2>&1 \
    || check "birefnet-yaml: its output did not land in '$landing' (${out:-none}, sha256:${sha:-none}; $(ls "$landing/files" 2> /dev/null | tr '\n' ' '))"
  white_background "$out" "$far/chien é.jpg" || check "birefnet-yaml: not a background removal"
else check "birefnet-yaml: cozy run $(pinned birefnet remove_background) --input --on $M did not complete: $(run_get "$OUT" birefnet-yaml '.reason.message' | head -c 300)"; fi
dog=$(picture dog2.jpg)
if run_cell "$OUT" birefnet-asset "$(pinned birefnet remove_background)" --file "image=$dog" --on "$M"; then
  white_background "$(run_output "$OUT" birefnet-asset webp)" "$dog" || check "birefnet-asset: not a background removal"
  # Downloaded from the rental, its bytes are what the run's record says it made.
  got=$(sha256sum "$(run_output "$OUT" birefnet-asset webp)" | cut -c1-64) made=$(run_get "$OUT" birefnet-asset '[.output[].sha256] | join(" ")')
  [[ " $made " == *" sha256:$got "* ]] || check "birefnet-asset: downloaded from $M it is sha256:$got, and its record says ${made:-nothing}"
  echo "birefnet-asset: run $(cat "$OUT/birefnet-asset.number") on $M, output $made, downloaded sha256:$got"
else check "birefnet-asset: cozy run --file --on $M did not complete: $(run_get "$OUT" birefnet-asset '.reason.message' | head -c 300)"; fi
cozy --json run ls --on "$M" > "$OUT/runs-rental.json" && J --arg m "$M" '(.runs | length > 0) and all(.runs[]; .machine == $m)' "$OUT/runs-rental.json" \
  || check "run ls --on $M: other machines' runs, or none"

step "SDXL on $M: a LoRA, then the package's other function"
# The executor protocol (xs-081): a LoRA reaches the executor as an adapter view; a second function is served by the same release.
if run_cell "$OUT" sdxl-lora "$(pinned sdxl generate)" prompt="a red armchair as flat-pack furniture, catalog photo" seed:=$RANDOM \
    --lora ostris/ikea-lora-sdxl --on "$M"; then
  [ -s "$(run_output "$OUT" sdxl-lora 'png|webp|jpg')" ] || check "sdxl-lora: no image came down"
  grep -q 'ikea-lora-sdxl' "$OUT/sdxl-lora.json" || check "sdxl-lora: its record names no LoRA"
else check "sdxl-lora: cozy run $(pinned sdxl generate) --lora --on $M did not complete: $(run_get "$OUT" sdxl-lora '.reason.message' | head -c 300)"; fi
if run_cell "$OUT" sdxl-image "$(pinned sdxl image)" instructions="a lighthouse at dusk, oil painting" seed:=$RANDOM --on "$M"; then
  [ -s "$(run_output "$OUT" sdxl-image 'png|webp|jpg')" ] || check "sdxl-image: no image came down"
else check "sdxl-image: cozy run $(pinned sdxl image) --on $M did not complete: $(run_get "$OUT" sdxl-image '.reason.message' | head -c 300)"; fi

step "a run on $M followed, then canceled"
deadline=$(jq '.idle_deadline_ms // 0' "$OUT/rental.json") || true
cozy --json machine keepalive "$M" > "$OUT/keepalive.json" || check "machine keepalive failed"
# A machine with work has no idle deadline to answer; one it answers is later than the one before.
J --argjson d "${deadline:-0}" '.acknowledged_at_ms > 0 and (.idle_deadline_ms == null or .idle_deadline_ms > $d)' "$OUT/keepalive.json" \
  || check "machine keepalive: not acknowledged, or its idle deadline did not move: $(head -c 300 "$OUT/keepalive.json")"
(cd "$here/../cli" && watched on-rental run ./weightless/reverse text=rented --on "$M") && grep -q detner "$OUT/on-rental.ndjson" \
  || check "a run on $M: no reply"
n=$(cd "$here/../cli" && run_submit "$OUT" cancel-wait run ./weightless/wait seconds:=300 --on "$M")
if [ -z "$n" ] || ! run_wait "$OUT" cancel-wait in_progress; then check "the waiting run on $M never ran"; else
  cozy --json run watch "$n" > "$OUT/cancel-followed.ndjson" &
  following=$!
  cozy --json run cancel "$n" > "$OUT/cancel.json" || check "run cancel $n failed"
  wait "$following" || true
  grep -qE '"status": *"canceled"' "$OUT/cancel-followed.ndjson" || check "run watch $n did not follow it to canceled"
fi
run_wait "$OUT" cancel-wait || true

step "an end while a run goes"
(cd "$RUN_PROJECT" && cozy --json run "$here/../cli/weightless/wait" seconds:=600 --on "$M" > "$OUT/rental-wait.ndjson" 2>&1) &
going=$!
until run_reached "$OUT/rental-wait.ndjson" in_progress || ! kill -0 "$going" 2> /dev/null; do sleep 0.2; done
# `machine end` returns once the Hub affirms the pod gone.
ended=$M
cozy --json machine end "$M" --force -y > "$OUT/end.json" || check "machine end $M --force failed"
! listed "$M" && M='' || check "machine end --force: $M is still listed"
# Its follower ends with the run (machine_lost); a bare `wait` would wait on --release's loopback server too.
wait "$going" || true

step "the rental in its history"
# Ended, it stays in the Hub's history with what it cost (cl-693).
cozy --json machine ls --since 1h > "$OUT/history.json" || check "machine ls --since 1h failed"
J --arg m "$ended" '.rentals | any(.name == $m and .ended_at_ms != null and .spend_usd_micros >= 0)' "$OUT/history.json" \
  || check "machine ls --since 1h: $ended is not in its history as ended"
cozy machine ls --history --since 1h > "$OUT/history.txt" && grep -q "^Total since" "$OUT/history.txt" \
  || check "machine ls --history --since 1h printed no total"

step coverage; coverage "$COZY" release
finish "cli-release: ${SKU} at \$$(usd "$hourly")/h, \$$(spent) of GPU time"
