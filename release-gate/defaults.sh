#!/usr/bin/env bash
# defaults.sh <packages checkout> <commit> [<held org/name@release>...]: run by promote.sh before anything moves, whatever its
# --phases (cl-1243). Every model a slot of a package at that commit loads by default is public, or held: one of the set's
# `models` (candidate.json), which the models phase publishes. On 11-18 eight defaults of packages rc/0.27.0 named private
# releases, and every default call of their packages failed 404 `release.not_found`, the owner's too (th-412).
# The defaults are the interfaces cozy describes from the commit's source (`cozy run <dir> --describe` on this computer's
# local, siblings by path: cl-1211): every slot of every function, private and needs-chosen ones too. A default is
# `[org/]model@release`: the package contract refuses one with no release or a digest, so that package is unread.
# The Hub is asked once a release that isn't held (`cozy model show`, signed in as you).
# Exit 0: each public or held, a line each held. Exit 1: a FAIL line each release neither, and an unread line each package
# cozy didn't describe or release the Hub didn't answer for (never private, never passed), naming each package, function
# and slot that defaults to it.
set -uo pipefail
[ $# -ge 2 ] || { sed -n '2,12s/^# \{0,1\}//p' "$0" >&2; exit 2; }
PKGS=$1 REF=$2; shift 2
began=$SECONDS
rooted() { case $1 in */*) echo "$1" ;; *) echo "_/$1" ;; esac; }  # a model with no org is the root's
HELD=" $(for m in "$@"; do rooted "$m"; done | paste -sd' ') "
git -C "$PKGS" cat-file -e "$REF^{commit}" 2> /dev/null || git -C "$PKGS" fetch -q origin "$REF" ||
  { echo "unread: $PKGS can't fetch $REF" >&2; exit 1; }
commit=$(git -C "$PKGS" rev-parse "$REF^{commit}") || exit 1
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
mkdir "$W/src" && git -C "$PKGS" archive "$commit" | tar -x -C "$W/src" || { echo "unread: $PKGS can't read $commit" >&2; exit 1; }
# slots: <release> TAB <package> <function> <slot>, a line each slot default (a quantize source has none: its caller names it).
packages=0 unread=0; : > "$W/slots"
for d in "$W"/src/*/; do
  grep -qs cozy.application "$d/pyproject.toml" || continue
  p=$(basename "$d") packages=$((packages + 1))
  if cozy --json --full run "$d" --describe > "$W/$p.json" 2> "$W/$p.err" && jq -r --arg p "$p" '.interface.entrypoints[]
      | .name as $fn | .models[]? | select(.default != null) | [.default, "\($p) \($fn) \(.path | split(".") | last)"] | @tsv' \
      "$W/$p.json" > "$W/$p.slots" 2> /dev/null; then
    cat "$W/$p.slots" >> "$W/slots"
  else
    unread=$((unread + 1)) why=$(jq -r '.error // empty | "\(.code): \(.message)"' "$W/$p.json" 2> /dev/null)
    echo "unread $p: cozy didn't describe it at ${commit:0:12}: ${why:-$(tail -n 1 "$W/$p.err" | grep . || echo 'no interface')}" >&2
  fi
done
[ "$packages" -gt 0 ] || { echo "unread: no package at ${commit:0:12}" >&2; exit 1; }
where() { awk -F'\t' -v r="$1" '$1 == r {print $2}' "$W/slots" | paste -sd';' | sed 's/;/; /g'; }
held=0 public=0 refused=0; : > "$W/asked"
while read -r r; do
  if [[ $HELD == *" $(rooted "$r") "* ]]; then
    held=$((held + 1)); echo "held $r: one of the set's models, which the models phase publishes: $(where "$r")"
  else echo "$r" >> "$W/asked"; fi
done < <(cut -f1 "$W/slots" | sort -u)
# The rest from the Hub, four at a time (as e2e/cpu/lanes.py asks).
awk '{print NR, $0}' "$W/asked" | xargs -r -P 4 -n 2 sh -c 'timeout 120 cozy --json model show "$2" > "$0/shown.$1" 2> /dev/null' "$W"
i=0
while read -r r; do
  i=$((i + 1)) shown=$(cat "$W/shown.$i" 2> /dev/null)
  case $(jq -r 'if .visibility then .visibility elif .error.kind == "not_found" then "absent" else empty end' <<< "$shown" 2> /dev/null) in
    public) public=$((public + 1)) ;;
    absent) refused=$((refused + 1))
      echo "FAIL $r: the Hub says \"$(jq -r .error.message <<< "$shown")\" and it is not one of the set's models: $(where "$r")" >&2 ;;
    '') unread=$((unread + 1)) why=$(jq -r '.error // empty | "\(.code): \(.message)"' <<< "$shown" 2> /dev/null)
      echo "unread $r: the Hub didn't say whether it is public (${why:-no answer from cozy model show}): $(where "$r")" >&2 ;;
    *) refused=$((refused + 1))
      echo "FAIL $r is $(jq -r .visibility <<< "$shown") and not one of the set's models: $(where "$r")" >&2 ;;
  esac
done < "$W/asked"
echo "defaults at ${commit:0:12}: $(grep -c . "$W/slots") slot defaults in $packages packages name $((held + i)) releases:" \
  "$public public, $held held, $refused refused, $unread unread ($((SECONDS - began)) s)"
[ "$refused" = 0 ] && [ "$unread" = 0 ]
