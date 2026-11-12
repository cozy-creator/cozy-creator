#!/bin/bash
# verify.sh <gate run id | gate dir>...: prints the candidate set that passed (candidate.json) and exits 0 only when
# every SKU's gate passed (or the lead accepted its FAIL) and all of them gated the same set. A run id is a
# completed, repository-owned workflow_dispatch of release-gate.yml (its gate-<SKU> artifacts);
# a dir is a hand gate's --out. A trusted failed run may carry a lead-accepted FAIL.
# Promotions (promote.sh, cozy-promote.yaml, tensorfs-publish.yaml, cozy-runtime runtime-publish.yaml) check what they
# publish against this output. --release (a Runtime, TensorFS or cozy release; a hand gate's record names its output): every
# row of escaped.tsv, a bug that once reached users, passed on a SKU it names in one of these gates.
set -euo pipefail
RELEASE=; [ "${1:-}" != --release ] || { RELEASE=1; shift; }
[ $# -gt 0 ] || { sed -n '2,8s/^# \{0,1\}//p' "$0" >&2; exit 2; }
dirs=() tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
for e in "$@"; do
  if [[ $e =~ ^[0-9]+$ ]]; then
    # PR artifacts run the PR's code. Their own PASS/hash claims cannot establish that a gate ran.
    # Branch dispatches remain supported for release candidates; dispatch authority is repository-scoped.
    gh api "repos/cozy-creator/cozy-creator/actions/runs/$e" | jq -e '
      .repository.full_name == "cozy-creator/cozy-creator" and
      .head_repository.full_name == "cozy-creator/cozy-creator" and
      .event == "workflow_dispatch" and .path == ".github/workflows/release-gate.yml" and
      .status == "completed" and (.conclusion == "success" or .conclusion == "failure")
    ' > /dev/null || { echo "$e: not a completed trusted release-gate dispatch" >&2; exit 1; }
    gh run download "$e" -R cozy-creator/cozy-creator -p 'gate-*' -D "$tmp/$e" >&2
    for d in "$tmp/$e"/gate-*; do [ -d "$d" ] && dirs+=("$d"); done
  else
    dirs+=("$e")
  fi
done
[ ${#dirs[@]} -gt 0 ] || { echo "no gate in $*" >&2; exit 1; }
first=
for d in "${dirs[@]}"; do
  grep -qE '^VERDICT (PASS|FAIL .*accepted by the lead)' "$d/verdict.txt" 2>/dev/null \
    || { echo "$d: $(grep -s '^VERDICT' "$d/verdict.txt" || echo 'no verdict')" >&2; exit 1; }
  [ -f "$d/candidate.json" ] || { echo "$d: no candidate.json" >&2; exit 1; }
  c=$(jq -S 'del(.label)' "$d/candidate.json")
  [ -z "$first" ] || [ "$c" = "$first" ] || { echo "$d gated another set than ${dirs[0]}" >&2; exit 1; }
  first=$c
  echo "$d: $(grep '^VERDICT' "$d/verdict.txt")" >&2
done
if [ -n "$RELEASE" ]; then
  # escaped.tsv: <expect row> <SKU pattern> <issue>. A gate's SKU is its baseline key less its suites.
  missing=
  while IFS=$'\t' read -r row skus issue; do
    [ -n "$row" ] && [ "${row:0:1}" != "#" ] || continue
    hit=
    for d in "${dirs[@]}"; do
      sku=$(cat "$d/baseline-key" 2>/dev/null); sku=${sku%-$(paste -sd- "$d/suites" 2>/dev/null)}
      [[ $sku == @($skus) ]] || continue
      awk -F'\t' -v r="$row" '$1 == r && $2 == "PASS" {f = 1} END {exit !f}' "$d/expect.tsv" 2>/dev/null && { hit="$sku: $d"; break; }
    done
    if [ -n "$hit" ]; then echo "escaped $row ($issue) PASS on $hit" >&2; else missing="$missing $row($skus, $issue)"; fi
  done < "$(dirname "$0")/escaped.tsv"
  [ -z "$missing" ] || { echo "a release gate must pass every escaped bug's cell; not passed here:$missing" >&2; exit 1; }
fi
echo "$first"
