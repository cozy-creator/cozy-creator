#!/usr/bin/env bash
# e2e/skew/run.sh (--hub-src DIR | --hub-bin DIR) (--client VERSION | --cozy-bin FILE | --candidates SET) --out DIR
#   [--checks FILE | --command FILE [-- ARGS...]] [--admin] [--cozy-min X.Y.Z]: a cozy against a Hub built from
# source (--hub-bin: one built already, DIR holding `tensorhub`, its config.local.yaml and `minio`, built for this OS), on a fresh
# GitHub runner with Go (xs-166; e2e.yaml's `skew`, by hand on a computer that is yours to replace). Postgres 18 (AuthKit's
# migrations call uuidv7) is Docker's on Linux, Homebrew's on macOS and EDB's binaries on Windows. The Hub serves on that
# throwaway Postgres and
# a MinIO built from its last source, whose SigV4 check is R2's: a
# key's reserved characters are percent-encoded in the canonical request, so v0.40.33-35's bare ':' in a workload's
# proof key fails here as it failed on R2. It runs as a development Hub (sign-in codes in its log): cozy dials it on
# loopback, and machines at its https public origin, an ingress under a fresh CA the Hub hands them (ingress.go; a Hub
# hands a CA only beside its docker provider, which rents nothing here; macOS and Windows have no Docker Engine, so no
# provider and no machines: their cozy runs no models).
# The client is installed by its own release's installer into this computer's default home, signed in with the code
# the Hub logs, and runs the Hub suite's cells (release-gate/suites/hub/lib.sh) on its local machine: account, the
# catalog, package publish, install and run, new weights uploaded by the machine (the workload writer, as pods
# upload), publish, download, delete. A client older than --cozy-min that names its version (`cozy/<version>`, from
# 0.23.0) must instead be refused at every step with the typed client_too_old naming `cozy update`. Any 5xx fails.
# --candidates takes e2e/candidates' $CANDIDATES: its --tensord file (or version) or its --release's Linux archive is the
# client, with its --runtime and --tensorfs. --command runs a focused suite instead of the cells: `FILE OUT COZY RUNTIME
# TENSORFS ARGS...`, signed in, as the Hub's admin with --admin. --software RUNTIME,TENSORFS[,COZY] pins the Hub's software (what
# `cozy machine update <machine>` naming nothing moves a machine to, and the cozy that updates itself for a package, cl-978). A
# cell restarts the Hub with other settings by e2e/skew/rehub.sh.
set -uo pipefail
E2E=$(cd "$(dirname "$0")/.." && pwd) HERE=$(cd "$(dirname "$0")/../../release-gate" && pwd)
START=$PWD
SRC= HUB_BIN= CLIENT= BIN= CHECKS= COMMAND= OUT= MIN= RT= TF= PUBLICATION_LOG= CLIENT_READY= ADMIN= RELEASE= PRIVATE_ARTIFACTS= SOFTWARE=
COMMAND_ARGS=()
while [ $# -gt 0 ]; do
  case $1 in
    --hub-src) SRC=$(cd "$2" && pwd) || exit 2;; --hub-bin) HUB_BIN=$(cd "$2" && pwd) || exit 2;;
    --client) CLIENT=${2#v};; --cozy-bin) BIN=$(realpath "$2") || exit 2;; --checks) CHECKS=$(realpath "$2") || exit 2;; --out) OUT=$2;; --cozy-min) MIN=$2;;
    --runtime) RT=$2; if [ -f "$RT" ]; then RT=$(realpath "$RT"); fi;;
    --tensorfs) TF=$2; if [ -f "$TF" ]; then TF=$(realpath "$TF"); fi;;
    --candidates)
      read -r -a cand <<< "$2"
      for ((i = 0; i + 1 < ${#cand[@]}; i += 2)); do
        v=${cand[i + 1]}
        case ${cand[i]} in
          --tensord) if [ -f "$v" ]; then BIN=$(realpath "$v"); else CLIENT=${v#v}; fi;;
          --runtime) RT=$v; if [ -f "$RT" ]; then RT=$(realpath "$RT"); fi;;
          --tensorfs) TF=$v; if [ -f "$TF" ]; then TF=$(realpath "$TF"); fi;;
          --release) RELEASE=$(realpath "$v") || exit 2;;
        esac
      done;;
    --publication-log) PUBLICATION_LOG=$(realpath -m "$2");;
    --software) SOFTWARE=$2;;
    --command) COMMAND=$(realpath "$2") || exit 2;;
    --admin) ADMIN=1; shift; continue;;
    --private-artifacts) PRIVATE_ARTIFACTS=1; shift; continue;;
    --) shift; COMMAND_ARGS=("$@"); break;;
    *) echo "unknown argument $1" >&2; exit 2;;
  esac; shift 2
