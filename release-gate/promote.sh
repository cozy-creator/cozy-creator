#!/bin/bash
# promote.sh [--dry-run] [--phases LIST] [--images '<profile>=sha256:<hex> …'] [--packages DIR] [--packages-ref REF] <release-gate run id> | --record <URL> <gate dir>...:
# the one step that hands a passed candidate set to users, the same bytes the gate ran (tracker design ci-v2, pre-release
# and promotion). Testing is manual: the evidence is the passing release-gate.yml run the release agent dispatched, or the
# hand gate's dirs with the URL where their result is recorded. verify.sh first; then, each waiting for the last to succeed:
#   PyPI      TensorFS tensorfs-publish.yaml and the Runtime runtime-publish.yaml prebuilt, with gate_run: the prereleases' files;
#   smoke     e2e cpu: the release cozy locks a gated source package fresh against PyPI's Runtime and runs it from that lock
#             (hard stop); its committed lock, the RC's, isn't the question;
#   cozy      cozy-promote.yaml: the v<V> prerelease becomes Latest (`cozy update`, the installer) and Homebrew's;
#   models    each held private release the set uses (candidate.json `models`) published again with its checkpoints: public.
#             Before packages: the Hub refuses a package release whose slot default is not a public model release
#             (tensorhub #1627, th-412: `422 package_release.default_not_public`);
#   packages  `task publish -- --gated <packages_ref>` in the packages checkout (--packages, on master with the RC merged):
#             the gated source's finals, only the version changed (Paul 11-13: software before the packages that need it), each
#             with its benchmarks (cl-1024): `bench.py references` of the newest stored baseline (`bench.py baseline` after the
#             gate), else benchmarks-seed.json, into each package's benchmarks.json, which its publish attaches;
#   images    tensorhub worker-images-publish.yaml promote: the digests staged from those wheels (--images) become the tags.
#             Last (cl-1164): a new package runs on an old image (its machine moves its Runtime up), an old package never on a
#             new one (a release refuses a Runtime its cap leaves out), so new pods boot the new Runtime once its packages exist.
# --phases pypi,smoke,cozy,models,packages,images (default all): only those, always in that order, so the release can stop
# between them (a check that needs PyPI's Runtime before Latest moves). A phase whose earlier phases this run leaves out stands
# on the world as they leave it, read before anything moves: PyPI serving the set's Runtime and TensorFS by the gate's digests,
# a successful smoke (e2e.yaml) on the cozy release's tag, the cozy release Latest, each held model public, every gated
# package's final on the Hub. Unread (PyPI, GitHub or the Hub not answering) is not done.
# Whatever the phases (so --packages for any), a set that holds packages is refused before anything moves unless every model a
# slot of theirs loads by default is public, or held in `models` for the models phase (defaults.sh, cl-1243: th-412's 404s).
# --packages-ref REF: the packages are released from REF, not the gated packages_ref, only when version-only.py finds every
# difference a package's version alone (each changed pyproject.toml's `version =` line, its uv.lock's own row): the gate's
# result stands for them (Paul 11-18: test only what changed). Anything else refuses before anything moves, naming the file.
# The Hub pin (/v1/software) stays the release owner's, after this.
set -euo pipefail
DRY= IMAGES= PKGS= RELREF= ALL=pypi,smoke,cozy,models,packages,images
PHASES=$ALL
while [ $# -gt 0 ]; do
  case $1 in
    --dry-run) DRY=1; shift ;;
    --phases) PHASES=$2; shift 2 ;;
    --images) IMAGES=$2; shift 2 ;;
    --packages) PKGS=$(cd "$2" && pwd); shift 2 ;;
    --packages-ref) RELREF=$2; shift 2 ;;
    *) break ;;
  esac
