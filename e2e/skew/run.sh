#!/usr/bin/env bash
# e2e/skew/run.sh --hub-src DIR (--client VERSION | --cozy-bin FILE) --out DIR [--checks FILE] [--cozy-min X.Y.Z]: a cozy against a Hub built from
# source, on a fresh Linux runner with Docker and Go (xs-166; e2e.yaml's `skew`, by hand on a computer that is yours to
# replace). The Hub serves on a throwaway Postgres and a MinIO built from its last source, whose SigV4 check is R2's: a
# key's reserved characters are percent-encoded in the canonical request, so v0.40.33-35's bare ':' in a workload's
# proof key fails here as it failed on R2. It runs as a development Hub (sign-in codes in its log): cozy dials it on
# loopback, and machines at its https public origin, an ingress under a fresh CA the Hub hands them (ingress.go; a Hub
# hands a CA only beside its docker provider, which rents nothing here).
# The client is installed by its own release's installer into this computer's default home, signed in with the code
# the Hub logs, and runs the Hub suite's cells (release-gate/suites/hub/lib.sh) on its local machine: account, the
# catalog, package publish, install and run, new weights uploaded by the machine (the workload writer, as pods
# upload), publish, download, delete. A client older than --cozy-min that names its version (`cozy/<version>`, from
# 0.23.0) must instead be refused at every step with the typed client_too_old naming `cozy update`. Any 5xx fails.
set -uo pipefail
E2E=$(cd "$(dirname "$0")/.." && pwd) HERE=$(cd "$(dirname "$0")/../../release-gate" && pwd)
SRC= CLIENT= BIN= CHECKS= OUT= MIN= RT= TF= PUBLICATION_LOG= CLIENT_READY=
while [ $# -gt 0 ]; do
  case $1 in
    --hub-src) SRC=$(cd "$2" && pwd) || exit 2;; --client) CLIENT=${2#v};; --cozy-bin) BIN=$(realpath "$2") || exit 2;; --checks) CHECKS=$(realpath "$2") || exit 2;; --out) OUT=$2;; --cozy-min) MIN=$2;;
    --runtime) RT=$2; if [ -f "$RT" ]; then RT=$(realpath "$RT"); fi;;
    --tensorfs) TF=$2; if [ -f "$TF" ]; then TF=$(realpath "$TF"); fi;;
    --publication-log) PUBLICATION_LOG=$(realpath -m "$2");;
    *) echo "unknown argument $1" >&2; exit 2;;
  esac; shift 2
done
[ -n "$SRC" ] && [ -n "$OUT" ] && { [ -n "$CLIENT" ] || [ -n "$BIN" ]; } \
  || { echo "usage: run.sh --hub-src DIR (--client VERSION | --cozy-bin FILE) --out DIR [--checks FILE] [--cozy-min X.Y.Z]" >&2; exit 2; }
