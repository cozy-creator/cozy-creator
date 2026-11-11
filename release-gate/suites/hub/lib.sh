# release-gate/suites/hub/lib.sh: what the Hub's suites share (`hub`: production after a Hub deploy; `skew`: a
# candidate Hub before one). Sourced by their cells.sh under gate.sh or hubgate.sh, which give `expect`, `log`, $CLI,
# $M (where package and model cells run), $R, $OUT, $HERE and e2e/run.sh's run_*. The cells.sh sets $HB (its folder)
# and $HUB_URL first. Every cozy call goes to $HUB_URL: exported as COZY_HUB_URL, the CLI names it to Calcifer and the
# machine on each call.
export COZY_HUB_URL=$HUB_URL
. "$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../e2e" && pwd)/test-hub.sh"
# A gate on production (gate.sh --hub prod) reads it and rents there; every synthetic write still needs the fixture (hub_test_target).
[ "${HUB_TARGET:-}" = prod ] || require_ci_test_hub "$HUB_URL" || return 2
hub_test_target() {
  if ! require_test_hub "$HUB_URL"; then
    expect "hub isolation" FAIL "synthetic checks require the isolated fixture Hub"
    return 2
  fi
  if [ "${COZY_HUB_URL:-}" != "$HUB_URL" ] && ! require_test_hub "${COZY_HUB_URL:-}"; then
    expect "hub isolation" FAIL "the CLI target differs from the isolated fixture Hub"
    return 2
  fi
}
HUB_PKG= HUB_CREATED= HUB_MODELS= HUB_LIVE= HUB_RENTED= WHO=
mkdir -p "$HB"
# hub_5xx <file>: the Hub answered a server failure, which no cell accepts, as a refusal or otherwise.
hub_5xx() { grep -qE '"status": ?5[0-9][0-9]|HTTP 5[0-9][0-9]|: 5[0-9][0-9]; |unexpected internal failure|"code": ?"(internal|hub_answer)"' "$1"; }
# hub_do <check> ok|refused <cozy args...>: PASS (returns 0) when the command exits as wanted and the Hub answered no 5xx.
hub_do() {
  local check=$1 want=$2 code; shift 2
  case "$1 ${2:-}" in
    'package publish'|'package yank'|'package delete'|'model upload'|'model publish'|'model edit'|'model delete'|'lora upload'|'lora publish'|'lora edit'|'lora delete'|'org claim')
      hub_test_target || { expect "hub $check" FAIL "synthetic mutation requires the isolated fixture Hub"; return 2; } ;;
  esac
  "$CLI" --json "$@" >"$HB/$check.out" 2>&1; code=$?
  if hub_5xx "$HB/$check.out"; then
    expect "hub $check" FAIL "cozy $* → exit $code, a server failure: $(tail -c 300 "$HB/$check.out")"
    return 1
  elif { [ "$want" = ok ] && [ "$code" = 0 ]; } || { [ "$want" = refused ] && [ "$code" != 0 ]; }; then
    expect "hub $check" PASS "cozy $* → exit $code"
  else
    expect "hub $check" FAIL "cozy $* → exit $code (wanted $want): $(tail -c 300 "$HB/$check.out")"
    return 1
  fi
}