done
if [ -n "$RELEASE" ]; then
  archive=$(find "$RELEASE" -maxdepth 1 -name 'cozy-x86_64-unknown-linux-gnu.tar.gz' -print -quit)
  [ -n "$archive" ] || { echo "no Linux candidate archive in $RELEASE" >&2; exit 2; }
  client_dir=$(mktemp -d "${TMPDIR:-/tmp}/cozy-fixture-client.XXXXXX") && tar -xzf "$archive" -C "$client_dir" || exit 2
  BIN=$(find "$client_dir" -type f -name cozy -print -quit) && [ -n "$BIN" ] && chmod +x "$BIN" || { echo "the candidate archive has no cozy" >&2; exit 2; }
  CLIENT=
fi
{ [ -n "$SRC" ] || [ -n "$HUB_BIN" ]; } && [ -n "$OUT" ] && { [ -n "$CLIENT" ] || [ -n "$BIN" ]; } \
  || { echo "usage: run.sh (--hub-src DIR | --hub-bin DIR) (--client VERSION | --cozy-bin FILE | --candidates SET) --out DIR [--checks FILE | --command FILE [-- ARGS...]] [--admin] [--cozy-min X.Y.Z]" >&2; exit 2; }
[ -z "$CLIENT" ] || [ -z "$BIN" ] || { echo "choose --client or --cozy-bin" >&2; exit 2; }
[ -z "$COMMAND" ] || { [ -n "$BIN" ] && [ -z "$CHECKS" ] && [ -f "$COMMAND" ]; } \
  || { echo "--command requires --cozy-bin and a script file, and cannot be combined with --checks" >&2; exit 2; }