done
HERE=$(cd "$(dirname "$0")" && pwd)
IFS=, read -r -a named <<< "$PHASES"
[ ${#named[@]} -gt 0 ] || { echo "--phases takes $ALL: none named" >&2; exit 2; }
for p in "${named[@]}"; do [[ ,$ALL, == *",$p,"* ]] || { echo "--phases takes $ALL: no '$p'" >&2; exit 2; }; done
want() { [[ ,$PHASES, == *",$1,"* ]]; }
if [ "${1:-}" = --record ]; then
  gate=${2:-}; shift 2
  [[ $gate == https://* ]] && [ $# -gt 0 ] || { echo "--record <URL> <gate dir>...: the hand gate's dirs and where their result is recorded" >&2; exit 2; }
else
  [ $# -eq 1 ] && [[ $1 =~ ^[0-9]+$ ]] || { sed -n '2,4s/^# \{0,1\}//p' "$0" >&2; exit 2; }
  gate=$1
fi
set_=$("$HERE/verify.sh" "$@")
field() { jq -r ".$1 // empty" <<< "$set_"; }
ref=$(field packages_ref) gated=
if [ -n "$RELREF" ]; then
  [ -n "$ref" ] && [ -n "$PKGS" ] || { echo "--packages-ref needs a set that holds packages and --packages <checkout>" >&2; exit 2; }
  rel=$(git -C "$PKGS" rev-parse -q --verify "$RELREF^{commit}") || { echo "$PKGS has no $RELREF: fetch it" >&2; exit 2; }
  git -C "$PKGS" cat-file -e "$ref^{commit}" 2> /dev/null || { echo "$PKGS has no gated $ref: fetch it" >&2; exit 2; }
  if [ "$rel" != "$(git -C "$PKGS" rev-parse "$ref^{commit}")" ]; then
    moved=$(python3 -I "$HERE/version-only.py" "$PKGS" "$ref" "$rel") ||
      { echo "$moved" >&2; echo "--packages-ref ${rel:0:10} differs from the gated ${ref:0:10} by more than versions: nothing promoted" >&2; exit 1; }
    echo "packages: gated on ${ref:0:10}, released from ${rel:0:10} (versions only: $(paste -sd';' <<< "$moved" | sed 's/;/; /g'))"
    gated=$ref ref=$rel
  fi
fi
mapfile -t models < <(jq -r '.models[]?' <<< "$set_")
# What it needs, checked before anything moves.
if want images && [ -n "$(field runtime_sha256)$(field tensorfs_sha256)$(field cozy_sha256)" ] && [ -z "$IMAGES" ]; then
  echo "the set moves cozy, the Runtime or TensorFS: --images '<profile>=sha256:… …', the worker images staged from it" >&2; exit 2
fi
if [ -n "$ref" ]; then
  [ -n "$PKGS" ] && [ -f "$PKGS/Taskfile.yaml" ] || { echo "the set holds packages at $ref: --packages <a packages checkout>" >&2; exit 2; }
  ! { want packages || want models || want images; } || [ "$(git -C "$PKGS" rev-parse --abbrev-ref HEAD)" = master ] ||
    { echo "$PKGS is not on master" >&2; exit 2; }
  "$HERE/defaults.sh" "$PKGS" "$ref" "${models[@]}" ||
    { echo "not every default model of the packages at $ref is public or held (above): nothing promoted" >&2; exit 1; }
fi
if want models; then
  for m in "${models[@]}"; do
    cozy --json model show "$m" > /dev/null 2>&1 || { echo "the Hub does not hold $m: nothing promoted" >&2; exit 1; }
  done
fi
# done_<phase>: whether the world is as that phase leaves it, read now. WHY says what is missing.
WHY=
on_pypi() {  # <distribution> <version> <sha256>: PyPI serves that release with a file of that digest
  curl -fsSL --retry 3 "https://pypi.org/pypi/$1/$2/json" 2> /dev/null | jq -e --arg s "$3" 'any(.urls[]; .digests.sha256 == $s)' > /dev/null ||
    { WHY="PyPI does not serve $1 $2 with the gate's digest ${3:0:12}…"; return 1; }
}
done_pypi() {
  { [ -z "$(field tensorfs_sha256)" ] || on_pypi tensorfs "$(field tensorfs)" "$(field tensorfs_sha256)"; } &&
    { [ -z "$(field runtime_sha256)" ] || on_pypi cozy-runtime "$(field runtime)" "$(field runtime_sha256)"; }
}
done_smoke() {  # a passing e2e.yaml dispatch on the cozy release's tag, where the smoke runs: the default path stops on a red one
  local passed
  [ -n "$(field runtime_sha256)" ] && [ -n "$ref" ] && [ -n "$(field cozy)" ] || return 0
  passed=$(gh run list -R cozy-creator/cozy-creator -w e2e.yaml -b "v$(field cozy)" -e workflow_dispatch -L 20 --json conclusion \
    -q '[.[] | select(.conclusion == "success")] | length' 2> /dev/null) || { WHY="GitHub's e2e runs on v$(field cozy) did not answer"; return 1; }
  [ "${passed:-0}" -gt 0 ] || { WHY="no passing smoke (e2e.yaml) on v$(field cozy)"; return 1; }
}
done_cozy() {
  local latest
  [ -n "$(field cozy_sha256)" ] || return 0
  latest=$(gh api repos/cozy-creator/cozy-creator/releases/latest -q .tag_name 2> /dev/null || true)
  [ "$latest" = "v$(field cozy)" ] || { WHY="cozy v$(field cozy) is not Latest (${latest:-unread} is)"; return 1; }
}
done_packages() {  # every package at the gated ref has its final (its version less any prerelease part) or later on the Hub
  local listed names p t v final latest missing= checked=0
  [ -n "$ref" ] || return 0
  git -C "$PKGS" cat-file -e "$ref^{commit}" 2> /dev/null || git -C "$PKGS" fetch -q origin "$ref" ||
    { WHY="$PKGS can't fetch $ref"; return 1; }
  names=$(git -C "$PKGS" ls-tree --name-only "$ref") || { WHY="$PKGS can't read $ref"; return 1; }
  listed=$(cozy package ls --json --limit 200) || { WHY="the Hub's package listing did not answer"; return 1; }
  for p in $names; do
    t=$(git -C "$PKGS" show "$ref:$p/pyproject.toml" 2> /dev/null) && grep -q cozy.application <<< "$t" || continue
    checked=$((checked + 1)) v=$(awk -F'"' '/^version = /{print $2; exit}' <<< "$t")
    final=$(sed -E 's/(a|b|rc)[0-9]+$//; s/\.dev[0-9]+$//' <<< "$v")
    latest=$(jq -r --arg p "_/$p" --arg q "$p" '[.packages[] | select(.package == $p or .package == $q)][0].latest_release // empty' <<< "$listed")
    [[ $latest =~ ^[0-9]+(\.[0-9]+)*$ ]] && [ "$(printf '%s\n' "$final" "$latest" | sort -V | head -1)" = "$final" ] || missing="$missing $p $final"
  done
  [ "$checked" -gt 0 ] || { WHY="no package at $ref"; return 1; }
  [ -z "$missing" ] || { WHY="no final on the Hub for:$missing"; return 1; }
}
done_models() {
  local m
  for m in "${models[@]}"; do
    [ "$(cozy --json model show "$m" 2> /dev/null | jq -r '.visibility // empty')" = public ] || { WHY="$m is not public"; return 1; }
  done
}
# Each phase this run takes stands on every earlier one it leaves out.
left= ordered= stood=
for p in ${ALL//,/ }; do
  if want "$p"; then
    ordered="$ordered${ordered:+, }$p"
    for q in $left; do
      [[ " $stood " == *" $q "* ]] && continue
      "done_$q" || { echo "--phases: $p stands on $q, which has not happened: $WHY. Nothing promoted" >&2; exit 1; }
      stood="$stood $q"
    done
  else left="$left $p"; fi
done
echo "phases: $ordered${stood:+ (standing on$stood, read now)}${DRY:+; dry run}"
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
if want pypi && [ -n "$(field tensorfs_sha256)" ]; then
  go cozy-creator/cozy-creator tensorfs-publish.yaml --ref "tensorfs-v$(field tensorfs)" -f gate_run="$gate"
fi
if want pypi && [ -n "$(field runtime_sha256)" ]; then
  v=$(field runtime)
  sums=$(gh release download "v$v" -R cozy-creator/cozy-runtime -p SHA256SUMS -O - | tr '\n' ' ')
  go cozy-creator/cozy-runtime runtime-publish.yaml --ref master -f release_tag="v$v" -f mode=prebuilt -f gated_sha256sums="$sums" -f gate_run="$gate"
fi
# The release cozy, before anything a user follows (0.26.0): until its Runtime is on PyPI it can't lock a source package
# floored on it, so no gate ran that. Now it locks one from the gated commit against PyPI's Runtime and runs it on CPU from that
# lock (e2e cpu --lock fresh, the cozy-publish prerelease, Runtime and TensorFS from PyPI). Images and Latest wait for it; a failure
# stops here.
if want smoke && [ -n "$(field runtime_sha256)" ] && [ -n "$ref" ] && [ -n "$(field cozy)" ]; then
  tag="v$(field cozy)"
  pub=$(gh run list -R cozy-creator/cozy-creator -w cozy-publish.yaml -b "$tag" -L 5 --json databaseId,conclusion -q '[.[] | select(.conclusion == "success")][0].databaseId // empty')
  [ -n "$pub" ] || { echo "no successful cozy-publish run for $tag: nothing after PyPI promoted" >&2; exit 1; }
  sha=$(gh api "repos/cozy-creator/cozy-creator/commits/$tag" -q .sha)
  go cozy-creator/cozy-creator e2e.yaml --ref "$tag" -f repo=cozy-creator -f ref="$sha" -f creator_ref="$sha" -f stack_ref="$sha" \
    -f select=cpu -f cpu_only=birefnet -f publish_run="$pub" -f runtime="$(field runtime)" -f tensorfs="$(field tensorfs)" -f packages_ref="$ref" -f cpu_lock_fresh=true
fi
c=$(field cozy)
if want cozy && [ -n "$(field cozy_sha256)" ] && [ "$(gh release view "v$c" -R cozy-creator/cozy-creator --json isPrerelease -q .isPrerelease 2>/dev/null)" = true ]; then
  go cozy-creator/cozy-creator cozy-promote.yaml -f tag="v$c" -f gate="$gate"
fi
# The held models first: a package release whose slot default is still private is refused (th-412).
if want models; then
  for m in "${models[@]}"; do
    shown=$(cozy --json model show "$m")
    [ "$(jq -r '.visibility // empty' <<< "$shown")" != public ] || { echo "  $m is public already"; continue; }
    mapfile -t checkpoints < <(jq -r '[.. | objects | select(has("lane") and has("checkpoint")) | .checkpoint] | unique[]' <<< "$shown")
    run cozy model publish "${m%@*}" "${m#*@}" "${checkpoints[@]}" -y
  done
fi
if want packages && [ -n "$ref" ]; then
  base=$(ls "$HERE"/baselines/*.json 2> /dev/null | sort -V | tail -1 || true)
  run uv run -q --no-project --with numpy python -I "$HERE/bench.py" references "${base:-$HERE/benchmarks-seed.json}" --packages "$PKGS"
  if [ -n "$DRY" ]; then (cd "$PKGS" && run task publish -- --dry-run --gated "$ref")
  else (cd "$PKGS" && run task publish -- --gated "$ref"); fi
fi
! want images || [ -z "$IMAGES" ] || go cozy-creator/tensorhub worker-images-publish.yaml --ref master -f mode=promote -f digests="$IMAGES"
echo "promoted (${PHASES//,/, }): $(jq -c . <<< "$set_")${gated:+, packages released from ${ref:0:10} (gated on ${gated:0:10})}.$(want images && echo ' Then the Hub pin, to this set.')"