# This suite also runs released clients in skew tests. The product exposes only uninstall;
# an older client still needs its original spelling to exercise the same wire operation.
hub_uninstall() {
  local check=$1 verb=uninstall; shift
  "$CLI" package uninstall --help > /dev/null 2>&1 || verb=remove
  hub_do "$check" ok package "$verb" "$@"
}
# hub_runs <check> <release> <package> [pin] [machine]: `cozy run <package>/echo[@pin]` on the machine ($M) answers with that release.
hub_runs() {
  local check=$1 want=$2 on=${5:-$M} n got
  run_cell "$HB" "$check" "$3/echo${4:+@$4}" text=hub --on "$on"
  n=$(cat "$HB/$check.number") got=$(run_get "$HB" "$check" .result.release)
  [ "$got" = "$want" ] && expect "hub $check" PASS "run $n of $3${4:+@$4} on $on served $got" ||
    expect "hub $check" FAIL "run $n of $3${4:+@$4} on $on served '${got}', not $want: $(tail -c 300 "$HB/$check.err")"
}
# hub_release <version> [publish flag...]: the throwaway package at that version, published from its own directory.
hub_release() {
  hub_test_target || return 2
  HUB_CREATED=1
  local dir=$HB/pkg-$1
  rm -rf "$dir" && cp -r "$HERE/suites/hub/pkg" "$dir"
  sed -i "s/^name = \"ci-hub-pkg\"/name = \"$HUB_PKG\"/; s/^version = \"0.1.0\"/version = \"$1\"/" "$dir/pyproject.toml"
  sed -i "s/^RELEASE = \"0.1.0\"/RELEASE = \"$1\"/" "$dir/ci_hub_pkg/__init__.py"
  (cd "$dir" && hub_do "package-publish-$1" ok package publish -y "${@:2}" && test -s uv.lock)
}
# hub_candidate <version>: published yanked while the Hub's newest is read throughout: no read names it (cl-732).
hub_candidate() {
  hub_test_target || return 2
  local seen=$HB/package-candidate-newest.txt poller
  : >"$seen"
  ( while :; do package_details "$WHO/$HUB_PKG" 2>/dev/null |
      python3 -I -c 'import json,sys; print(json.load(sys.stdin)["latest_release"])' >>"$seen" 2>/dev/null; sleep 0.2; done ) &
  poller=$!
  hub_release "$1" --yanked && HUB_LIVE="$HUB_LIVE $1"
  kill "$poller" 2>/dev/null; wait "$poller" 2>/dev/null
  local reads named
  reads=$(wc -l <"$seen") named=$(grep -cxF "$1" "$seen")
  [ "$reads" -gt 0 ] && [ "$named" = 0 ] && expect "hub package-candidate-never-newest" PASS "$reads reads of the Hub's newest during the publish, none named $1" ||
    expect "hub package-candidate-never-newest" FAIL "$reads reads of the Hub's newest during the publish, $named named $1"
}
# hub_listed <check> <model>: the model metadata API lists the model's release 0.1.0 with its one lane, f16.
hub_listed() {
  model_details "$2" >"$HB/$1.out" 2>&1
  python3 -I -c 'import json,sys; r=[x for x in json.load(open(sys.argv[1])).get("releases") or [] if x.get("version")=="0.1.0"]; sys.exit(0 if r and [l["lane"] for l in r[0]["lanes"]]==["f16"] else 1)' "$HB/$1.out" &&
    expect "hub $1" PASS "model metadata lists 0.1.0 with lane f16" ||
    expect "hub $1" FAIL "model metadata does not list 0.1.0 with lane f16: $(tail -c 300 "$HB/$1.out")"
}
# hub_weights <dir>: 8 MiB of new random weights as a CozyTensors folder (new blobs, so an upload sends every one).
hub_weights() {
  local f=$1.safetensors store=$HB/tfs-store
  uv run -q --no-project --with numpy --with safetensors python -I - "$f" <<'PY'
import sys
import numpy as np
from safetensors.numpy import save_file
save_file({f"w.{i}": np.random.default_rng().standard_normal((1024, 1024)).astype(np.float16) for i in range(4)}, sys.argv[1])
PY
  rm -rf "$1" && TENSORFS_HOME=$store uv tool run -q --from "${TWF:-tensorfs}" tfs import "$f" --converter as-is --name "local/$(basename "$1")" >/dev/null &&
    TENSORFS_HOME=$store uv tool run -q --from "${TWF:-tensorfs}" tfs export "local/$(basename "$1")" "$1" >/dev/null
}
# hub_machines: the names `cozy machine ls` shows, one per line.
hub_machines() {
  "$CLI" --json machine ls 2>/dev/null | python3 -c 'import json,sys
def walk(o):
    if isinstance(o, dict):
        if isinstance(o.get("name"), str) and ("rental" in o or "kind" in o): print(o["name"])
        for v in o.values(): walk(v)
    elif isinstance(o, list):
        for v in o: walk(v)
walk(json.load(sys.stdin))' 2>/dev/null | sort -u
}
hub_cleanup() {
  local v failed=0
  if [ -n "$HUB_CREATED$HUB_MODELS" ]; then
    if hub_test_target; then
      if [ -n "$HUB_CREATED" ]; then
        if python3 "$HERE/../e2e/delete-test-package.py" "$WHO/$HUB_PKG" >>"$HB/cleanup.log" 2>&1; then
          HUB_CREATED= HUB_LIVE=
        else
          expect "hub package-cleanup" FAIL "could not remove the isolated test package; see cleanup.log"
          failed=1
        fi
      fi
      for v in $HUB_MODELS; do
        if "$CLI" model delete "$v" -y >>"$HB/cleanup.log" 2>&1; then HUB_MODELS=${HUB_MODELS/ $v/}; else
          expect "hub model-cleanup" FAIL "could not remove isolated model $v; see cleanup.log"
          failed=1
        fi
      done
    else
      expect "hub cleanup" FAIL "refusing synthetic cleanup outside the isolated fixture Hub"
      failed=1
    fi
  fi
  # Rentals remain explicitly owned by this run even when a fixture cleanup failed.
  for v in $HUB_RENTED; do
    if "$CLI" machine end "$v" -y >>"$HB/cleanup.log" 2>&1; then HUB_RENTED=${HUB_RENTED/ $v/}; else
      expect "hub rental-cleanup" FAIL "could not end owned rental $v; see cleanup.log"
      failed=1
    fi
  done
  return "$failed"
}

