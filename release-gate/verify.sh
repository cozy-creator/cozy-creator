#!/bin/bash
# verify.sh <gate run id | gate dir>...: prints the candidate set that passed (candidate.json) and exits 0 only when
# every SKU's gate passed (or the lead accepted its FAIL) and all of them gated the same set. A run id is a
# completed, repository-owned workflow_dispatch of release-gate.yml (its gate-<SKU> artifacts);
# a dir is a hand gate's --out. A trusted failed run may carry a lead-accepted FAIL.
# Promotions (promote.sh, cozy-promote.yaml, tensorfs-publish.yaml, cozy-runtime runtime-publish.yaml) check what they
# publish against this output. --release (a Runtime, TensorFS or cozy release; a hand gate's record names its output): every
# row of escaped.tsv, a bug that once reached users, passed on a SKU it names in one of these gates, or escaped-accepted.tsv
# accepts it by name for this release: such a row is printed in full, never passed over. --unchanged FILE (with --release):
# a required row these gates did not run, named in FILE (`<row> <gate run id>`, gate-select's record), stands on that earlier
# gate's pass when nothing its escaped-paths.tsv line names changed between that gate's set and this one: printed UNCHANGED.
# --checkouts DIR: where the cozy-runtime and packages checkouts are (default: beside this one), each holding both sets' commits.
set -euo pipefail
RELEASE= UNCHANGED= CHECKOUTS=
while [ $# -gt 0 ]; do
  case $1 in
    --release) RELEASE=1; shift ;;
    --unchanged) UNCHANGED=${2:?--unchanged FILE}; shift 2 ;;
    --checkouts) CHECKOUTS=${2:?--checkouts DIR}; shift 2 ;;
    *) break ;;
  esac