[ -z "$BIN" ] || { [ -x "$BIN" ] && CLIENT=$("$BIN" --version | awk '{print $2}'); } || exit 2
OS=${RUNNER_OS:-Linux} EXE=''
[ "$OS" != Windows ] || EXE=.exe
# A path as a program that isn't this shell's reads it: Windows's own spelling, which Go's filepath.Clean leaves as it is.
native() { if [ "$OS" = Windows ]; then cygpath -w "$1"; else echo "$1"; fi; }
NICE=(nice -n 19)
[ "$OS" != Windows ] || NICE=()
mkdir -p "$OUT" && OUT=$(cd "$OUT" && pwd) && cd "$OUT" || exit 2
: > expect.tsv
log() { echo "$(date -u +%H:%M:%SZ) $*" | tee -a gate.log; }
expect() { printf '%s\t%s\t%s\n' "$1" "$2" "$(tr '\n\t' '  ' <<< "$3" | cut -c1-400)" >> expect.tsv; log "$1: $2 $3"; }
die() { expect "skew $1" FAIL "$2"; verdict; }
verdict() {
  local fails passes
  [ -z "$PRIVATE_ARTIFACTS" ] && [ -n "$CLIENT_READY" ] && [ "$OS" = Linux ] && command -v cozy > /dev/null && for l in tensord calcifer; do timeout 30 cozy machine logs local $l -n 0 > machine-$l.log 2>&1; done
  fails=$(grep -c $'\tFAIL\t' expect.tsv) passes=$(grep -c $'\tPASS\t' expect.tsv)
  grep $'\tFAIL\t' expect.tsv | cut -f1,3 | sed 's/^/FAIL /' | tee verdict.txt
  if [ "$fails" = 0 ] && [ "$passes" -gt 0 ]; then echo "VERDICT PASS: cozy $CLIENT × the Hub at $(cat hub.version 2>/dev/null) ($passes cells)" | tee -a verdict.txt; exit 0; fi
  echo "VERDICT FAIL: cozy $CLIENT × the Hub at $(cat hub.version 2>/dev/null) ($fails of $((fails + passes)) cells)" | tee -a verdict.txt; exit 1
}
# older <a> <b>: release a is older than b (X.Y.Z; a pre-release or build suffix ignored).
older() { local a=${1%%[-+]*} b=${2%%[-+]*}; [ "$a" != "$b" ] && [ "$(printf '%s\n%s\n' "$a" "$b" | sort -V | head -1)" = "$a" ]; }
# Windows's Python ends its lines in CR LF.
read -r HUB_PORT S3_PORT PG_PORT < <(python3 - <<'PY' | tr -d '\r'
import socket
with socket.socket() as hub, socket.socket() as store, socket.socket() as db:
    for s in (hub, store, db):
        s.bind(("127.0.0.1", 0))
    print(hub.getsockname()[1], store.getsockname()[1], db.getsockname()[1])
PY
)
HUB=http://127.0.0.1:$HUB_PORT S3=http://127.0.0.1:$S3_PORT S3KEY=skew S3SECRET=skew-$RANDOM$RANDOM$RANDOM
MINIO=RELEASE.2025-09-07T16-13-09Z  # the archived repository's last release; its source still builds
# Canonical: the Hub refuses a scratch path that isn't clean, and macOS's $TMPDIR ends in a slash.
WORK=$(mktemp -d "${TMPDIR:-/tmp}/cozy-hub-fixture.XXXXXX") && WORK=$(cd "$WORK" && pwd -P) || exit 1
PG_NAME=cozy-skew-pg-$$-$RANDOM PG_BIN=''
cleanup() {
  local status=$?
  [ ! -s "$WORK/hub.pid" ] || HUBPID=$(cat "$WORK/hub.pid")
  for pid in "${HUBPID:-}" "${S3PID:-}" "${INGRESS:-}"; do
    [ -z "$pid" ] || { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }
  done
  if [ "$OS" = Linux ]; then docker rm -f "$PG_NAME" >/dev/null 2>&1 || true
  elif [ -n "$PG_BIN" ]; then "$PG_BIN/pg_ctl" -D "$(native "$WORK/pg")" -m immediate stop >/dev/null 2>&1 || true; fi
  [ ! -f "$WORK/requests.jsonl" ] || cp "$WORK/requests.jsonl" "$OUT/hub-requests.jsonl"
  rm -rf "$WORK"
  return "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# The store, the database, the Hub.
TOOLS=${SKEW_TOOLS:-$OUT/tools}
[ -z "$HUB_BIN" ] || TOOLS=$HUB_BIN
if [ ! -x "$TOOLS/minio$EXE" ]; then
  log "MinIO $MINIO from source"
  GOBIN=$TOOLS go install "github.com/minio/minio@$MINIO" > minio-build.log 2>&1 || die store "MinIO $MINIO did not build: $(tail -c 300 minio-build.log)"
fi
# The fixture stands in for services on other computers: it takes only what CPU the suites leave (nice, Postgres's weight).
MINIO_ROOT_USER=$S3KEY MINIO_ROOT_PASSWORD=$S3SECRET "${NICE[@]}" "$TOOLS/minio$EXE" server "$(native "$WORK/s3")" --address 127.0.0.1:$S3_PORT --quiet > minio.log 2>&1 & S3PID=$!
case $OS in
  Linux)
    docker run -d --name "$PG_NAME" --cpu-shares 64 -e POSTGRES_HOST_AUTH_METHOD=trust -p 127.0.0.1::5432 postgres:18-alpine > /dev/null || die store "Postgres did not start"
    PG_PORT=$(docker port "$PG_NAME" 5432/tcp | sed 's/.*://')
    pg_ready() { docker exec "$PG_NAME" pg_isready -q -U postgres; } ;;
  *)
    # No Docker for Linux images here: Postgres 18 for this OS, in a cluster of this run's. The Windows image's is 17.
    if [ "$OS" = macOS ]; then
      HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_INSTALL_CLEANUP=1 HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK=1 brew install -q postgresql@18 > postgres-install.log 2>&1 \
        || die store "brew install postgresql@18: $(tail -c 300 postgres-install.log)"
      PG_BIN=$(brew --prefix postgresql@18)/bin pg_options="-p $PG_PORT -c listen_addresses=127.0.0.1 -k $WORK"
    else
      pg=postgresql-18.4-1-windows-x64-binaries.zip
      curl -fsSL --retry 3 --retry-all-errors -o "$WORK/$pg" "https://get.enterprisedb.com/postgresql/$pg" > postgres-install.log 2>&1 \
        && 7z x -y -o"$(native "$WORK")" "$(native "$WORK/$pg")" >> postgres-install.log 2>&1 \
        || die store "Postgres 18 for Windows: $(tail -c 300 postgres-install.log)"
      rm -f "$WORK/$pg"
      PG_BIN=$WORK/pgsql/bin pg_options="-p $PG_PORT -c listen_addresses=127.0.0.1"
    fi
    "$PG_BIN/initdb" -D "$(native "$WORK/pg")" -U postgres -A trust -E UTF8 --no-locale > postgres-init.log 2>&1 || die store "initdb: $(tail -c 300 postgres-init.log)"
    "$PG_BIN/pg_ctl" -D "$(native "$WORK/pg")" -l "$(native "$OUT/postgres.log")" -o "$pg_options" -w start > postgres-start.log 2>&1 \
      || die store "Postgres did not start: $(tail -c 300 postgres-start.log) $(tail -c 300 postgres.log 2>/dev/null)"
    pg_ready() { "$PG_BIN/pg_isready" -q -h 127.0.0.1 -p "$PG_PORT" -U postgres; } ;;