# hub_contract [paths|paths-empty]: each path no verb reaches, called anonymously through e2e/hubcheck (contract.py),
# answers as its contract says, and every request and response is as the document the Hub serves has it; mismatches are
# FAILs. paths-empty: a Hub without production's data, where a named package, model or SKU may be absent (404).
hub_contract() {
  local hc=''
  local production=()
  [ "${HUB_TARGET:-}" != prod ] || production=(--production)
  go build -C "$HERE/../e2e/hubcheck" -o "$HB/hubcheck" . 2> "$HB/hubcheck.log" && { "$HB/hubcheck" --hub "$HUB_URL" "${production[@]}" --out "$HB" > "$HB/hubcheck.addr" 2>> "$HB/hubcheck.log" & hc=$!; }
  until [ -z "$hc" ] || [ -s "$HB/hubcheck.addr" ] || ! kill -0 "$hc" 2> /dev/null; do sleep 1; done
  if [ -s "$HB/hubcheck.addr" ]; then
    while IFS=$'\t' read -r check verdict what; do expect "$check" "$verdict" "$what"; done < <(uv run -q --no-project \
      --with pyyaml python -I "$HERE/suites/hub/contract.py" "http://$(head -1 "$HB/hubcheck.addr")" "${1:-paths}")
    kill "$hc"
    if [ -s "$HB/hub-mismatches.jsonl" ]; then
      while read -r what; do expect "hub contract document" FAIL "$what"; done < <(jq -r \
        '"\(.side) \(.method) \(.route // .path) \(.status // "") \(.field // ""): \(.message)"' "$HB/hub-mismatches.jsonl" | sort -u)
    else
      expect "hub contract document" PASS "every request and response as the Hub's document has it: $(jq -c . "$HB/hub-calls.json")"
    fi
  else
    [ -z "$hc" ] || kill "$hc" 2> /dev/null
    expect "hub contract document" FAIL "hubcheck did not start: $(tail -c 300 "$HB/hubcheck.log")"
  fi
}

