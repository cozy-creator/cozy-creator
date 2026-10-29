# Suite `hub` (gate.sh --suite hub): the Hub's API contract (tensorhub api/openapi.yaml, as the Hub serves it) as cozy
# uses it, on the rented machine $M and a throwaway <account>/ci-hub-<R> package and model that the suite retires at the
# end (the package's releases yanked, the model deleted):
#   paths            contract.py: each path no cozy verb reaches, called anonymously through e2e/hubcheck, answers as
#                    its contract says, and hubcheck finds every request and response as the document has it
#   account          whoami (the account, its balance, the buy page), the rental catalog, the rental $M itself
#   providers        an admin rental of the cheapest CPU SKU on each provider (RunPod, vast), ended and checked
#                    gone, so both acquisition paths are covered (renters never pick one: th-254)
#   package          lock and publish 0.1.0 and 0.1.1; the unversioned run takes 0.1.1; yank 0.1.1 → the simple index marks it
#                    data-yanked, a fresh install takes 0.1.0, its exact pin still installs; un-yank → 0.1.1 again
#   model            upload a commit in lane f16, publish it as 0.1.0 (listed), download; the exact pin and lane
#                    download; delete → gone
# Each is an `expect` row: a FAIL fails the gate. The migration rehearsal on a prod dump is the release agent's
# step before a Hub deploy (release-gate README).
HB=$OUT/suite-hub HUB_URL=${HUB_URL:-https://tensorhub.com} HUB_PKG= HUB_MODEL= HUB_LIVE= HUB_RENTED=
mkdir -p "$HB"
# hub_do <check> ok|refused <cozy args...>: PASS (returns 0) when the command exits as wanted; output in <check>.out.
hub_do() {
  local check=$1 want=$2 code; shift 2
  "$CLI" --json "$@" >"$HB/$check.out" 2>&1; code=$?
  if { [ "$want" = ok ] && [ "$code" = 0 ]; } || { [ "$want" = refused ] && [ "$code" != 0 ]; }; then
    expect "hub $check" PASS "cozy $* → exit $code"
  else
    expect "hub $check" FAIL "cozy $* → exit $code (wanted $want): $(tail -c 300 "$HB/$check.out")"
    return 1
  fi
}
# hub_runs <check> <release> <package> [pin]: `cozy run <package>/echo[@pin]` on $M (e2e/run.sh) answers with that release.
hub_runs() {
  local check=$1 want=$2 n got
  run_cell "$HB" "$check" "$3/echo${4:+@$4}" text=hub --on "$M"
  n=$(cat "$HB/$check.number") got=$(run_get "$HB" "$check" .result.release)
  [ "$got" = "$want" ] && expect "hub $check" PASS "run $n of $3${4:+@$4} served $got" ||
    expect "hub $check" FAIL "run $n of $3${4:+@$4} served '${got}', not $want: $(tail -c 300 "$HB/$check.err")"
}
# hub_release <version>: the throwaway package at that version, published from its own directory.
hub_release() {
  local dir=$HB/pkg-$1
  rm -rf "$dir" && cp -r "$HERE/suites/hub/pkg" "$dir"
  sed -i "s/^name = \"ci-hub-pkg\"/name = \"$HUB_PKG\"/; s/^version = \"0.1.0\"/version = \"$1\"/" "$dir/pyproject.toml"
  sed -i "s/^RELEASE = \"0.1.0\"/RELEASE = \"$1\"/" "$dir/ci_hub_pkg/__init__.py"
  (cd "$dir" && hub_do "package-lock-$1" ok package lock && hub_do "package-publish-$1" ok package publish -y)
}
# hub_listed <check>: `cozy model info` lists the model's release 0.1.0 with its one lane, f16.
hub_listed() {
  "$CLI" --json model info "$HUB_MODEL" >"$HB/$1.out" 2>&1
  python3 -I -c 'import json,sys; r=[x for x in json.load(open(sys.argv[1])).get("releases") or [] if x.get("version")=="0.1.0"]; sys.exit(0 if r and [l["lane"] for l in r[0]["lanes"]]==["f16"] else 1)' "$HB/$1.out" &&
    expect "hub $1" PASS "model info lists 0.1.0 with lane f16" ||
    expect "hub $1" FAIL "model info does not list 0.1.0 with lane f16: $(tail -c 300 "$HB/$1.out")"
}
hub_cleanup() {
  local v
  for v in $HUB_LIVE; do "$CLI" package yank "$WHO/$HUB_PKG@$v" -y >>"$HB/cleanup.log" 2>&1; done
  [ -z "$HUB_MODEL" ] || "$CLI" model delete "$HUB_MODEL" -y >>"$HB/cleanup.log" 2>&1
  for v in $HUB_RENTED; do "$CLI" machine end "$v" -y >>"$HB/cleanup.log" 2>&1; done
  HUB_LIVE= HUB_MODEL= HUB_RENTED=
}
trap 'hub_cleanup; declare -F tfs_cleanup >/dev/null && tfs_cleanup; end_it' EXIT

# paths: through hubcheck, which reads the document the Hub serves; its mismatches are the gate's FAILs.
hc=''
go build -C "$HERE/../e2e/hubcheck" -o "$HB/hubcheck" . 2> "$HB/hubcheck.log" && { "$HB/hubcheck" --hub "$HUB_URL" --out "$HB" > "$HB/hubcheck.addr" 2>> "$HB/hubcheck.log" & hc=$!; }
until [ -z "$hc" ] || [ -s "$HB/hubcheck.addr" ] || ! kill -0 "$hc" 2> /dev/null; do sleep 1; done
if [ -s "$HB/hubcheck.addr" ]; then
  while IFS=$'\t' read -r check verdict what; do expect "$check" "$verdict" "$what"; done < <(uv run -q --no-project \
    --with pyyaml python -I "$HERE/suites/hub/contract.py" "http://$(head -1 "$HB/hubcheck.addr")" paths)
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
# account
WHO=; for _ in 1 2 3; do  # a dropped connection is asked again
  WHO=$("$CLI" --json whoami 2>"$HB/whoami.err" | python3 -c 'import json,sys; print(json.load(sys.stdin)["account"])' 2>/dev/null) && [ -n "$WHO" ] && break
  sleep 5
done
[ -n "$WHO" ] && expect "hub whoami" PASS "signed in as $WHO" || expect "hub whoami" FAIL "$(tail -c 200 "$HB/whoami.err")"
BAL=$("$CLI" --json whoami 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["billing"], d["buy_credits"])' 2>/dev/null)
[ -n "$BAL" ] && expect "hub balance" PASS "whoami: $BAL" || expect "hub balance" FAIL "whoami names no balance or buy page"
hub_do catalog ok machine rent
# The rental itself is gate.sh's (rent, then end and check it is gone).
P=$("$CLI" --json machine show "$M" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["rental"]["sku"])' 2>/dev/null)
[ -n "$P" ] && expect "hub rental" PASS "$M is a $P rental" || expect "hub rental" FAIL "machine show $M names no rental"
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
# hub_rent_on <provider>: a rental of `cpu` there (`machine rent --provider`, in dev builds only; the Hub decides who may), ended and checked
# gone. A rental it cannot name from the reply is the one new in `machine ls`; hub_cleanup ends any left.
hub_rent_on() {
  local m before
  before=$(hub_machines)
  "$CLI" --json machine rent cpu --provider "$1" -y >"$HB/rent-$1.out" 2>&1
  m=$(python3 -c 'import json,sys
t = open(sys.argv[1]).read(); dec = json.JSONDecoder(); i = 0
while (i := t.find("{", i)) >= 0:
    d, i = dec.raw_decode(t, i)
    if isinstance(d.get("machine"), dict) and d["machine"].get("name"): print(d["machine"]["name"]); break' "$HB/rent-$1.out" 2>/dev/null)
  [ -n "$m" ] || m=$(comm -13 <(echo "$before") <(hub_machines) | grep -vxF "$M" | head -1)
  [ -n "$m" ] || { expect "hub rent on $1" FAIL "$(tail -c 300 "$HB/rent-$1.out")"; return; }
  HUB_RENTED="$HUB_RENTED $m"
  grep -q '"machine"' "$HB/rent-$1.out" && expect "hub rent on $1" PASS "rented $m on $1" ||
    expect "hub rent on $1" FAIL "rented $m on $1, but the reply named no machine: $(tail -c 200 "$HB/rent-$1.out")"
  "$CLI" machine end "$m" -y >"$HB/end-$1.out" 2>&1
  for _ in $(seq 30); do
    hub_machines | grep -qxF "$m" || { expect "hub end on $1" PASS "$m ended and gone"; HUB_RENTED=${HUB_RENTED/ $m/}; return; }
    sleep 10
  done
  expect "hub end on $1" FAIL "$m still listed 5 min after cozy machine end: $(tail -c 200 "$HB/end-$1.out")"
}
if "$CLI" machine rent --help 2>/dev/null | grep -q -- --provider || "$CLI" machine rent cpu --provider runpod --help >/dev/null 2>&1; then
  for p in runpod vast; do hub_rent_on "$p"; done
else
  expect "hub providers" FAIL "this cozy is not a dev build (cargo feature dev): it has no \`machine rent --provider\` to rent on each provider"
fi
[ -n "$WHO" ] || return 0
HUB_PKG=ci-hub-$R
# package
hub_release 0.1.0 && HUB_LIVE="0.1.0"
hub_release 0.1.1 && HUB_LIVE="0.1.0 0.1.1"
hub_do package-info ok package info "$WHO/$HUB_PKG"
hub_runs package-newest 0.1.1 "$WHO/$HUB_PKG"
hub_do package-yank ok package yank "$WHO/$HUB_PKG@0.1.1"
while IFS=$'\t' read -r check verdict what; do expect "$check" "$verdict" "$what"; done < <(uv run -q --no-project \
  --with pyyaml python -I "$HERE/suites/hub/contract.py" "$HUB_URL" yanked "$WHO" "$HUB_PKG" 0.1.1)
hub_do package-remove ok package remove "$WHO/$HUB_PKG" --on "$M"
hub_runs package-skips-yanked 0.1.0 "$WHO/$HUB_PKG"
hub_runs package-pinned-yanked 0.1.1 "$WHO/$HUB_PKG" 0.1.1
hub_do package-unyank ok package yank --undo "$WHO/$HUB_PKG@0.1.1"
hub_do package-remove-again ok package remove "$WHO/$HUB_PKG" --on "$M"
hub_runs package-unyanked 0.1.1 "$WHO/$HUB_PKG"
# model: 8 MiB of new random weights as a CozyTensors folder.
uv run -q --no-project --with numpy --with safetensors python -I - "$HB/w.safetensors" <<'PY'
import sys
import numpy as np
from safetensors.numpy import save_file
save_file({f"w.{i}": np.random.default_rng().standard_normal((1024, 1024)).astype(np.float16) for i in range(4)}, sys.argv[1])
PY
TENSORFS_HOME=$HB/tfs-store uv tool run -q --from "${TWF:-tensorfs}" tfs import "$HB/w.safetensors" --name local/ci-hub >/dev/null &&
  TENSORFS_HOME=$HB/tfs-store uv tool run -q --from "${TWF:-tensorfs}" tfs export local/ci-hub "$HB/w" >/dev/null
HUB_MODEL=$WHO/$HUB_PKG
hub_do model-upload ok model upload "$HB/w" "$HUB_MODEL" --lane f16 --on "$M" -y
HUB_COMMIT=$("$CLI" --json model info "$HUB_MODEL" 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["commits"][0]["commit"])' 2>/dev/null)
hub_do model-publish ok model publish "$HUB_MODEL" 0.1.0 "$HUB_COMMIT"
hub_listed model-info-published
hub_do model-download ok model download "$HUB_MODEL" --on "$M" -y
hub_do model-pinned ok model download "$HUB_MODEL@0.1.0" --lane f16 --on "$M" -y
hub_do model-delete ok model delete "$HUB_MODEL" -y && HUB_MODEL=
# Deleted: gone, as the Hub serves the model (GET /v1/models/{org}/{name}) and as `model info` shows it.
hub_do model-gone refused model info "$WHO/$HUB_PKG"
hub_cleanup