done
[ $# -gt 0 ] || { sed -n '2,13s/^# \{0,1\}//p' "$0" >&2; exit 2; }
[ -z "$UNCHANGED" ] || [ -n "$RELEASE" ] || { echo "--unchanged goes with --release" >&2; exit 2; }
dirs=() heads=() tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
# trusted <run id>: its commit, when it is a completed, repository-owned dispatch of release-gate.yml; else nothing.
trusted() {
  # PR artifacts run the PR's code. Their own PASS/hash claims cannot establish that a gate ran.
  # Branch dispatches remain supported for release candidates; dispatch authority is repository-scoped.
  gh api "repos/cozy-creator/cozy-creator/actions/runs/$1" | jq -er '
    select(.repository.full_name == "cozy-creator/cozy-creator" and
      .head_repository.full_name == "cozy-creator/cozy-creator" and
      .event == "workflow_dispatch" and .path == ".github/workflows/release-gate.yml" and
      .status == "completed" and (.conclusion == "success" or .conclusion == "failure")) | .head_sha'
}
for e in "$@"; do
  if [[ $e =~ ^[0-9]+$ ]]; then
    h=$(trusted "$e") || { echo "$e: not a completed trusted release-gate dispatch" >&2; exit 1; }
    heads+=("$h")
    gh run download "$e" -R cozy-creator/cozy-creator -p 'gate-*' -D "$tmp/$e" >&2
    for d in "$tmp/$e"/gate-*; do [ -d "$d" ] && dirs+=("$d"); done
  else
    dirs+=("$e") heads+=("")
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
  # escaped-accepted.tsv: <row> <why it was not run> <who accepted it> <date> <release>, tab-separated, all five filled: a line
  # that isn't refuses the release. A line counts only for the set it names, `cozy=<x.y.z>,runtime=<x.y.z>,tensorfs=<x.y.z>`
  # (this set's versions less any -dev, .dev or build suffix), so an acceptance never carries to a later release of any of the
  # three (a Runtime-only gate keeps the released cozy), and never for a row that ran and failed in one of these gates.
  missing= pref=$(jq -r '.packages_ref // ""' <<< "$first") crt=$(jq -r '.runtime // ""' <<< "$first") released=
  release=
  for k in cozy runtime tensorfs; do
    v=$(jq -r --arg k "$k" '.[$k] // ""' <<< "$first"); v=${v%%[-+]*}; v=${v%%.dev*}
    [ -n "$v" ] && release="$release${release:+,}$k=$v" || { release=; break; }
  done
  accepted=$(dirname "$0")/escaped-accepted.tsv
  bad=$(awk -F'\t' 'function ok(s) { return s ~ /^[^ \r]/ && s ~ /[^ \r]$/ }
    !/^#/ && NF && !(NF == 5 && ok($1) && ok($2) && ok($3) && ok($4) && ok($5)) {printf " %d", NR}' "$accepted" 2>/dev/null || true)
  [ -z "$bad" ] || { echo "escaped-accepted.tsv: line$bad: not <row> <why it was not run> <who accepted it> <date> <release>, tab-separated, all five filled" >&2; exit 1; }
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
  # unchanged <row> <SKU pattern>: with --unchanged, its line's earlier gate must be trusted and have passed the row on such a
  # SKU, and `git diff --name-only` from that gate's set to this one must touch none of escaped-paths.tsv's paths for the row,
  # each repo at its own commit in a checkout here (cozy-creator: this one; cozy-runtime and packages in --checkouts). Prints where
  # it stands; anything it cannot read or verify says why and fails.
  # Read only with --unchanged: --release alone needs no checkout.
  top= && [ -z "$UNCHANGED" ] || top=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
  CHECKOUTS=${CHECKOUTS:-$top/..}
  declare -A repo=([cr]=$top [tf]=$top [rg]=$top [rt]=$CHECKOUTS/cozy-runtime [pk]=$CHECKOUTS/packages)
  paths=$(dirname "$0")/escaped-paths.tsv
  # at <repo key> <set json> [harness commit]: the set's commit in that repo (a version's +g<sha>, else its release tag).
  at() {
    local v
    case $1 in
      rg) echo "${3:-}"; return ;;
      pk) jq -r '.packages_ref // empty' <<< "$2"; return ;;
      cr) v=$(jq -r '.cozy // empty' <<< "$2") ;;
      rt) v=$(jq -r '.runtime // empty' <<< "$2") ;;
      tf) v=$(jq -r '.tensorfs // empty' <<< "$2") ;;
    esac
    if [[ $v =~ \+(dev\.)?g([0-9a-f]{7,40}) ]]; then echo "${BASH_REMATCH[2]}"
    elif [[ $v =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then [ "$1" = tf ] && echo "tensorfs-v$v" || echo "v$v"; fi
  }
  unchanged() {
    local row=$1 skus=$2 run line h d old= sku k p a b c files pats=() spec bs
    run=$(awk -F'\t' -v r="$row" '$1 == r {print $2; exit}' "$UNCHANGED")
    [[ $run =~ ^[0-9]+$ ]] || { echo "not in $UNCHANGED"; return 1; }
    line=$(awk -F'\t' -v r="$row" '$1 == r {print $2; exit}' "$paths" 2>/dev/null)
    [ -n "$line" ] || { echo "no escaped-paths.tsv line: it can only pass"; return 1; }
    # Asked of GitHub once a run, however many rows stand on it.
    h=$(cat "$tmp/u$run.sha" 2>/dev/null) || h=
    [ -n "$h" ] || { h=$(trusted "$run") && echo "$h" > "$tmp/u$run.sha"; } || { echo "$run: not a completed trusted release-gate dispatch"; return 1; }
    [ -d "$tmp/u$run" ] || gh run download "$run" -R cozy-creator/cozy-creator -p 'gate-*' -D "$tmp/u$run" >&2 || { echo "$run: no gate artifact"; return 1; }
    for d in "$tmp/u$run"/gate-*; do
      sku=$(cat "$d/baseline-key" 2>/dev/null); sku=${sku%-$(paste -sd- "$d/suites" 2>/dev/null)}
      [[ $sku == @($skus) ]] && grep -qE '^VERDICT (PASS|FAIL .*accepted by the lead)' "$d/verdict.txt" 2>/dev/null &&
        awk -F'\t' -v r="$row" '$1 == r && $2 == "PASS" {f = 1} END {exit !f}' "$d/expect.tsv" 2>/dev/null &&
        old=$(jq -S 'del(.label)' "$d/candidate.json") && break
    done
    [ -n "$old" ] || { echo "$run passed it on no SKU of $skus"; return 1; }
    # The line's patterns, macros expanded (never globbed here), then one pathspec list per repo key. A pattern of no known repo
    # or an unknown macro refuses: a path the check would skip is a path it never compared.
    expand() {
      local x m xs
      read -ra xs <<< "$1"
      for x in "${xs[@]}"; do
        if [[ $x == @* ]]; then m=$(awk -F'\t' -v m="$x" '$1 == m {print $2; exit}' "$paths"); expand "${m:-?:unknown-macro-${x#@}}"
        else echo "$x"; fi
      done
    }
    mapfile -t pats < <(expand "$line")
    for p in "${pats[@]}"; do
      a=${p#!}; case ${a%%:*} in cr | tf | rt | pk | rg) ;; *) echo "escaped-paths.tsv: '$p' names no repo"; return 1 ;; esac
    done
    for k in cr tf rt pk rg; do
      spec=()
      for p in "${pats[@]}"; do
        a=${p#!}; [ "${a%%:*}" = "$k" ] || continue
        if [ "$a" != "$p" ]; then spec+=(":(glob,exclude)${a#*:}"); else spec+=(":(glob)${a#*:}"); fi
      done
      [ ${#spec[@]} -gt 0 ] || continue
      a=$(at "$k" "$old" "$h")
      # Each gate's harness commit, or this set's commit: an empty one (a hand gate's) is an element, and refuses below.
      if [ "$k" = rg ]; then bs=("${heads[@]}"); else bs=("$(at "$k" "$first")"); fi
      for b in "${bs[@]}"; do
        [ -n "$a" ] && [ -n "$b" ] || { echo "$k: no commit for $([ -n "$a" ] && echo this set || echo "$run's set") (a hand gate names no harness commit)"; return 1; }
        for c in "$a" "$b"; do git -C "${repo[$k]}" rev-parse -q --verify "$c^{commit}" > /dev/null || { echo "$k: $c is not in ${repo[$k]}: fetch it"; return 1; }; done
        files=$(git -C "${repo[$k]}" diff --name-only "$a" "$b" -- "${spec[@]}") || { echo "$k: git diff ${a:0:10}..${b:0:10} failed"; return 1; }
        [ -z "$files" ] || { echo "changed since $run ($k ${a:0:10}..${b:0:10}): $(head -3 <<< "$files" | paste -sd' ')"; return 1; }
      done
    done
    echo "since $run on $(jq -r '[.cozy, .runtime, .tensorfs, .packages_ref[0:10]] | join(" · ")' <<< "$old"): none of its paths changed to this set"
  }
  while IFS=$'\t' read -r row skus issue when; do
    [ -n "$row" ] && [ "${row:0:1}" != "#" ] || continue
    WHY=
    if ! required "${when:-}"; then echo "escaped $row ($issue) not required: $WHY" >&2; continue; fi
    hit= failed=
    for d in "${dirs[@]}"; do
      awk -F'\t' -v r="$row" '$1 == r && $2 == "FAIL" {f = 1} END {exit !f}' "$d/expect.tsv" 2>/dev/null && failed=$d
      sku=$(cat "$d/baseline-key" 2>/dev/null); sku=${sku%-$(paste -sd- "$d/suites" 2>/dev/null)}
      [[ $sku == @($skus) ]] || continue
      awk -F'\t' -v r="$row" '$1 == r && $2 == "PASS" {f = 1} END {exit !f}' "$d/expect.tsv" 2>/dev/null && { hit="$sku: $d"; break; }
    done
    took=$(awk -F'\t' -v r="$row" -v v="$release" '($1 "") == r && ($5 "") == v && v != "" {print "not run: " $2 "; accepted by " $3 ", " $4 ", for " $5; exit}' \
      "$accepted" 2>/dev/null || true)
    same=
    if [ -z "$hit" ] && [ -z "$failed" ] && [ -n "$UNCHANGED" ] && awk -F'\t' -v r="$row" '$1 == r {f = 1} END {exit !f}' "$UNCHANGED"; then
      same=$(unchanged "$row" "$skus") || { missing="$missing $row($skus, $issue; not unchanged: $same)"; continue; }
    fi
    if [ -n "$hit" ]; then echo "escaped $row ($issue) PASS on $hit${WHY:+ (required: $WHY)}" >&2
    elif [ -n "$same" ]; then echo "escaped $row ($issue) UNCHANGED $same" >&2
    elif [ -n "$took" ] && [ -z "$failed" ]; then echo "escaped $row ($issue) ACCEPTED on no SKU ($skus): $took" >&2
    else missing="$missing $row($skus, $issue${WHY:+; $WHY}${took:+${failed:+; accepted as not run, but it failed in $failed}})"; fi
  done < "$(dirname "$0")/escaped.tsv"
  [ -z "$missing" ] || { echo "a release gate must pass every escaped bug's cell, or escaped-accepted.tsv accept it for $release; not passed here:$missing" >&2; exit 1; }
fi
echo "$first"
