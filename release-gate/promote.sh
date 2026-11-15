#!/bin/bash
# promote.sh [--dry-run] [--images '<profile>=sha256:<hex> …'] [--packages DIR] <release-gate run id> | --record <URL> <gate dir>...:
# the one step that hands a passed candidate set to users, the same bytes the gate ran (tracker design ci-v2, pre-release
# and promotion). Testing is manual: the evidence is the passing release-gate.yml run the release agent dispatched, or the
# hand gate's dirs with the URL where their result is recorded. verify.sh first; then, each waiting for the last to succeed:
#   PyPI      TensorFS tensorfs-publish.yaml and the Runtime runtime-publish.yaml prebuilt, with gate_run: the prereleases' files;
#   images    tensorhub worker-images-publish.yaml promote: the digests staged from those wheels (--images) become the tags;
#   cozy      cozy-promote.yaml: the v<V> prerelease becomes Latest (`cozy update`, the installer) and Homebrew's;
#   packages  `task publish -- --gated <packages_ref>` in the packages checkout (--packages, on master with the RC merged):
#             the gated source's finals, only the version changed (Paul 11-13: software before the packages that need it);
#   models    each held private release the set uses (candidate.json `models`) published again with its checkpoints: public.
# The Hub pin (/v1/software) stays the release owner's, after this.
set -euo pipefail
DRY= IMAGES= PKGS=
while [ $# -gt 0 ]; do
  case $1 in
    --dry-run) DRY=1; shift ;;
    --images) IMAGES=$2; shift 2 ;;
    --packages) PKGS=$(cd "$2" && pwd); shift 2 ;;
    *) break ;;
  esac
done
HERE=$(cd "$(dirname "$0")" && pwd)
if [ "${1:-}" = --record ]; then
  gate=${2:-}; shift 2
  [[ $gate == https://* ]] && [ $# -gt 0 ] || { echo "--record <URL> <gate dir>...: the hand gate's dirs and where their result is recorded" >&2; exit 2; }
else
  [ $# -eq 1 ] && [[ $1 =~ ^[0-9]+$ ]] || { sed -n '2,4s/^# \{0,1\}//p' "$0" >&2; exit 2; }
  gate=$1
fi
set_=$("$HERE/verify.sh" "$@")
field() { jq -r ".$1 // empty" <<< "$set_"; }
ref=$(field packages_ref)
mapfile -t models < <(jq -r '.models[]?' <<< "$set_")
# What it needs, checked before anything moves.
if [ -n "$(field runtime_sha256)$(field tensorfs_sha256)$(field cozy_sha256)" ] && [ -z "$IMAGES" ]; then
  echo "the set moves cozy, the Runtime or TensorFS: --images '<profile>=sha256:… …', the worker images staged from it" >&2; exit 2
fi
if [ -n "$ref" ]; then
  [ -n "$PKGS" ] && [ -f "$PKGS/Taskfile.yaml" ] || { echo "the set holds packages at $ref: --packages <a packages checkout>" >&2; exit 2; }
  [ "$(git -C "$PKGS" rev-parse --abbrev-ref HEAD)" = master ] || { echo "$PKGS is not on master" >&2; exit 2; }
fi
for m in "${models[@]}"; do
  cozy --json model show "$m" > /dev/null 2>&1 || { echo "the Hub does not hold $m: nothing promoted" >&2; exit 1; }
done
# go <repo> <workflow> <args...>: the workflow dispatched, then followed to its end; a failure stops the promotion.
go() {
  local repo=$1 workflow=$2 since id; shift 2
  echo "+ gh workflow run $workflow -R $repo $*"
  [ -z "$DRY" ] || return 0
  since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  gh workflow run "$workflow" -R "$repo" "$@"
  for _ in $(seq 20); do
    sleep 15
    id=$(gh run list -R "$repo" -w "$workflow" -e workflow_dispatch -L 5 --json databaseId,createdAt \
      -q "[.[] | select(.createdAt >= \"$since\")] | last | .databaseId // empty")
    [ -z "$id" ] || break
  done
  [ -n "$id" ] || { echo "$workflow's run did not appear on $repo: nothing after it promoted" >&2; exit 1; }
  gh run watch "$id" -R "$repo" --exit-status --interval 60 > /dev/null || { echo "$workflow run $id failed: nothing after it promoted" >&2; exit 1; }
  echo "  $workflow run $id passed"
}
run() { echo "+ $*"; [ -n "$DRY" ] || "$@"; }
if [ -n "$(field tensorfs_sha256)" ]; then
  go cozy-creator/cozy-creator tensorfs-publish.yaml --ref "tensorfs-v$(field tensorfs)" -f gate_run="$gate"
fi
if [ -n "$(field runtime_sha256)" ]; then
  v=$(field runtime)
  sums=$(gh release download "v$v" -R cozy-creator/cozy-runtime -p SHA256SUMS -O - | tr '\n' ' ')
  go cozy-creator/cozy-runtime runtime-publish.yaml --ref master -f release_tag="v$v" -f mode=prebuilt -f gated_sha256sums="$sums" -f gate_run="$gate"
fi
[ -z "$IMAGES" ] || go cozy-creator/tensorhub worker-images-publish.yaml --ref master -f mode=promote -f digests="$IMAGES"
c=$(field cozy)
if [ -n "$(field cozy_sha256)" ] && [ "$(gh release view "v$c" -R cozy-creator/cozy-creator --json isPrerelease -q .isPrerelease 2>/dev/null)" = true ]; then
  go cozy-creator/cozy-creator cozy-promote.yaml -f tag="v$c" -f gate="$gate"
fi
if [ -n "$ref" ]; then
  if [ -n "$DRY" ]; then (cd "$PKGS" && run task publish -- --dry-run --gated "$ref")
  else (cd "$PKGS" && run task publish -- --gated "$ref"); fi
fi
for m in "${models[@]}"; do
  shown=$(cozy --json model show "$m")
  [ "$(jq -r '.visibility // empty' <<< "$shown")" != public ] || { echo "  $m is public already"; continue; }
  mapfile -t checkpoints < <(jq -r '[.. | objects | select(has("lane") and has("checkpoint")) | .checkpoint] | unique[]' <<< "$shown")
  run cozy model publish "${m%@*}" "${m#*@}" "${checkpoints[@]}" -y
done
echo "promoted: $(jq -c . <<< "$set_"). Then the Hub pin, to this set."
