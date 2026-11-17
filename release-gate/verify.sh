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
  # escaped.tsv: <expect row> <SKU pattern> <issue> [packages=<pkg,...>]. A gate's SKU is its baseline key less its suites.
  # A row naming packages is required only when the set changes one (its version at the set's packages_ref is a prerelease,
  # as suite packages reads it) or carries a Runtime other than the last release's (PyPI's newest cozy-runtime); one it
  # can't read is required.
  missing= pref=$(jq -r '.packages_ref // ""' <<< "$first") crt=$(jq -r '.runtime // ""' <<< "$first") released=
  required() {
    local p v
    [ -n "$1" ] || return 0
    if [ -n "$crt" ]; then
      [ -n "$released" ] || released=$(curl -fsSL --retry 3 https://pypi.org/pypi/cozy-runtime/json | jq -r '.info.version // empty') || true
      [ -n "$released" ] && [ "$crt" = "$released" ] || { WHY="Runtime $crt, the release's ${released:-unread}"; return 0; }
    fi
    [ -n "$pref" ] || { WHY="no packages_ref"; return 1; }
    for p in $(tr , ' ' <<< "${1#packages=}"); do
      v=$(gh api "repos/cozy-creator/packages/contents/$p/pyproject.toml?ref=$pref" -H 'Accept: application/vnd.github.raw' 2>/dev/null |
        sed -n 's/^version = "\(.*\)"$/\1/p' | head -1)
      [ -n "$v" ] || { WHY="$p unread at $pref"; return 0; }
      [[ ! $v =~ (a|b|rc)[0-9]+$|\.dev[0-9]+$ ]] || { WHY="$p $v"; return 0; }
    done
    WHY="${1#packages=} released and the Runtime the release's"; return 1
  }
  while IFS=$'\t' read -r row skus issue when; do
    [ -n "$row" ] && [ "${row:0:1}" != "#" ] || continue
    WHY=
    if ! required "${when:-}"; then echo "escaped $row ($issue) not required: $WHY" >&2; continue; fi
    hit=
    for d in "${dirs[@]}"; do
      sku=$(cat "$d/baseline-key" 2>/dev/null); sku=${sku%-$(paste -sd- "$d/suites" 2>/dev/null)}
      [[ $sku == @($skus) ]] || continue
      awk -F'\t' -v r="$row" '$1 == r && $2 == "PASS" {f = 1} END {exit !f}' "$d/expect.tsv" 2>/dev/null && { hit="$sku: $d"; break; }
    done
    if [ -n "$hit" ]; then echo "escaped $row ($issue) PASS on $hit${WHY:+ (required: $WHY)}" >&2; else missing="$missing $row($skus, $issue${WHY:+; $WHY})"; fi
  done < "$(dirname "$0")/escaped.tsv"
  [ -z "$missing" ] || { echo "a release gate must pass every escaped bug's cell; not passed here:$missing" >&2; exit 1; }
fi
echo "$first"