esac
for _ in $(seq 60); do curl -sf "$S3/minio/health/live" > /dev/null && pg_ready && break; sleep 1; done
for b in repo source; do
  AWS_ACCESS_KEY_ID=$S3KEY AWS_SECRET_ACCESS_KEY=$S3SECRET aws --endpoint-url "$S3" --region us-east-1 s3api create-bucket --bucket "$b" > /dev/null 2>> s3.log ||
    die store "bucket $b: $(tail -c 300 s3.log)"
done
# The store is R2's strictness, checked before the Hub uses it: a key holding ':' signed bare is refused, %3A answers.
# NTFS holds no ':' name, so MinIO on Windows can't store the key; such keys are workload proofs, and that Hub rents nothing.
if [ "$OS" = Windows ]; then
  log "skew store strict: not checked on Windows (NTFS holds no ':' key; this Hub rents nothing, so it writes no workload proof)"
else
  go run "$E2E/skew/sigv4.go" "$S3" "$S3KEY" "$S3SECRET" repo > store-check.txt 2>&1 &&
    expect "skew store strict" PASS "$(cat store-check.txt)" || die store "the store is not R2's strictness: $(cat store-check.txt)"
fi

if [ -n "$HUB_BIN" ]; then
  cp "$HUB_BIN/tensorhub$EXE" "$WORK/tensorhub$EXE" && CONFIG=$HUB_BIN/config.local.yaml || die hub "no built Hub in $HUB_BIN"
else
  (cd "$SRC" && "${NICE[@]}" go build -o "$WORK/tensorhub$EXE" .) > hub-build.log 2>&1 || die hub "the Hub did not build: $(tail -c 300 hub-build.log)"
  CONFIG=$SRC/config.local.yaml
fi
"$WORK/tensorhub$EXE" version | tee hub.version
go build -o "$(native "$WORK/ingress$EXE")" "$(native "$E2E/skew/ingress.go")" > ingress-build.log 2>&1 || die ingress "the local ingress did not build: $(tail -c 300 ingress-build.log)"
"${NICE[@]}" "$WORK/ingress$EXE" 127.0.0.1:0 "$HUB" "$(native "$WORK")" > ingress.addr 2> ingress.log & INGRESS=$!
for _ in $(seq 60); do [ -s ingress.addr ] && break; kill -0 $INGRESS 2>/dev/null || break; sleep 1; done
ORIGIN=$(head -1 ingress.addr | tr -d '\r'); [ -n "$ORIGIN" ] || die hub "the ingress did not start: $(tail -c 300 ingress.log)"
# Machines rent from the docker provider, which hands them the ingress's CA. With no Docker Engine there are none to rent.
provider=(--set rental.hub_ca_path="$(native "$WORK/ca.pem")" --set provider.kind=docker)
[ "$OS" = Linux ] || provider=(--set provider.kind=runpod)
hub=("$WORK/tensorhub$EXE" --config "$(native "$CONFIG")"
  --set env.name=development --set server.host=127.0.0.1 --set server.http_port=$HUB_PORT --set server.public_origin=$ORIGIN
  --set auth.issuer=$ORIGIN/v1/auth "${provider[@]}"
  --set auth.keys_path="$(native "$WORK/authkit")" --set auth.trusted_proxies=
  --set database.url="postgres://postgres@127.0.0.1:$PG_PORT/postgres?sslmode=disable"
  --set storage.backend.kind=s3 --set storage.backend.endpoint_url=$S3 --set storage.backend.region=us-east-1
  --set storage.backend.access_key_id=$S3KEY --set storage.backend.secret_access_key=$S3SECRET
  --set storage.repo_cas.bucket=repo --set storage.package_source.bucket=source
  --set package_release.scratch_root="$(native "$WORK/release")" ${MIN:+--set clients.cozy_min=$MIN}
  ${SOFTWARE:+--set software.runtime=$(cut -d, -f1 <<< "$SOFTWARE") --set software.tensorfs=$(cut -d, -f2 <<< "$SOFTWARE")}
  ${SOFTWARE:+--set software.cozy=$(cut -s -d, -f3 <<< "$SOFTWARE")})
