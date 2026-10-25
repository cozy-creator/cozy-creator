#!/usr/bin/env bash
# e2e/capabilities/run.sh: a run capability reaches what its runs need and nothing else (cl-670). It runs no run: it
# speaks the Hub's API with this computer's signed-in cozy, so nothing lands in a run history.
#
#   e2e/capabilities/run.sh --out DIR --commit ORG/NAME@sha256:<hex> [--hub ORIGIN]
#
# ORIGIN is the Hub cozy is signed in to (default $COZY_HUB_URL, else https://tensorhub.com); the commit is a private
# one of the signed-in account (the release suite's throwaway upload). Checks, each against the Hub's own answer:
# - a capability reads its commit's blobs, which an anonymous read cannot;
# - it is refused for another repository, for a release, for a delete, for rentals and for minting another;
# - the account cannot mint one for another account's repository;
# - once it expires (as it does after its runs end, when nothing mints it again) and when forged, it reads nothing.
set -euo pipefail
OUT='' HUB=${COZY_HUB_URL:-https://tensorhub.com} COMMIT=''
while [ $# -gt 0 ]; do
  case $1 in
    --out) OUT=$2 ;;
    --hub) HUB=${2%/} ;;
    --commit) COMMIT=$2 ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
  shift 2
done
[ -n "$OUT" ] && [ -n "$HUB" ] && [[ $COMMIT =~ ^([a-z0-9_.-]+)/([a-z0-9_.-]+)@sha256:([0-9a-f]{64})$ ]] \
  || { echo "usage: $0 --out DIR --commit ORG/NAME@sha256:<hex> [--hub ORIGIN]" >&2; exit 2; }
ORG=${BASH_REMATCH[1]} NAME=${BASH_REMATCH[2]} SHA=${BASH_REMATCH[3]}
mkdir -p "$OUT"
failed=0
fail() { echo "FAIL $*"; failed=1; }
pass() { echo "ok   $*"; }

# This computer's session at the Hub, minted fresh by cozy itself.
COZY_HUB_URL=$HUB cozy --json whoami > "$OUT/whoami.json"
ACCOUNT=$(jq -r .account "$OUT/whoami.json")
BEARER=$(jq -r .token "$HOME/.cozy/keys/device-$(printf %s "$HUB" | sha256sum | cut -c1-64).bearer.json")

# ask NAME METHOD PATH [TOKEN] [BODY]: the status, the body kept as $OUT/NAME.json.
ask() {
  local how=(-X "$2")
  [ "$2" != HEAD ] || how=(--head)
  [ -z "${4:-}" ] || how+=(-H "authorization: Bearer $4")
  [ -z "${5:-}" ] || how+=(-H 'content-type: application/json' --data "$5")
  curl -sS -o "$OUT/$1.json" -w '%{http_code}' "${how[@]}" "$HUB$3"
}
code() { jq -r '.error.code // empty' "$OUT/$1.json" 2> /dev/null; }
expect() { # expect NAME STATUS GOT [CODE]
  if [ "$3" = "$2" ] && { [ -z "${4:-}" ] || [ "$(code "$1")" = "$4" ]; }; then pass "$1: $3 ${4:-}"
  else fail "$1: want $2 ${4:-}, got $3 $(code "$1")"; fi
}
BLOB=/blobs/${SHA:0:2}/${SHA:2:2}/$SHA

status=$(ask mint POST /v1/capabilities "$BEARER" "{\"machine\":\"e2e\",\"reads\":[\"sha256:$SHA\"],\"writes\":[\"$ORG/$NAME\"]}")
expect mint 201 "$status"
CAP=$(jq -r .token "$OUT/mint.json")

expect anonymous-read 404 "$(ask anonymous-read GET "$BLOB")"
expect read 307 "$(ask read GET "$BLOB" "$CAP")"
expect head 200 "$(ask head HEAD "$BLOB" "$CAP")"
# A variable: macOS's bash 3.2 brace-expands the {..,..} of this body inside "$(...)", splitting it.
body="{\"commit\":{\"sha256\":\"$SHA\",\"length\":1}}"
expect other-repository 403 "$(ask other-repository POST "/v1/models/$ORG/$NAME-e2e-other/commits" "$CAP" "$body")" \
  capability.operation_forbidden
expect release 403 "$(ask release PUT "/v1/models/$ORG/$NAME/releases/999.0.0" "$CAP" \
  "{\"commits\":[\"sha256:$SHA\"]}")" capability.operation_forbidden
expect delete 403 "$(ask delete DELETE "/v1/models/$ORG/$NAME/commits/sha256:$SHA" "$CAP")" capability.operation_forbidden
expect rentals 403 "$(ask rentals GET /v1/rentals "$CAP")" capability.operation_forbidden
expect mint-with-capability 403 "$(ask mint-with-capability POST /v1/capabilities "$CAP" \
  "{\"reads\":[\"sha256:$SHA\"]}")" capability.operation_forbidden
expect foreign-repository 403 "$(ask foreign-repository POST /v1/capabilities "$BEARER" \
  "{\"writes\":[\"$ACCOUNT-e2e-not-yours/x\"]}")" capability.repository_forbidden

payload=${CAP#cap1.}
forged="cap1.$(printf %s "${payload%%.*}" | tr 'A-Za-z' 'B-ZAb-za').${CAP##*.}"
expect forged 401 "$(ask forged GET "$BLOB" "$forged")" unauthorized

status=$(ask short POST /v1/capabilities "$BEARER" "{\"reads\":[\"sha256:$SHA\"],\"expires_s\":2}")
expect short 201 "$status"
SHORT=$(jq -r .token "$OUT/short.json")
expect short-read 307 "$(ask short-read GET "$BLOB" "$SHORT")"
sleep 3
expect expired 401 "$(ask expired GET "$BLOB" "$SHORT")" unauthorized
jq -e '.error.message | test("expired")' "$OUT/expired.json" > /dev/null && pass "expired: says so" \
  || fail "expired: $(jq -c .error "$OUT/expired.json")"

[ $failed = 0 ] && echo "capabilities: every check passed" || { echo "capabilities: FAILED"; exit 1; }
