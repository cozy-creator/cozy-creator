#!/usr/bin/env bash
# e2e/skew/pg-stats.sh CONTAINER [LIMIT]: the fixture Postgres's statements since its last pg_stat_statements_reset(), most total
# time first (th-385): calls, total and mean ms, rows, and the query on one line.
set -euo pipefail
docker exec -i "$1" psql -qtA -F $'\t' -U postgres <<SQL
SELECT 'calls', 'total_ms', 'mean_ms', 'rows', 'query';
SELECT calls, round(total_exec_time::numeric, 1), round(mean_exec_time::numeric, 3), rows, regexp_replace(query, '\s+', ' ', 'g')
  FROM pg_stat_statements ORDER BY total_exec_time DESC LIMIT ${2:-500};
SQL