(umask 077; printf '%s\0' "${hub[@]}" > "$WORK/hub-command.argv")
export COZY_TEST_HUB_COMMAND_FILE=$WORK/hub-command.argv
"${hub[@]}" migrate > hub-migrate.log 2>&1 || die hub "migrate: $(tail -c 300 hub-migrate.log)"
# AuthKit refuses a TOTP key that is group or world writable, and Go reads every writable Windows file as 0666: the
# development key it would write is written here, read-only (0444; readable is a warning only).
if [ "$OS" = Windows ]; then
  { mkdir -p "$WORK/authkit" && python3 -c 'import secrets; print(secrets.token_hex(32))' | tr -d '\r' > "$WORK/authkit/totp.key" &&
    attrib +R "$(native "$WORK/authkit/totp.key")"; } > authkit-key.log 2>&1 || die hub "the read-only TOTP key: $(tail -c 300 authkit-key.log)"
fi
"${NICE[@]}" "${hub[@]}" serve > hub.log 2>&1 & HUBPID=$!
echo "$HUBPID" > "$WORK/hub.pid"
export COZY_TEST_HUB_PID_FILE=$WORK/hub.pid COZY_TEST_HUB_SERVE_LOG=$OUT/hub.log COZY_TEST_HUB_HEALTH=$HUB/healthz
for _ in $(seq 90); do [ "$(curl -s -o /dev/null -w '%{http_code}' $HUB/healthz)" = 204 ] && break; kill -0 $HUBPID 2>/dev/null || break; sleep 1; done
[ "$(curl -s -o /dev/null -w '%{http_code}' $HUB/healthz)" = 204 ] || die hub "the Hub did not come up: $(tail -c 600 hub.log)"
kill -0 "$HUBPID" 2>/dev/null || die hub "the owned fixture Hub exited during startup"
log "Hub $(cat hub.version) on $HUB, machines at $ORIGIN${MIN:+, clients.cozy_min $MIN}"
. "$E2E/test-hub.sh"
export COZY_TEST_HUB_ORIGIN=$ORIGIN
require_test_hub "$HUB" || die isolation "the fixture Hub did not verify as local"
export COZY_TEST_HUB_CA=$WORK/ca.pem
export COZY_TEST_HUB_REQUEST_LOG=$WORK/requests.jsonl

# The client, as its release installs it; every cozy process reads config.toml's hub_url.
mkdir -p ~/.cozy && printf 'hub_url = "%s"\n' "$HUB" >> ~/.cozy/config.toml
export COZY_HUB_URL=$HUB
if [ -n "$BIN" ]; then
  mkdir -p "$HOME/.cozy/bin" && install -m 0755 "$BIN" "$HOME/.cozy/bin/cozy$EXE" || die client "could not install the candidate CLI"
else
  curl -LsSf --retry 3 --retry-all-errors "https://github.com/cozy-creator/cozy-creator/releases/download/v$CLIENT/cozy-installer.sh" -o installer.sh &&
    sh installer.sh > install.log 2>&1 || die client "cozy $CLIENT's installer failed: $(tail -c 300 install.log)"