# hub_account: whoami (the account, its balance, the buy page). Sets WHO and the throwaway package's name.
hub_account() {
  local _ bal
  for _ in 1 2 3; do  # a dropped connection is asked again
    WHO=$("$CLI" --json whoami 2>"$HB/whoami.err" | python3 -c 'import json,sys; print(json.load(sys.stdin)["account"])' 2>/dev/null) && [ -n "$WHO" ] && break
    sleep 5
  done
  [ -n "$WHO" ] && expect "hub whoami" PASS "signed in to $HUB_URL as $WHO" || expect "hub whoami" FAIL "$HUB_URL: $(tail -c 200 "$HB/whoami.err")"
  bal=$("$CLI" --json whoami 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["billing"], d["buy_credits"])' 2>/dev/null)
  [ -n "$bal" ] && expect "hub balance" PASS "whoami: $bal" || expect "hub balance" FAIL "whoami names no balance or buy page"
  HUB_PKG=ci-hub-$R
}

# hub_packages: lock and publish 0.1.0 and 0.1.1; the unversioned run takes 0.1.1; yank 0.1.1 → the simple index marks it
# data-yanked, a fresh install takes 0.1.0, its exact pin still installs; un-yank → 0.1.1 again; publish 0.1.2 --yanked →
# the Hub's newest never names it, read throughout, the publisher's next unversioned run keeps 0.1.1, and its pin installs.
hub_publish() {
  hub_test_target || return 2
  hub_release 0.1.0 && HUB_LIVE="0.1.0"
  hub_release 0.1.1 && HUB_LIVE="0.1.0 0.1.1"
  package_details "$WHO/$HUB_PKG" > "$HB/package-detail.out" || expect "hub package detail" FAIL "package metadata is unavailable"
  hub_do package-ls ok package ls "$WHO/$HUB_PKG"
}
hub_packages() {
  hub_test_target || return 2
  hub_runs package-newest 0.1.1 "$WHO/$HUB_PKG"
  hub_do package-yank ok package yank "$WHO/$HUB_PKG@0.1.1"
  while IFS=$'\t' read -r check verdict what; do expect "$check" "$verdict" "$what"; done < <(uv run -q --no-project \
    --with pyyaml python -I "$HERE/suites/hub/contract.py" "$HUB_URL" yanked "$WHO" "$HUB_PKG" 0.1.1)
  hub_uninstall package-remove "$WHO/$HUB_PKG" --on "$M"
  hub_runs package-skips-yanked 0.1.0 "$WHO/$HUB_PKG"
  hub_runs package-pinned-yanked 0.1.1 "$WHO/$HUB_PKG" 0.1.1
  hub_do package-unyank ok package yank --undo "$WHO/$HUB_PKG@0.1.1"
  hub_uninstall package-remove-again "$WHO/$HUB_PKG" --on "$M"
  hub_runs package-unyanked 0.1.1 "$WHO/$HUB_PKG"
  hub_candidate 0.1.2
  hub_runs package-candidate-skipped 0.1.1 "$WHO/$HUB_PKG"
  hub_runs package-candidate-pinned 0.1.2 "$WHO/$HUB_PKG" 0.1.2
}

# hub_model_upload <check> <machine> <model>: new weights uploaded by that machine (its run capability: the workload
# writer's path, as every machine and pod uploads) as a commit in lane f16, which it sets in HUB_COMMIT.
hub_model_upload() {
  hub_test_target || return 2
  local w=$HB/w-$1
  HUB_COMMIT=
  hub_weights "$w" || { expect "hub $1" FAIL "no weights to upload: tfs import/export failed"; return 1; }
  HUB_MODELS="$HUB_MODELS $3"
  hub_do "$1" ok model upload "$w" "$3" --lane f16 --on "$2" -y || return 1
  HUB_COMMIT=$(model_details "$3" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["checkpoints"][0]["checkpoint"])' 2>/dev/null)
  [ -n "$HUB_COMMIT" ] || { expect "hub $1-commit" FAIL "model metadata for $3 names no commit after the upload"; return 1; }
}
# Released clients use download; the current CLI shares package install terminology.
hub_model_install() {
  local check=$1 verb=install; shift
  "$CLI" model install --help > /dev/null 2>&1 || verb=download
  hub_do "$check" ok model "$verb" "$@"
}

