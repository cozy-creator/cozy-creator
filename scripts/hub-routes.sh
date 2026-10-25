#!/usr/bin/env bash
# Every route hub-client calls (`Method::<VERB>, "<path>"`) is one Tensorhub's API document has, by method and path,
# parameter names aside: scripts/hub-routes.sh <tensorhub api/openapi.yaml>. CI reads the document at check time.
set -euo pipefail
shape() { sed -E 's/\{[^}]*\}/{}/g' | sort -u; }
documented=$(awk '/^  [^ ]/ { path = $1 } /^    (get|put|post|patch|delete):/ { print toupper($1), path }' "$1" |
  tr -d ":\"'" | { grep -E '^[A-Z]+ /' || true; } | shape)
source=$(cat "$(dirname "$0")"/../crates/hub-client/src/*.rs | tr '\n' ' ')
calls=$({ grep -oE '\.at\( *Method::[A-Z]+, *"[^"]*"' <<<"$source" || true; } | sed -E 's/.*Method::([A-Z]+), *"([^"]*)"/\1 \2/')
[ -n "$documented" ] && [ -n "$calls" ] || { echo "hub-routes: no routes read" >&2; exit 1; }
# A route not written out where it is called would go unchecked.
[ "$(grep -o '\.at(' <<<"$source" | wc -l)" = "$(wc -l <<<"$calls")" ] ||
  { echo "hub-routes: every .at( names its Method and route literally" >&2; exit 1; }
called=$(shape <<<"$calls")
missing=$(comm -23 <(echo "$called") <(echo "$documented"))
if [ -n "$missing" ]; then
  printf 'hub-client calls routes api/openapi.yaml does not have:\n%s\n' "$missing" >&2
  exit 1
fi
echo "hub-routes: $(wc -l <<<"$called") routes hub-client calls, all in api/openapi.yaml"