fi
export PATH=$HOME/.cozy/bin:$PATH CLI=cozy M=local R=$(( ($(date +%s) % 4000000) * 1000 + RANDOM % 1000 ))
COZY_BIN=$HOME/.cozy/bin/cozy$EXE
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
# cl-989 diagnostics (throwaway branch): Calcifer in the foreground, and the CLI's own view while login waits.
if [ "$OS" = Windows ]; then
  log "cl-989 diag: cozy calcifer in the foreground (30 s bound)"
  timeout 30 "$COZY_BIN" calcifer > calcifer-fg.out 2>&1; fg=$?
  log "cl-989 diag: cozy calcifer exited $fg; output: $(tr '\r\n' '  ' < calcifer-fg.out | tail -c 300)"
  { echo "== ~/.cozy"; ls -la ~/.cozy; echo "== calcifer"; ls -la ~/.cozy/calcifer; echo "== services"; ls -la ~/.cozy/services; cat ~/.cozy/calcifer/refused.json; } > cozy-tree-fg.txt 2>&1
  log "cl-989 diag: after the foreground run: $(tr '\r\n' '  ' < cozy-tree-fg.txt | tail -c 600)"
  (
    sleep 45
    for i in 1 2 3 4 5; do cat ~/.cozy/services/calcifer.birth 2>&1; echo; sleep 1; done > births-during.txt
    timeout 60 "$COZY_BIN" doctor > doctor-during.out 2>&1; echo "doctor exit $?" >> doctor-during.out
    tasklist > tasks-during.txt 2>&1
    { ls -la ~/.cozy ~/.cozy/calcifer ~/.cozy/services; cat ~/.cozy/calcifer/refused.json; } > cozy-tree-during.txt 2>&1
  ) &
  DIAG=$!
fi
log "cozy login ci@skew.test: the code from the Hub's log, typed on stdin"
# Its stdin a pipe, written once the Hub has logged the code (or not, after two minutes): an ended pipe is an answer, so a
# question this harness doesn't answer can't hang the job.
{
  for _ in $(seq 120); do
    code=$(grep -h 'development device enrollment code' hub.log | grep -o '"code":"[^"]*"' | tail -1 | cut -d'"' -f4)
    [ -z "$code" ] || { printf '%s\nciskew%s\n' "$code" "$RANDOM"; break; }
    grep -q '"error"' login.out 2>/dev/null && break  # refused before a code: nothing to type
    sleep 1
  done
} | { [ -z "$PUBLICATION_LOG" ] || { printf '%s\x1f' --json login ci@skew.test; printf '\x1e'; } >> "$PUBLICATION_LOG"; timeout 150 "$COZY_BIN" --json login ci@skew.test; } > login.out 2>&1
login=${PIPESTATUS[1]}
if [ "$OS" = Windows ]; then
  wait "${DIAG:-}" 2>/dev/null
  log "cl-989 diag: login exit $login; births while it waited: $(tr '\r\n' '  ' < births-during.txt 2>/dev/null | tail -c 300)"
  log "cl-989 diag: cozy doctor while login waited: $(tr '\r\n' '  ' < doctor-during.out 2>/dev/null | tail -c 1500)"
  log "cl-989 diag: cozy.exe processes: $(grep -i cozy tasks-during.txt 2>/dev/null | tr '\r\n' '  ' | tail -c 600)"
  log "cl-989 diag: home while login waited: $(tr '\r\n' '  ' < cozy-tree-during.txt 2>/dev/null | tail -c 800)"
  [ "$login" = 0 ] || { timeout 20 "$COZY_BIN" login ci@skew.test < /dev/null > login-plain.out 2>&1; log "cl-989 diag: plain login (20 s): exit $?: $(tr '\r\n' '  ' < login-plain.out | tail -c 600)"; }
fi
code=$(grep -h 'development device enrollment code' hub.log | grep -o '"code":"[^"]*"' | tail -1 | cut -d'"' -f4)
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

if [ -n "$ADMIN" ]; then
  WHO=$(cozy --json whoami | python3 -c 'import json,sys; print(json.load(sys.stdin)["account"])' | tr -d '\r') && "${hub[@]}" admin grant "$WHO" > fixture-admin.log 2>&1 \
    || die account "could not make the fixture account its Hub's admin: $(tail -c 300 fixture-admin.log)"
fi

# A focused suite inherits the real isolated Hub, OAuth key and default home,
# but its shell variables/traps cannot alter this runner's cleanup ownership.
if [ -n "$COMMAND" ]; then
  # From where this runner was started: its arguments may be relative to it.
  (cd "$START" && bash "$COMMAND" "$OUT/command" "$COZY_BIN" "$RT" "$TF" "${COMMAND_ARGS[@]}")
  command_status=$?
  if [ "$command_status" = 0 ]; then expect "fixture command" PASS "the focused suite completed"
  else expect "fixture command" FAIL "the focused suite exited $command_status"; fi
  grep -q '"status":5[0-9][0-9]' hub.log && expect "skew no 5xx" FAIL "the Hub logged a 5xx: $(grep '"status":5' hub.log | tail -1 | cut -c1-300)"
  verdict