# hub_unreleased <model>: xs-173, the owner's model with no release named by name on $M: a cozy that signs model reads
# (0.25.0 on, or a dev build) at a Hub that grants them installs its newest commit; an older cozy, or a Hub before them,
# is refused, typed (hub_do fails a 5xx).
hub_unreleased() {
  local want=refused verb=install version base
  version=$("$CLI" --version | awk '{print $2}'); base=${version%%[-+]*}
  if { [[ $version == *+dev* ]] || [ "$(printf '%s\n0.25.0\n' "$base" | sort -V | head -1)" = 0.25.0 ]; } &&
    curl -fsS "$HUB_URL/.well-known/oauth-protected-resource" | grep -q '"tensorhub_model_read"'; then want=ok; fi
  "$CLI" model install --help > /dev/null 2>&1 || verb=download
  hub_do model-unreleased "$want" model "$verb" "$1" --on "$M" -y
}
# hub_models: on $M, upload a commit (the workload writer), publish it as 0.1.0 (listed), download; the exact pin and
# lane download; delete → gone, as the Hub serves the model and as the model metadata API shows it.
hub_models() {
  hub_test_target || return 2
  local model=$WHO/$HUB_PKG
  hub_model_upload model-upload "$M" "$model" || return 0
  hub_unreleased "$model"
  hub_do model-publish ok model publish "$model" 0.1.0 "$HUB_COMMIT"
  hub_listed model-info-published "$model"
  hub_model_install model-download "$model" --on "$M" -y
  hub_model_install model-pinned "$model@0.1.0" --lane f16 --on "$M" -y
  hub_do model-delete ok model delete "$model" -y && HUB_MODELS=${HUB_MODELS/ $model/}
  if model_details "$model" > "$HB/model-gone.out" 2>&1; then
    expect "hub model-gone" FAIL "deleted model still answers"
  elif hub_5xx "$HB/model-gone.out"; then
    expect "hub model-gone" FAIL "model lookup returned a server failure"
  else
    expect "hub model-gone" PASS "deleted model is absent"
  fi
}

# hub_dev: `dev = true` in this home's cozy config, which turns on dev-only options (cl-844: hidden from --help,
# refused without it). Never a --help probe: a cozy that refuses a dev option fails the cell that needed it.
hub_dev() {
  local cfg=$HOME/.cozy/config.toml
  grep -qxE 'dev *= *true' "$cfg" 2>/dev/null && return
  # Top level: before any [table]. With no config.toml yet (a CI runner signed in by key has none), `cat` failing must not stop the write.
  if mkdir -p "${cfg%/*}" && { echo 'dev = true'; [ ! -f "$cfg" ] || cat "$cfg"; } >"$cfg.new" && mv "$cfg.new" "$cfg"; then
    log "dev = true written into $cfg"
  else
    log "could not write dev = true into $cfg"
  fi
}