[ -z "$CLIENT" ] || [ -z "$BIN" ] || { echo "choose --client or --cozy-bin" >&2; exit 2; }
[ -z "$BIN" ] || { [ -x "$BIN" ] && CLIENT=$("$BIN" --version | awk '{print $2}'); } || exit 2
mkdir -p "$OUT" && OUT=$(cd "$OUT" && pwd) && cd "$OUT" || exit 2
: > expect.tsv
log() { echo "$(date -u +%H:%M:%SZ) $*" | tee -a gate.log; }
expect() { printf '%s\t%s\t%s\n' "$1" "$2" "$(tr '\n\t' '  ' <<< "$3" | cut -c1-400)" >> expect.tsv; log "$1: $2 $3"; }
die() { expect "skew $1" FAIL "$2"; verdict; }
verdict() {
  local fails passes
  [ -n "$CLIENT_READY" ] && command -v cozy > /dev/null && for l in tensord calcifer; do timeout 30 cozy machine logs local $l -n 0 > machine-$l.log 2>&1; done
  fails=$(grep -c $'\tFAIL\t' expect.tsv) passes=$(grep -c $'\tPASS\t' expect.tsv)
  grep $'\tFAIL\t' expect.tsv | cut -f1,3 | sed 's/^/FAIL /' | tee verdict.txt
  if [ "$fails" = 0 ] && [ "$passes" -gt 0 ]; then echo "VERDICT PASS: cozy $CLIENT × the Hub at $(cat hub.version 2>/dev/null) ($passes cells)" | tee -a verdict.txt; exit 0; fi
  echo "VERDICT FAIL: cozy $CLIENT × the Hub at $(cat hub.version 2>/dev/null) ($fails of $((fails + passes)) cells)" | tee -a verdict.txt; exit 1
}
# older <a> <b>: release a is older than b (X.Y.Z; a pre-release or build suffix ignored).
older() { local a=${1%%[-+]*} b=${2%%[-+]*}; [ "$a" != "$b" ] && [ "$(printf '%s\n%s\n' "$a" "$b" | sort -V | head -1)" = "$a" ]; }
read -r HUB_PORT S3_PORT < <(python3 - <<'PY'
import socket
with socket.socket() as hub, socket.socket() as store:
    hub.bind(("127.0.0.1", 0))
    store.bind(("127.0.0.1", 0))
    print(hub.getsockname()[1], store.getsockname()[1])
PY
)
HUB=http://127.0.0.1:$HUB_PORT S3=http://127.0.0.1:$S3_PORT S3KEY=skew S3SECRET=skew-$RANDOM$RANDOM$RANDOM
MINIO=RELEASE.2025-09-07T16-13-09Z  # the archived repository's last release; its source still builds
WORK=$(mktemp -d "${TMPDIR:-/tmp}/cozy-hub-fixture.XXXXXX") || exit 1
PG_NAME=cozy-skew-pg-$$-$RANDOM
cleanup() {
  local status=$?
  for pid in "${HUBPID:-}" "${S3PID:-}" "${INGRESS:-}"; do
    [ -z "$pid" ] || { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }
  done
  docker rm -f "$PG_NAME" >/dev/null 2>&1 || true
  rm -rf "$WORK"
  return "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# The store, the database, the Hub.
log "MinIO $MINIO from source"
GOBIN=${SKEW_TOOLS:-$OUT/tools} go install "github.com/minio/minio@$MINIO" > minio-build.log 2>&1 || die store "MinIO $MINIO did not build: $(tail -c 300 minio-build.log)"
MINIO_ROOT_USER=$S3KEY MINIO_ROOT_PASSWORD=$S3SECRET "${SKEW_TOOLS:-$OUT/tools}/minio" server "$WORK/s3" --address 127.0.0.1:$S3_PORT --quiet > minio.log 2>&1 & S3PID=$!
docker run -d --name "$PG_NAME" -e POSTGRES_HOST_AUTH_METHOD=trust -p 127.0.0.1::5432 postgres:18-alpine > /dev/null || die store "Postgres did not start"
PG_PORT=$(docker port "$PG_NAME" 5432/tcp | sed 's/.*://')
for _ in $(seq 60); do curl -sf "$S3/minio/health/live" > /dev/null && docker exec "$PG_NAME" pg_isready -q -U postgres && break; sleep 1; done
for b in repo source; do
  AWS_ACCESS_KEY_ID=$S3KEY AWS_SECRET_ACCESS_KEY=$S3SECRET aws --endpoint-url "$S3" --region us-east-1 s3api create-bucket --bucket "$b" > /dev/null 2>> s3.log ||
    die store "bucket $b: $(tail -c 300 s3.log)"
done
# The store is R2's strictness, checked before the Hub uses it: a key holding ':' signed bare is refused, %3A answers.
go run "$E2E/skew/sigv4.go" "$S3" "$S3KEY" "$S3SECRET" repo > store-check.txt 2>&1 &&
  expect "skew store strict" PASS "$(cat store-check.txt)" || die store "the store is not R2's strictness: $(cat store-check.txt)"

(cd "$SRC" && nice -n 19 go build -o "$WORK/tensorhub" .) > hub-build.log 2>&1 || die hub "the Hub did not build: $(tail -c 300 hub-build.log)"
"$WORK/tensorhub" version | tee hub.version
go build -o "$WORK/ingress" "$E2E/skew/ingress.go" > ingress-build.log 2>&1 || die ingress "the local ingress did not build"
"$WORK/ingress" 127.0.0.1:0 "$HUB" "$WORK" > ingress.addr 2> ingress.log & INGRESS=$!
for _ in $(seq 60); do [ -s ingress.addr ] && break; kill -0 $INGRESS 2>/dev/null || break; sleep 1; done
ORIGIN=$(head -1 ingress.addr); [ -n "$ORIGIN" ] || die hub "the ingress did not start: $(tail -c 300 ingress.log)"
hub=("$WORK/tensorhub" --config "$SRC/config.local.yaml"
  --set env.name=development --set server.host=127.0.0.1 --set server.http_port=$HUB_PORT --set server.public_origin=$ORIGIN
  --set auth.issuer=$ORIGIN/v1/auth --set rental.hub_ca_path="$WORK/ca.pem" --set provider.kind=docker
  --set auth.keys_path="$WORK/authkit" --set auth.trusted_proxies=
  --set database.url="postgres://postgres@127.0.0.1:$PG_PORT/postgres?sslmode=disable"
  --set storage.backend.kind=s3 --set storage.backend.endpoint_url=$S3 --set storage.backend.region=us-east-1
  --set storage.backend.access_key_id=$S3KEY --set storage.backend.secret_access_key=$S3SECRET
  --set storage.repo_cas.bucket=repo --set storage.package_source.bucket=source
  --set package_release.scratch_root="$WORK/release" ${MIN:+--set clients.cozy_min=$MIN})
"${hub[@]}" migrate > hub-migrate.log 2>&1 || die hub "migrate: $(tail -c 300 hub-migrate.log)"
"${hub[@]}" serve > hub.log 2>&1 & HUBPID=$!
for _ in $(seq 90); do [ "$(curl -s -o /dev/null -w '%{http_code}' $HUB/healthz)" = 204 ] && break; kill -0 $HUBPID 2>/dev/null || break; sleep 1; done
[ "$(curl -s -o /dev/null -w '%{http_code}' $HUB/healthz)" = 204 ] || die hub "the Hub did not come up: $(tail -c 600 hub.log)"
kill -0 "$HUBPID" 2>/dev/null || die hub "the owned fixture Hub exited during startup"
log "Hub $(cat hub.version) on $HUB, machines at $ORIGIN${MIN:+, clients.cozy_min $MIN}"
. "$E2E/test-hub.sh"
export COZY_TEST_HUB_ORIGIN=$ORIGIN
require_test_hub "$HUB" || die isolation "the fixture Hub did not verify as local"

# The client, as its release installs it; every cozy process reads config.toml's hub_url.
mkdir -p ~/.cozy && printf 'hub_url = "%s"\n' "$HUB" >> ~/.cozy/config.toml
export COZY_HUB_URL=$HUB
if [ -n "$BIN" ]; then
  mkdir -p "$HOME/.cozy/bin" && install -m 0755 "$BIN" "$HOME/.cozy/bin/cozy" || die client "could not install the candidate CLI"
else
  curl -LsSf --retry 3 --retry-all-errors "https://github.com/cozy-creator/cozy-creator/releases/download/v$CLIENT/cozy-installer.sh" -o installer.sh &&
    sh installer.sh > install.log 2>&1 || die client "cozy $CLIENT's installer failed: $(tail -c 300 install.log)"
fi
export PATH=$HOME/.cozy/bin:$PATH CLI=cozy M=local R=$(( $(date +%s%N) / 1000 % 4000000000 ))
COZY_BIN=$HOME/.cozy/bin/cozy
if [ -n "$PUBLICATION_LOG" ]; then mkdir -p "${PUBLICATION_LOG%/*}"; fi
cozy() {
  if [ -n "$PUBLICATION_LOG" ]; then printf '%s\x1f' "$@" >> "$PUBLICATION_LOG"; printf '\x1e' >> "$PUBLICATION_LOG"; fi
  "$COZY_BIN" "$@"
}
[ "$(cozy --version)" = "cozy $CLIENT" ] || die client "installed cozy is '$(cozy --version)', not $CLIENT"
CLIENT_READY=1
REFUSE=; [ -n "$MIN" ] && older "$CLIENT" "$MIN" && ! older "$CLIENT" 0.23.0 && REFUSE=1

# skew_refused <check> <cozy args...>: refused with the typed client_too_old naming `cozy update`, never a 5xx.
skew_refused() {
  local check=$1 code; shift
  cozy --json "$@" > "$check.out" 2>&1 < /dev/null; code=$?
  if [ "$code" != 0 ] && grep -q '"client_too_old"' "$check.out" && grep -q 'cozy update' "$check.out"; then
    expect "skew $check" PASS "cozy $* → exit $code, client_too_old: $(grep -o '"message": *"[^"]*"' "$check.out" | head -1)"
  else
    expect "skew $check" FAIL "cozy $* → exit $code, not a typed client_too_old naming cozy update: $(tail -c 300 "$check.out")"
  fi
}
# Sign-in: the email's code, as the Hub logs it, typed into `cozy login` on stdin; then the account's name.
log "cozy login ci@skew.test: the code from the Hub's log, typed on stdin"
mkfifo login.in; exec 5<> login.in
cozy --json login ci@skew.test < login.in > login.out 2>&1 & LOGIN=$!
( sleep 300; kill $LOGIN 2>/dev/null ) & WATCH=$!  # a question this harness doesn't answer must not hang the job
code=; for _ in $(seq 120); do
  code=$(grep -h 'development device enrollment code' hub.log | grep -o '"code":"[^"]*"' | tail -1 | cut -d'"' -f4)
  [ -n "$code" ] && break
  kill -0 $LOGIN 2>/dev/null || break
  sleep 1
done
if [ -n "$code" ]; then printf '%s\nciskew%s\n' "$code" "$RANDOM" >&5; else kill $LOGIN 2>/dev/null; fi
wait $LOGIN; login=$?; exec 5>&-; kill $WATCH 2>/dev/null
if [ -n "$REFUSE" ]; then
  [ "$login" != 0 ] && grep -q '"client_too_old"' login.out && grep -q 'cozy update' login.out &&
    expect "skew login" PASS "cozy $CLIENT < clients.cozy_min $MIN: login refused, client_too_old" ||
    expect "skew login" FAIL "cozy $CLIENT < $MIN: login was not refused client_too_old (exit $login): $(tail -c 300 login.out)"
  skew_refused package-ls package ls --limit 5
  skew_refused model-ls model ls --limit 5
  grep -q '"status":5[0-9][0-9]' hub.log && expect "skew no 5xx" FAIL "the Hub logged a 5xx: $(grep '"status":5' hub.log | tail -1 | cut -c1-300)"
  verdict
fi
[ "$login" = 0 ] && expect "skew login" PASS "signed in: $(tail -c 200 login.out)" || die login "cozy login (exit $login, code ${code:-none logged}): $(tail -c 300 login.out)"

# The Hub suite's cells, through verbs every supported cozy has (a run's project and flags moved in 0.24.0): the
# package is installed on the machine from the Hub's index, not run.
HB=$OUT/hub HUB_URL=$HUB
. "$E2E/run.sh"
if ! older "$CLIENT" 0.24.0; then run_project "$OUT/project" || die project "could not initialize the fixture project"; fi
. "$HERE/suites/hub/lib.sh"
if [ -n "$RT$TF" ]; then
  update=(machine update local)
  [ -z "$RT" ] || update+=(--runtime "$RT")
  [ -z "$TF" ] || update+=(--tensorfs "$TF")
  cozy --json "${update[@]}" -y > candidate-update.json 2>&1 || die candidates "candidate update failed: $(tail -c 300 candidate-update.json)"
fi
TWF=$TF
if [ -n "$TF" ] && [ ! -f "$TF" ] && [[ $TF != *://* ]]; then TWF="tensorfs==$TF"; fi
log "account and catalog"; hub_account
[ -n "$WHO" ] || verdict
hub_do catalog ok package ls --limit 5
log "model upload by the machine, publish, download, delete"; hub_models
log "package publish, then install on the machine (its first environment: slow on a fresh runner)"; hub_publish
hub_do package-install ok package install "$WHO/$HUB_PKG" --on local
if ! older "$CLIENT" 0.24.0; then hub_packages; fi
if [ -n "$CHECKS" ]; then
  . "$CHECKS"
  checks_status=$?
  [ "$checks_status" = 0 ] || expect "fixture extension" FAIL "additional synthetic checks failed (exit $checks_status)"
fi
hub_cleanup
grep -q '"status":5[0-9][0-9]' hub.log && expect "skew no 5xx" FAIL "the Hub logged a 5xx: $(grep '"status":5' hub.log | tail -1 | cut -c1-300)"
verdict