fi

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
require_test_hub "$HUB" || die isolation "the publication account requires the owned fixture Hub"
"${hub[@]}" admin grant "$WHO" > fixture-admin.log 2>&1 \
  || die account "could not authorize the isolated fixture publisher: $(tail -c 300 fixture-admin.log)"
hub_do catalog ok package ls --limit 5
log "model upload by the machine, publish, download, delete"; hub_models
log "package publish, then install on the machine (its first environment: slow on a fresh runner)"; hub_publish
hub_do package-install ok package install "$WHO/$HUB_PKG" --on local
if ! older "$CLIENT" 0.24.0; then hub_packages; fi
# skew_newer (cl-978): a release whose cozy-runtime floor only a newer cozy's line runs (NEXT: the next line's Runtime, on
# PyPI). A client before cozy's self-update (0.26) is refused, typed, naming `cozy update`, at its run and its install, and so
# is its machine's move to the Hub's software (`cozy machine update local`, what its machine's refusals name), which it
# can't run; never a 5xx. A newer client updates itself (e2e/cli/selfupdate.sh).
skew_newer() {
  local next dir=$HB/pkg-newer name=ci-next-$R code
  case $CLIENT in
    0.24.*) next=0.46.0 ;;
    *) log "skew newer: no released Runtime needs a cozy newer than $CLIENT yet"; return 0 ;;
  esac
  rm -rf "$dir" && cp -r "$HERE/suites/hub/pkg" "$dir" &&
    sed -i "s/^name = \"ci-hub-pkg\"/name = \"$name\"/; s/\"cozy-runtime>=0.30\"/\"cozy-runtime>=$next\"/" "$dir/pyproject.toml" || return 1
  (cd "$dir" && hub_do newer-publish ok package publish -y) || return 0
  for verb in run install software; do
    case $verb in
      run) (cd "$RUN_PROJECT" && timeout 1200 "$COZY_BIN" --json run "$WHO/$name/echo" text=newer --on "$M") > "$HB/newer-$verb.out" 2>&1 < /dev/null; code=$? ;;
      install) timeout 1200 "$COZY_BIN" --json package install "$WHO/$name" --on "$M" > "$HB/newer-$verb.out" 2>&1 < /dev/null; code=$? ;;
      software) timeout 1200 "$COZY_BIN" --json machine update "$M" -y > "$HB/newer-$verb.out" 2>&1 < /dev/null; code=$? ;;
    esac
    log "skew newer $verb (cozy $CLIENT, $name needs cozy-runtime>=$next): exit $code: $(grep -o '"code": *"[^"]*"\|"message": *"[^"]*"\|"next": *\[[^]]*\]' "$HB/newer-$verb.out" | head -6 | tr '\n' ' ')"
    if hub_5xx "$HB/newer-$verb.out"; then
      expect "skew newer-$verb" FAIL "cozy $CLIENT: a server failure: $(tail -c 300 "$HB/newer-$verb.out")"
    elif [ "$code" != 0 ] && grep -q 'cozy update' "$HB/newer-$verb.out" && ! grep -q '"code": *"internal"' "$HB/newer-$verb.out"; then
      expect "skew newer-$verb" PASS "cozy $CLIENT refused $name (cozy-runtime>=$next), naming cozy update: $(grep -o '"message": *"[^"]*"' "$HB/newer-$verb.out" | head -1)"
    else
      expect "skew newer-$verb" FAIL "cozy $CLIENT → exit $code, not a refusal naming cozy update: $(tail -c 400 "$HB/newer-$verb.out")"
    fi
  done
}
if ! older "$CLIENT" 0.24.0; then skew_newer; fi
if [ -n "$CHECKS" ]; then
  . "$CHECKS" "${COMMAND_ARGS[@]}"
  checks_status=$?
  [ "$checks_status" = 0 ] || expect "fixture extension" FAIL "additional synthetic checks failed (exit $checks_status)"
fi
hub_cleanup
grep -q '"status":5[0-9][0-9]' hub.log && expect "skew no 5xx" FAIL "the Hub logged a 5xx: $(grep '"status":5' hub.log | tail -1 | cut -c1-300)"
verdict