# hub_pod_secrets <where> <machine>: xs-157 on a fresh rental, over `cozy machine ssh` with the throwaway key.
#   pod-env   the names (never values) in every process's environment on the pod, pid 1's (what the provider launched it
#             with) among them: none is a Hub secret, and the public names are there (COZY_WORKER_ID), so the read is
#             real. RunPod's own RUNPOD_API_KEY is in every pod and cannot be removed; it is named in the detail.
#   pod-auth  the machine authenticated to the Hub with its leaf key alone: the rental reached ready (the Hub makes it
#             ready only once its pinned machine has read its lease, signed), and the lease it read is kept (0600) and
#             names a key, so the renter is admitted.
hub_pod_secrets() {
  local p=$1 m=$2 names='' lease='' agent forbidden found SSH_AUTH_SOCK SSH_AGENT_PID  # the agent is this cell's alone
  agent=$(ssh-agent -s) && eval "$agent" >/dev/null && ssh-add -q "$HB/pod-ssh" 2>/dev/null
  for _ in 1 2 3 4 5 6; do  # a pod's sshd may start a few seconds after the machine is ready
    # One argument: ssh hands the remote shell one command line.
    names=$("$CLI" machine ssh "$m" -- 'for f in /proc/[0-9]*/environ; do tr "\0" "\n" <"$f"; echo; done 2>/dev/null | cut -d= -f1 | sort -u' 2>"$HB/pod-env-$p.err") && [ -n "$names" ] && break
    sleep 10
  done
  lease=$("$CLI" machine ssh "$m" -- 'stat -c %a /var/lib/cozy/machine/lease.json && cat /var/lib/cozy/machine/lease.json' 2>>"$HB/pod-env-$p.err")
  ssh-agent -k >/dev/null 2>&1
  echo "$names" >"$HB/pod-env-$p.names"
  forbidden='COZY_WORKER_AUTH_TOKEN COZY_BOOTSTRAP_RECEIPT_HMAC_KEY_B64URL COZY_BOOTSTRAP_RECEIPT_HMAC_KEY_FILE COZY_AUTHORIZED_KEYS'
  found=$(for n in $forbidden; do grep -qxF "$n" <<<"$names" && echo "$n"; done | tr '\n' ' ')
  if [ -z "$names" ] || ! grep -qxF COZY_WORKER_ID <<<"$names"; then
    expect "hub pod-env on $p" FAIL "could not read $m's environment: $(tail -c 300 "$HB/pod-env-$p.err")"
  elif [ -n "$found" ]; then
    expect "hub pod-env on $p" FAIL "$m's environment names Hub secrets: $found"
  else
    expect "hub pod-env on $p" PASS "$(wc -l <<<"$names") names on $m, no Hub secret; COZY_/TENSORHUB_: $(grep -E '^(COZY|TENSORHUB)_' <<<"$names" | tr '\n' ' ')$(grep -qxF RUNPOD_API_KEY <<<"$names" && echo '; RUNPOD_API_KEY is RunPod'"'"'s own')"
  fi
  if [ "$(head -1 <<<"$lease")" = 600 ] && tail -n +2 <<<"$lease" | python3 -I -c 'import json,sys; sys.exit(0 if json.load(sys.stdin)["keys"] else 1)' 2>/dev/null; then
    expect "hub pod-auth on $p" PASS "$m ready, its signed lease read kept (0600) with $(tail -n +2 <<<"$lease" | python3 -I -c 'import json,sys; print(len(json.load(sys.stdin)["keys"]))') key(s)"
  else
    expect "hub pod-auth on $p" FAIL "$m's kept lease is missing, not 0600 or names no key: $(head -c 300 <<<"$lease") $(tail -c 200 "$HB/pod-env-$p.err")"
  fi
}
# hub_pod <provider|any>: the real-pod cells on one fresh `cpu` rental with a throwaway SSH key: rented (`--provider`, a
# dev-only option, unless `any`, the Hub's choice), a local weightless package run there by content, pod-env and pod-auth (hub_pod_secrets), ended and checked gone.
# Synthetic package publication and model uploads run separately against the isolated fixture Hub.
# Only a machine the rent reply names is ended (hub_cleanup ends any left); returns 2 when this cozy refuses the
# provider option, having failed the cell.
hub_pod() {
  local p=$1 m before prov=()
  [ "$p" = any ] || prov=(--provider "$p")
  before=$(hub_machines)
  [ -s "$HB/pod-ssh.pub" ] || ssh-keygen -q -t ed25519 -N '' -C ci-hub-pod -f "$HB/pod-ssh"
  # A provider with no `cpu` right now answers rental.sku_out_of_stock: ask again each minute, 3 at most, then take its cheapest
  # GPU of HUB_GPU_FALLBACK in turn (vast went without `cpu` for hours on 11-10, and xs-157's proof must still reach it).
  local code sku
  for sku in cpu ${HUB_GPU_FALLBACK:-rtx-3060 rtx-4060-ti rtx-3070 rtx-a4000 rtx-3080 rtx-3090}; do
    for _ in $( [ "$sku" = cpu ] && seq 3 || echo 1 ); do
      "$CLI" --json machine rent "$sku" "${prov[@]}" --ssh "$HB/pod-ssh.pub" -y >"$HB/rent-$p.out" 2>&1; code=$?
      grep -qE '"rental.sku_out_of_stock"|no provider inventory' "$HB/rent-$p.out" || break 2
      [ "$sku" != cpu ] || { log "hub rent on $p: no cpu inventory on $p right now; asking again in 60 s"; sleep 60; }
    done
    log "hub rent on $p: no $sku there; trying the next"
  done
  echo "$sku" > "$HB/rent-$p.sku"
  if grep -qE "unexpected argument '--provider'|dev_only" "$HB/rent-$p.out"; then
    expect "hub rent on $p" FAIL "this cozy refuses --provider with dev = true (it needs cl-844's switch): $(tail -c 200 "$HB/rent-$p.out"); $(command -v "$CLI") $("$CLI" --version 2>&1) read $HOME/.cozy/config.toml, which begins: $(head -c 120 "$HOME/.cozy/config.toml" 2>&1 | tr '\n' '|')"
    return 2
  fi
  m=$(python3 -c 'import json,sys
t = open(sys.argv[1]).read(); dec = json.JSONDecoder(); i = 0
while (i := t.find("{", i)) >= 0:
    d, i = dec.raw_decode(t, i)
    if isinstance(d.get("machine"), dict) and d["machine"].get("name"): print(d["machine"]["name"]); break' "$HB/rent-$p.out" 2>/dev/null)
  if [ "$code" != 0 ] && ! grep -q '"machine"' "$HB/rent-$p.out"; then
    expect "hub rent on $p" FAIL "cozy machine rent $sku ${prov[*]} → exit $code: $(grep -o '"code":"[^"]*","message":"[^"]*"' "$HB/rent-$p.out" | tail -1 || tail -c 300 "$HB/rent-$p.out")"
    return
  fi
  # Only the machine the reply names is ours: another session may rent at the same moment, and its pod is never ended here.
  [ -n "$m" ] || { expect "hub rent on $p" FAIL "the reply named no machine (see cozy machine ls; new since: $(comm -13 <(echo "$before") <(hub_machines) | paste -sd' ')): $(tail -c 300 "$HB/rent-$p.out")"; return; }
  HUB_RENTED="$HUB_RENTED $m"
  echo "$p $m $(date -u +%FT%TZ)" >>"$HB/rented.txt"
  if grep -q '"machine"' "$HB/rent-$p.out"; then
    expect "hub rent on $p" PASS "rented $m: $("$CLI" --json machine show "$m" 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); r=d.get("rental") or {}; print(r.get("sku"), r.get("provider") or d.get("provider"), r.get("datacenter") or d.get("datacenter"))' 2>/dev/null)"
  else
    expect "hub rent on $p" FAIL "rented $m on $p, but the reply named no machine: $(tail -c 200 "$HB/rent-$p.out")"
  fi
  # Source travels by content; provider qualification never creates a Hub repository.
  hub_runs "run-on-$p" 0.1.0 "$HERE/suites/hub/pkg" '' "$m"
  hub_pod_secrets "$p" "$m"
  "$CLI" machine end "$m" -y >"$HB/end-$p.out" 2>&1
  for _ in $(seq 30); do
    hub_machines | grep -qxF "$m" || { expect "hub end on $p" PASS "$m ended and gone"; HUB_RENTED=${HUB_RENTED/ $m/}; return; }
    sleep 10
  done
  expect "hub end on $p" FAIL "$m still listed 5 min after cozy machine end: $(tail -c 200 "$HB/end-$p.out")"
}
