# The Hub's read cache (th-385), against the suite's isolated Hub and a second replica of it on the same database:
#   hub_read_cache  Paul: "it could cache, on its application side, common read-query results; especially useful if they're
#                expensive queries". 400 reads of one model (resolve and card) across both replicas read its index rows and its
#                lookup at most a few times (pg_stat_statements); then a release published and the model deleted through replica A
#                each show on replica B at once, not after the cache's minute. 50 signed-in reads read the caller's orgs once a
#                replica, and an AuthKit event (a role revoked and granted) has B read them again.

# hub_read_cache (th-385): the e2e and production load reads the same models over and over (production: 12,000 resolves a day,
# each a storage HEAD and four queries). The Hub keeps a model's index and lookup a minute, and a write to them empties them on
# every replica by Postgres NOTIFY.
hub_read_cache() (
  step "repeated model reads come from the Hub's cache, and a second replica agrees at once after a write (th-385)"
  [ -n "${COZY_TEST_HUB_COMMAND_FILE:-}" ] && [ -n "${COZY_HUB_URL:-}" ] && [ -n "${E2E_HUB_EMAIL:-}" ] \
    || fail "hub read cache: e2e/isolated.sh named no Hub command, URL or email"
  local S=$OUT/hub-read-cache db port container hub=() word second B A=$COZY_HUB_URL account model c1 c2 calls lag
  mkdir -p "$S"
  db=$(tr '\0' '\n' < "$COZY_TEST_HUB_COMMAND_FILE" | grep -o 'postgres://[^" ]*' | head -1 || true)
  port=$(sed -E 's#.*@[^:/]+:([0-9]+)/.*#\1#' <<< "$db")
  container=$(docker ps --filter "publish=$port" --format '{{.Names}}' 2> /dev/null | head -1 || true)
  [ -n "$container" ] || fail "hub read cache: no Postgres container publishes ${port:-?}"
  sql() { docker exec -i -e PGAPPNAME=th385 "$container" psql -v ON_ERROR_STOP=1 -qtA -U postgres "$@"; }
  sql -c "SELECT 1 FROM pg_extension WHERE extname = 'pg_stat_statements'" | grep -q 1 \
    || fail "hub read cache: the fixture's Postgres has no pg_stat_statements (e2e/skew/run.sh)"

  # Replica B: the same Hub on the same database and store, on a port of its own.
  while IFS= read -r -d '' word; do hub+=("$word"); done < "$COZY_TEST_HUB_COMMAND_FILE"
  second=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
  B=http://127.0.0.1:$second
  nice -n 19 "${hub[@]}" --set server.http_port="$second" serve > "$S/replica-b.log" 2>&1 &
  local replica=$!
  trap 'kill "$replica" 2> /dev/null; wait "$replica" 2> /dev/null; cozy --json logout > /dev/null 2>&1 || true' EXIT
  for _ in $(seq 90); do [ "$(curl -s -o /dev/null -w '%{http_code}' "$B/readyz")" = 204 ] && break; sleep 1; done
  [ "$(curl -s -o /dev/null -w '%{http_code}' "$B/readyz")" = 204 ] || fail "hub read cache: replica B didn't come up: $(tail -c 400 "$S/replica-b.log")"

  # A model of two 1 MiB checkpoints, the first released 0.1.0.
  signin "$E2E_HUB_EMAIL" > "$S/login.json" || fail "hub read cache: cozy login: $(tail -c 300 "$S/login.json")"
  account=$(cozy --json whoami | jq -r '.account // empty')
  model=${account:-e2e}/th385-cache-$RANDOM
  upload() {
    python3 -I - "$S/$1.safetensors" "$1" <<'PY'
import json, os, struct, sys
n = 1 << 20
header = json.dumps({"w": {"dtype": "F16", "shape": [n // 2], "data_offsets": [0, n]}, "__metadata__": {"part": sys.argv[2]}}).encode()
header += b" " * (-len(header) % 8)
with open(sys.argv[1], "wb") as out:
    out.write(struct.pack("<Q", len(header)) + header + bytes(b & 0xBF for b in os.urandom(n)))
PY
    cozy --json model upload "$S/$1.safetensors" "$model" --lane bf16 --source-format as-is --on local -y > "$S/upload-$1.ndjson" 2> "$S/upload-$1.err" \
      || return 1
    grep '^{' "$S/upload-$1.ndjson" | jq -rs --arg m "$model" '[.. | objects | select(.published?.destination? == $m) | .published.commit] | last // empty'
  }
  c1=$(upload one) && [ -n "$c1" ] || fail "hub read cache: uploading $model (one) named no commit: $(tail -c 300 "$S/upload-one.ndjson") $(tail -c 200 "$S/upload-one.err")"
  cozy --json model publish "$model" 0.1.0 "$c1" -y > "$S/publish-1.json" 2>&1 || fail "hub read cache: publishing 0.1.0: $(tail -c 300 "$S/publish-1.json")"
  c2=$(upload two) && [ -n "$c2" ] || fail "hub read cache: uploading $model (two) named no commit: $(tail -c 300 "$S/upload-two.ndjson") $(tail -c 200 "$S/upload-two.err")"

  # 200 resolves and 200 cards, half to each replica, 20 at once; what the Hub asked of Postgres meanwhile.
  local path=/v1/models/$model
  for _ in $(seq 100); do printf '%s\n' "$A$path/resolve" "$B$path/resolve" "$A$path" "$B$path"; done > "$S/reads.txt"
  sql -c "SELECT pg_stat_statements_reset()" > /dev/null
  xargs -P 20 -I{} curl -s -o /dev/null -w '%{http_code}\n' -m 60 {} < "$S/reads.txt" > "$S/codes.txt" 2>&1 || true
  bash "$here/../skew/pg-stats.sh" "$container" 60 > "$S/statements.tsv" 2>&1 || true
  read_calls() { sql -c "SELECT coalesce(sum(calls), 0) FROM pg_stat_statements WHERE regexp_replace(query, '\s+', ' ', 'g') LIKE '%$1%'"; }
  local index lookup etag every
  index=$(( $(read_calls 'FROM hub_v2.model_commits WHERE repo_id=$1 ORDER BY created_at') + $(read_calls 'FROM hub_v2.model_releases WHERE repo_id=$1 ORDER BY created_at') ))
  etag=$(read_calls 'SELECT etag FROM hub_v2.model_repos WHERE repo_id=$1')
  lookup=$(read_calls 'FROM hub_v2.repos WHERE org = $1 AND kind = $3')
  every=$(sql -c "SELECT coalesce(sum(calls), 0) FROM pg_stat_statements WHERE query NOT LIKE '%pg_stat_statements%'")
  echo "hub read cache: 400 reads ($(sort "$S/codes.txt" | uniq -c | awk '{printf "%s×%s ", $2, $1}')) asked Postgres $every statements: index rows $index, its file's ETag $etag, the model's lookup $lookup"
  head -12 "$S/statements.tsv" | cut -c1-220
  ! grep -qv '^200$' "$S/codes.txt" || check "hub read cache: 400 reads of $model answered $(sort "$S/codes.txt" | uniq -c | awk '{printf "%s×%s ", $2, $1}')"
  [ "$index" -le 8 ] || check "hub read cache: 400 reads of one model read its index rows $index times, not from a cache (8 at most: each replica's first, twice)"
  [ "$lookup" -le 8 ] || check "hub read cache: 400 reads of one model looked it up $lookup times, not from a cache (8 at most)"

  # Both replicas hold 0.1.0; a release published through A shows on B at once.
  newest() { curl -s -m 10 "$1$path/resolve" | jq -r '.release // empty'; }
  agree() {  # agree <what> <jq of a replica's answer> <wanted> <path>: both replicas answer it, B within 2 s of the write
    local t0 seen_a seen_b=''
    t0=$(date +%s%N)
    while :; do
      seen_b=$(curl -s -m 10 "$B$4" | jq -r "$2" 2> /dev/null || true)
      [ "$seen_b" = "$3" ] && break
      [ $(( ($(date +%s%N) - t0) / 1000000 )) -lt 2000 ] || break
      sleep 0.05
    done
    lag=$(( ($(date +%s%N) - t0) / 1000000 ))
    seen_a=$(curl -s -m 10 "$A$4" | jq -r "$2" 2> /dev/null || true)
    echo "hub read cache: $1: replica B answered '$seen_b' ${lag} ms after the write, A '$seen_a'"
    [ "$seen_b" = "$3" ] || check "hub read cache: $1: replica B still answered '$seen_b', not '$3', 2 s after the write through A"
    [ "$seen_a" = "$3" ] || check "hub read cache: $1: replica A, which took the write, answered '$seen_a', not '$3'"
  }
  [ "$(newest "$A")" = 0.1.0 ] && [ "$(newest "$B")" = 0.1.0 ] || check "hub read cache: before the write the replicas answered $(newest "$A") and $(newest "$B"), not 0.1.0"
  cozy --json model publish "$model" 0.2.0 "$c2" -y > "$S/publish-2.json" 2>&1 || fail "hub read cache: publishing 0.2.0: $(tail -c 300 "$S/publish-2.json")"
  agree "a release published" '.release // empty' 0.2.0 "$path/resolve"
  # The model deleted: neither replica finds it (its lookup).
  cozy --json model delete "$model" -y > "$S/delete.json" 2>&1 || fail "hub read cache: cozy model delete: $(tail -c 300 "$S/delete.json")"
  agree "the model deleted" '.error.code // "resolved"' model.not_readable "$path/resolve"

  # A signed-in caller's orgs come from the cache; an AuthKit event (the root role revoked and granted again by the Hub's
  # operator command, as skew/run.sh grants it) empties it on B.
  local token who before after
  token=$(jq -r .token "$(ls "$HOME"/.cozy/keys/*.bearer.json | head -1)" 2> /dev/null || true)
  who=$(cozy --json whoami | jq -r '.account // empty')
  [ -n "$token" ] && [ -n "$who" ] || fail "hub read cache: no Bearer kept in $HOME/.cozy/keys, or no account"
  orgs() { read_calls 'FROM hub_v2.orgs WHERE id::text = ANY($1)'; }
  signed() { for _ in $(seq "$2"); do curl -s -o /dev/null -w '%{http_code}\n' -m 10 -H "Authorization: Bearer $token" "$1/v1/accounts/current"; done; }
  before=$(orgs)
  { signed "$A" 25; signed "$B" 25; } > "$S/signed.txt"
  after=$(orgs)
  echo "hub read cache: 50 signed-in reads ($(sort "$S/signed.txt" | uniq -c | awk '{printf "%s×%s ", $2, $1}')) read the caller's orgs $((after - before)) times"
  [ $((after - before)) -le 2 ] || check "hub read cache: 50 signed-in reads read the caller's orgs $((after - before)) times, not from a cache (2 at most)"
  { "${hub[@]}" admin revoke "$who" && "${hub[@]}" admin grant "$who"; } > "$S/admin.log" 2>&1 \
    || fail "hub read cache: revoking and granting $who's root role: $(tail -c 300 "$S/admin.log")"
  sleep 1
  before=$(orgs)
  signed "$B" 1 >> "$S/signed.txt"
  after=$(orgs)
  echo "hub read cache: after AuthKit recorded a role change, replica B read the caller's orgs $((after - before)) times"
  [ $((after - before)) -ge 1 ] || check "hub read cache: replica B kept the caller's orgs after AuthKit recorded a role change"
)
