# derived-state.sh <reset|settle|census>: on a rented pod's root shell (machines.sh `pod_derived`), what the machine keeps from
# earlier runs besides its weights and installs.
# - reset clears it, before an update restarts the machine:
#   - TensorFS derived pages (LoRA bakes, in the OS's temporary storage: `/var/tmp/cozy-derived-<uid>/<boot>`)
#   - the stage memo, learned memory peaks, the kernel store and JIT caches (`/var/lib/cozy/tensord/gpu/...`, and any
#     project's stage memo)
#   - the page cache of the store's files.
#   Kept: model weights, package environments, installed builds.
# - settle, after the restart, clears again what the ended executors wrote after `reset`: a finished run's derived pages
#   are written behind it, so the last run's persister was still writing when `reset` ran (44038983465: 16 pages).
#   Kernels and JIT caches stay: the new machine's own executors may be building theirs.
# - census prints one line: files left of each, and the host's page cache. Every mode prints it.
# An arm that finds another arm's state reads it as its own (44006261838: the candidate read the 0.25.0 arm's Turbo LoRA
# bakes, Flux-2 load 79 s -> 8.8 s).
set -u
G=/var/lib/cozy/tensord/gpu S=/var/lib/tensorfs D=$(echo /var/tmp/cozy-derived-*/*/)
memos=$({ echo "$G/stage-memo"; find /var/lib/cozy -maxdepth 7 -type d -name stage-memo -not -path "*/generations/*" -not -path "*/editable/*" 2>/dev/null; } | sort -u)
files() { n=0; for d in "$@"; do [ -d "$d" ] && n=$((n + $(find "$d" -mindepth 1 -type f 2>/dev/null | wc -l))); done; echo "$n"; }
if [ "${1:-census}" = reset ] || [ "${1:-census}" = settle ]; then
  # shellcheck disable=SC2086 # boot directories, none with spaces
  rm -rf $D "$S/derived"; D=  # the store's: where a TensorFS before cl-1318 kept them
  caches=; [ "$1" = settle ] || caches="$G/kernels $G/jit"
  # shellcheck disable=SC2086 # one path a line, none with spaces
  for d in $memos "$G/peaks" $caches; do [ -d "$d" ] && find "$d" -mindepth 1 -delete 2>/dev/null; done
  sync
  python3 - "$S" <<'PY'
import os, sys
for top, _, names in os.walk(sys.argv[1]):
    for name in names:
        try:
            fd = os.open(os.path.join(top, name), os.O_RDONLY)
        except OSError:
            continue
        try:
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        finally:
            os.close(fd)
PY
fi
# shellcheck disable=SC2086
# shellcheck disable=SC2086
echo "derived $(files $D) memo $(files $memos) peaks $(files "$G/peaks") kernels $(files "$G/kernels") jit $(files "$G/jit") cached_gib $(awk '/^Cached:/ {printf "%.1f", $2 / 1048576}' /proc/meminfo)"
