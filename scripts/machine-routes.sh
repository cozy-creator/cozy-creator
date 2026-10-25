#!/usr/bin/env bash
# TensorD's router and the machine API document (crates/tensord/api/openapi.yaml) name the same routes, by method and
# path (parameter names aside), both ways; and every route the machine client calls is in the document.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
doc=$root/crates/tensord/api/openapi.yaml
routers=("$root/crates/tensord/src/api/http.rs" "$root/crates/tensord/src/api/server.rs")
client=$root/crates/protocol/src/machine/client.rs
shape() { sed -E 's/\{[^}]*\}/{}/g' | sort -u; }
fail() { echo "machine-routes: $*" >&2; exit 1; }

# `path::NAME` to its path, from protocol::machine's `pub mod path`.
paths=$(sed -n '/^pub mod path {/,/^}/p' "$root/crates/protocol/src/machine.rs" |
  { grep -oE 'const [A-Z_]+: &str = "[^"]+"' || true; } | sed -E 's/const ([A-Z_]+): &str = "([^"]+)"/\1 \2/')
resolve() { awk 'NR == FNR { at[$1] = $2; next } !($2 in at) { print "?", $2; next } { print $1, at[$2] }' <(echo "$paths") -; }

documented=$(awk '/^  [^ ]/ { path = $1 } /^    (get|put|post|patch|delete|head):/ { print toupper($1), path }' "$doc" |
  tr -d ":\"'" | { grep -E '^[A-Z]+ /' || true; } | shape)

# Each `.route(path::NAME, verb(…).verb(…))` on one line: its verbs.
lines=$(cat "${routers[@]}" | { grep -E '\.route\(' || true; })
served=$(while read -r line; do
  name=$(sed -E 's/.*\.route\((super::v1::)?path::([A-Z_]+),.*/\2/' <<<"$line")
  { grep -oE '(^|[^a-z_])(get|post|put|head|delete|patch)\(' <<<"${line#*,}" || true; } | tr -dc 'a-z\n' |
    tr a-z A-Z | sed "s/\$/ $name/"
done <<<"$lines" | resolve | shape)
[ "$(cat "${routers[@]}" | grep -o '\.route(' | wc -l)" = "$(grep -cE '\.route\((super::v1::)?path::[A-Z_]+,' <<<"$lines")" ] ||
  fail "every .route( names a path:: constant on one line"

source=$(tr '\n' ' ' <"$client")
calls=$({ grep -oE '\.request\( *Method::[A-Z]+, *path::[A-Z_]+' <<<"$source" || true; } |
  sed -E 's/.*Method::([A-Z]+), *path::([A-Z_]+)/\1 \2/')
# The client's two request builders pass `method` through; every other .request( is a call.
[ "$(grep -oE '\.request\( *[^m ]' <<<"$source" | wc -l)" = "$(wc -l <<<"$calls")" ] ||
  fail "every client .request( names its Method and a path:: constant literally"
called=$(resolve <<<"$calls" | shape)

[ -n "$documented" ] && [ -n "$served" ] && [ -n "$called" ] || fail "no routes read"
! printf '%s\n%s\n' "$served" "$called" | grep -q '^?' || fail "a route names a path:: constant protocol::machine does not have"
drift=
differ() { [ -z "$2" ] || drift+=$(printf '\n%s:\n%s' "$1" "$2"); }
differ "TensorD serves, the document lacks" "$(comm -23 <(echo "$served") <(echo "$documented"))"
differ "the document has, TensorD does not serve" "$(comm -13 <(echo "$served") <(echo "$documented"))"
differ "the client calls, the document lacks" "$(comm -23 <(echo "$called") <(echo "$documented"))"
[ -z "$drift" ] || fail "the routes drifted:$drift"
echo "machine-routes: TensorD serves the document's $(wc -l <<<"$documented") routes; the client calls $(wc -l <<<"$called") of them"
