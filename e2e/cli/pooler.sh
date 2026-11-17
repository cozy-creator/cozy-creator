# e2e/cli/pooler.sh: the isolated Hub behind PgBouncer in transaction mode, as production's CNPG Pooler runs it (th-384). Linux.
#   hub_pooled_connections  th-384 (Paul: "it should use a connection pool in production"): PgBouncer 1.25.1, the image CNPG 1.29.1
#                runs, in transaction mode in front of the fixture's Postgres, tracking search_path as tensorhub-gitops#329 sets it.
#                The Hub restarted with database.pooled_url (budget 50) says it is pooled. Under 800 requests, 80 at once, with
#                blob reads slowed for 3 s: no 5xx, PgBouncer carries the Hub's queries on at most its pool of 6, the Hub's
#                sessions straight to Postgres stay within what it says, no LISTEN goes through PgBouncer, River keeps its
#                leader, and a package publication commits (its finalize holds the org-index lock, a session's).

hub_pooled_connections() (
  step "the Hub's own queries through PgBouncer in transaction mode; its sessions straight to Postgres (th-384)"
  [ -n "${COZY_TEST_HUB_COMMAND_FILE:-}" ] && [ -n "${COZY_HUB_URL:-}" ] && [ -n "${E2E_HUB_EMAIL:-}" ] && [ -n "${COZY_TEST_HUB_SERVE_LOG:-}" ] \
    || fail "pooler: e2e/isolated.sh named no Hub command, URL, email or log"
  local S=$OUT/hub-pooler budget=50 pool=6 image=ghcr.io/cloudnative-pg/pgbouncer:1.25.1 name=th384-pgbouncer
  local db pgport port container said shas token sampler before after carried peak_hub peak_direct peak_pooled listens copy state
  mkdir -p "$S"
  db=$(tr '\0' '\n' < "$COZY_TEST_HUB_COMMAND_FILE" | grep -o 'postgres://[^" ]*' | head -1 || true)
  pgport=$(sed -E 's#.*@[^:/]+:([0-9]+)/.*#\1#' <<< "$db")
  container=$(docker ps --filter "publish=$pgport" --format '{{.Names}}' 2> /dev/null | head -1 || true)
  [ -n "$container" ] || fail "pooler: no Postgres container publishes ${pgport:-?}"
  sql() { docker exec -i -e PGAPPNAME=th384 "$container" psql -v ON_ERROR_STOP=1 -qtA -U postgres "$@"; }
  bouncer() { docker exec -i "$name" psql -h 127.0.0.1 -p "$port" -U postgres -qA -F ' ' pgbouncer "$@"; }
  # PgBouncer's transactions for the fixture's database (SHOW STATS, by its header).
  xacts() { bouncer -c 'SHOW STATS' | awk 'NR == 1 { for (i = 1; i <= NF; i++) if ($i == "total_xact_count") c = i; next } c && $1 == "postgres" { print $c }'; }
  release() { sql -c "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity WHERE application_name = 'th384-slow'" > /dev/null 2>&1 || true; }
  cleanup() {
    release
    sql -c "DELETE FROM hub_v2.verified_objects WHERE bucket = 'th384'" > /dev/null 2>&1 || true
    docker logs "$name" > "$S/pgbouncer.log" 2>&1 || true
    docker rm -f "$name" > /dev/null 2>&1 || true
    bash "$here/../skew/rehub.sh" > /dev/null 2>&1 || true
  }
  trap cleanup EXIT

  # PgBouncer on the host's network, so it reaches Postgres where the Hub does; trust, as the fixture's Postgres.
  port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
  mkdir -p "$S/etc"
  cat > "$S/etc/pgbouncer.ini" <<INI
[databases]
* = host=127.0.0.1 port=$pgport

[pgbouncer]
listen_addr = 127.0.0.1
listen_port = $port
unix_socket_dir =
auth_type = trust
auth_file = /etc/pgbouncer/userlist.txt
admin_users = postgres
pool_mode = transaction
default_pool_size = $pool
max_db_connections = $pool
max_client_conn = 400
ignore_startup_parameters = extra_float_digits,options
track_extra_parameters = IntervalStyle, search_path
application_name_add_host = 1
INI
  echo '"postgres" ""' > "$S/etc/userlist.txt"
  chmod 755 "$S/etc" && chmod 644 "$S/etc/"*
  docker run -d --name "$name" --network host --cpu-shares 64 -v "$S/etc:/etc/pgbouncer:ro" "$image" > "$S/run.out" 2>&1 \
    || fail "pooler: PgBouncer did not start: $(tail -c 300 "$S/run.out")"
  for _ in $(seq 60); do bouncer -t -c 'SHOW VERSION' > "$S/version.txt" 2> /dev/null && break; sleep 0.5; done
  grep -q 'PgBouncer 1.25.1' "$S/version.txt" || fail "pooler: PgBouncer 1.25.1 didn't answer: $(cat "$S/version.txt"; docker logs "$name" 2>&1 | tail -5)"

  signin "$E2E_HUB_EMAIL" > "$S/login.json" || fail "pooler: cozy login: $(tail -c 300 "$S/login.json")"
  # Behind PgBouncer: a Hub before th-384 refuses the key and is restarted on its budget alone, straight to Postgres.
  bash "$here/../skew/rehub.sh" --set "database.connections_per_replica=$budget" \
    --set "database.pooled_url=postgres://postgres@127.0.0.1:$port/postgres?sslmode=disable" > "$S/rehub.log" 2>&1 \
    || bash "$here/../skew/rehub.sh" --set "database.connections_per_replica=$budget" > "$S/rehub-direct.log" 2>&1 \
    || fail "pooler: restarting the Hub: $(tail -c 300 "$S/rehub.log")"
  said=$(grep '"database connected"' "$COZY_TEST_HUB_SERVE_LOG" | tail -1)
  echo "pooler: the Hub says $(jq -c '{connections_per_replica, pooled, hub_pool, session_pool, postgres, tensorhub, "tensorhub-authkit"}' <<< "$said" 2> /dev/null || echo 'nothing')"
  [ "$(jq -r '.pooled // false' <<< "$said" 2> /dev/null)" = true ] \
    || check "pooler: the Hub didn't take database.pooled_url: $(tail -c 300 "$S/rehub.log" | tr '\n' ' ')"

  cozy --json whoami > /dev/null 2>&1 || true
  token=$(jq -r .token "$(ls "$HOME"/.cozy/keys/*.bearer.json | head -1)" 2> /dev/null || true)
  [ -n "$token" ] || fail "pooler: no Bearer kept in $HOME/.cozy/keys"
  sql > "$S/objects.out" 2>&1 <<'SQL' || fail "pooler: recording public objects: $(tail -c 300 "$S/objects.out")"
INSERT INTO hub_v2.verified_objects (object_id, length, bucket, key, backend_version, etag, checksum_sha256, storage_domain, custody_scope, public)
SELECT 'sha256:' || h, 16 << 20, 'th384', 'th384/' || h, '', '', '', 'repo_cas', 'shared', true
  FROM (SELECT encode(sha256(('th384-' || i || '-' || random())::bytea), 'hex') AS h FROM generate_series(1, 40) i) o;
SQL
  shas=$(sql -c "SELECT substr(object_id, 8) FROM hub_v2.verified_objects WHERE bucket = 'th384'")
  before=$(xacts)

  # Each 0.2 s: the Hub's sessions straight to Postgres, PgBouncer's (its host added to their label), and LISTENs on each.
  (while :; do sql -F ' ' -c "SELECT count(*) FILTER (WHERE application_name = 'tensorhub'),
      count(*) FILTER (WHERE application_name IN ('tensorhub', 'tensorhub-authkit')),
      count(*) FILTER (WHERE application_name LIKE 'tensorhub - %'),
      count(*) FILTER (WHERE application_name LIKE 'tensorhub - %' AND query ILIKE 'LISTEN%'),
      count(*) FILTER (WHERE application_name IN ('tensorhub', 'tensorhub-authkit') AND query ILIKE 'LISTEN%')
      FROM pg_stat_activity WHERE backend_type = 'client backend'" 2> /dev/null; sleep 0.2; done) > "$S/sessions.txt" &
  sampler=$!
  docker exec -i -e PGAPPNAME=th384-slow "$container" psql -qtA -U postgres > "$S/slow.out" 2>&1 <<'SQL' &
BEGIN;
LOCK TABLE hub_v2.verified_objects IN ACCESS EXCLUSIVE MODE;
SELECT pg_sleep(3);
COMMIT;
SQL
  for _ in $(seq 50); do
    [ "$(sql -c "SELECT count(*) FROM pg_locks l JOIN pg_stat_activity a USING (pid) WHERE a.application_name = 'th384-slow' AND l.mode = 'AccessExclusiveLock' AND l.granted" 2> /dev/null || echo 0)" -ge 1 ] && break
    sleep 0.1
  done
  for _ in $(seq 10); do printf '%s\n' $shas; done | sed -E 's#^(..)(..)(.*)$#/blobs/\1/\2/\1\2\3#' > "$S/load.txt"
  for _ in $(seq 400); do echo /v1/accounts/current; done >> "$S/load.txt"
  shuf "$S/load.txt" | xargs -P 80 -I{} curl -s -o /dev/null -w '%{http_code}\n' -m 60 -H "Authorization: Bearer $token" \
    "$COZY_HUB_URL{}" > "$S/codes.txt" 2>&1 || true
  release

  # A publication's finalize holds the org-index lock, a session's lock, across its commit: it must commit behind PgBouncer.
  copy=$S/e2e-pooled
  rm -rf "$copy" && cp -r "$here/../../release-gate/suites/hub/pkg" "$copy"
  sed -i 's/^name = "ci-hub-pkg"$/name = "e2e-pooled"/' "$copy/pyproject.toml"
  (cd "$copy" && cozy --json package publish -y) > "$S/publish.json" 2>&1 || check "pooler: publishing e2e-pooled: $(tail -c 400 "$S/publish.json" | tr '\n' ' ')"
  for _ in $(seq 120); do
    state=$(sql -c "SELECT o.state FROM hub_v2.package_release_operations o JOIN hub_v2.repos r ON r.id = o.repo_id
      WHERE r.kind = 'package' AND r.name = 'e2e-pooled' LIMIT 1" 2> /dev/null || true)
    [ "$state" != committed ] || break
    sleep 1
  done
  kill "$sampler" 2> /dev/null || true
  wait "$sampler" 2> /dev/null || true

  after=$(xacts)
  bouncer -c 'SHOW POOLS' > "$S/pools.txt" 2>&1 || true
  carried=$(( ${after:-0} - ${before:-0} ))
  peak() { awk -v c="$1" '$c > m { m = $c } END { print m + 0 }' "$S/sessions.txt"; }
  peak_hub=$(peak 1) peak_direct=$(peak 2) peak_pooled=$(peak 3) listens=$(peak 5)
  echo "pooler: 800 requests answered $(sort "$S/codes.txt" | uniq -c | awk '{printf "%s×%s ", $2, $1}')"
  echo "pooler: PgBouncer carried $carried transactions; most sessions at once: tensorhub straight $peak_hub of $(jq -r '.tensorhub // "?"' <<< "$said" 2> /dev/null), straight in all $peak_direct of $(jq -r '.postgres // "?"' <<< "$said" 2> /dev/null), PgBouncer's $peak_pooled of $pool; LISTENs straight $listens, through PgBouncer $(peak 4); e2e-pooled ${state:-not recorded}"
  [ "$carried" -ge 400 ] || check "pooler: PgBouncer carried $carried transactions, not the Hub's 400 blob reads at least"
  ! grep -q '^5' "$S/codes.txt" || check "pooler: $(grep -c '^5' "$S/codes.txt") of 800 requests answered a 5xx"
  [ "$(grep -c . "$S/codes.txt")" = 800 ] || check "pooler: $(grep -c . "$S/codes.txt") of 800 requests answered"
  [ "$(peak 4)" = 0 ] || check "pooler: $(peak 4) LISTEN sessions went through PgBouncer"
  [ "$listens" -ge 1 ] || check "pooler: no LISTEN session straight to Postgres"
  [ "$peak_pooled" -le "$pool" ] || check "pooler: PgBouncer held $peak_pooled server connections, over its pool of $pool"
  local share total
  share=$(jq -r '.tensorhub // empty' <<< "$said" 2> /dev/null || true) total=$(jq -r '.postgres // empty' <<< "$said" 2> /dev/null || true)
  [ -n "$share" ] && [ "$peak_hub" -le "$share" ] || check "pooler: tensorhub's sessions straight to Postgres reached $peak_hub, over its share ${share:-(none said)}"
  [ -n "$total" ] && [ "$peak_direct" -le "$total" ] || check "pooler: the Hub's sessions straight to Postgres reached $peak_direct, over ${total:-(none said)}"
  [ "$(sql -c 'SELECT count(*) FROM river.river_leader WHERE expires_at > now()' 2> /dev/null || echo 0)" = 1 ] \
    || check "pooler: River has no leader"
  [ "$state" = committed ] || check "pooler: e2e-pooled's publication is ${state:-not recorded}, not committed"
)
