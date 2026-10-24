#!/usr/bin/env bash
# TEMPORARY proof branch (proof/se085-cl662), never merged: dispatched as e2e.yaml's cli suite with
# creator_ref=<this commit>, ref=<a PR head>. The candidate TensorD is the PR's (.src). Each part asserts only
# when .src carries its change; otherwise it prints its numbers as the control.
#   se-085 (#1998): an import of many small tensors reports its pack as a `committing` stage.
#   cl-662 (#2014): a download cut part-way, then run again, records `bytes.local` (what the store held) and
#   fetches only the rest; the PR's release-gate check.py judges it a resume, master's a restart.
. "$(dirname "$0")/../lib.sh"
parse "$@"
SRC=$PWD/.src CREATOR=$PWD/.creator W=${RUNNER_TEMP:-/tmp}/proof SUM=$OUT/proof.txt
mkdir -p "$W"
say() { echo "$*" | tee -a "$SUM"; }
has_1998=''; grep -q 'fn commit_with' "$SRC/crates/tensorfs/src/writer.rs" && has_1998=1
has_2014=''; grep -q '"local": landed.local' "$SRC/crates/tensord/src/runs.rs" && has_2014=1

step disk; sudo rm -rf /usr/share/dotnet /usr/local/lib/android /opt/ghc /opt/hostedtoolcache/CodeQL; df -h / | tee -a "$SUM"
step install; install_cozy
user_services
step "the local machine"; machine_ready
step candidates; candidates
say "candidate: $(cozy --version) (src $(git -C "$SRC" rev-parse HEAD)); #1998 change: ${has_1998:-no}; #2014 change: ${has_2014:-no}"
nproc | xargs echo cpus: | tee -a "$SUM"; free -g | tee -a "$SUM"

step "se-085: an import of 8192 small tensors (2 GiB) reports its commit"
uv run -q --no-project --with numpy --with safetensors python -I - "$W/many.safetensors" 8192 <<'PY'
import sys
import numpy as np
from safetensors.numpy import save_file
rng = np.random.default_rng()
save_file({f"experts.{i}.weight": rng.standard_normal((128, 1024)).astype(np.float16) for i in range(int(sys.argv[2]))}, sys.argv[1])
PY
ls -l "$W/many.safetensors" | tee -a "$SUM"
cozy --json model download "$W/many.safetensors" local/e2e-many > "$OUT/import.ndjson" 2> "$OUT/import.err" \
  || check "model download of many.safetensors failed: $(tail -c 300 "$OUT/import.err")"
n=$(grep -o '"number":[0-9]*' "$OUT/import.ndjson" | head -1 | cut -d: -f2 || true)
[ -z "$n" ] || cozy --json run show "$n" > "$OUT/import.json" 2>> "$OUT/import.err" || true
rm -f "$W/many.safetensors"
if python3 -I - "$OUT/import.ndjson" "$n" <<'PY' | tee -a "$SUM"; then :; else [ -z "$has_1998" ] || check "se-085: the import's commit reported no progress"; fi
import json, sys
stages = {}
for line in open(sys.argv[1]):
    try: e = json.loads(line)
    except ValueError: continue
    p = e.get('progress') or {}
    if e.get('type') == 'run.progress' and p.get('stage'):
        stages.setdefault(p['stage'], []).append((e.get('at_ms'), p.get('bytes_done'), p.get('bytes_total')))
for s, v in stages.items():
    print(f"import run {sys.argv[2]}: stage {s!r}: {len(v)} events, first {v[0][1:]} last {v[-1][1:]}")
c = stages.get('committing') or []
done = [d for _, d, _ in c]
ok = len(set(done)) >= 2 and done == sorted(done) and c[-1][1] == c[-1][2] and c[-1][2] > 0
if c and c[0][0] and c[-1][0]:
    print(f"committing: {len(set(done))} distinct byte counts over {(c[-1][0] - c[0][0]) / 1000:.1f} s")
print(("PASS" if ok else "NO") + " se-085: the pack reports its bytes as it goes (committing, increasing, ends at its total)")
sys.exit(0 if ok else 1)
PY

step "cl-662: sdxl --lane 16 cut at 35%, then downloaded again"
T=$OUT/suite-tensorfs; mkdir -p "$T"
cozy --json model download sdxl --lane 16 --on local -y > "$T/download-cut.ndjson" 2> "$T/download-cut.err" & pid=$!
n='' moved=''
while kill -0 "$pid" 2> /dev/null; do
  [ -n "$n" ] || n=$(grep -o '"number":[0-9]*' "$T/download-cut.ndjson" | head -1 | cut -d: -f2 || true)
  moved=$(python3 -I "$SRC/release-gate/suites/tensorfs/check.py" --past "$T/download-cut.ndjson" downloading 0.35 || true)
  if [ -n "$n" ] && [ -n "$moved" ]; then cozy run cancel "$n" -y > "$T/cancel.txt" 2>&1 || true; echo "$moved" > "$T/download-cut.cut"; break; fi
  sleep 1
done
wait "$pid" || true
printf 'download-cut\t%s\t\n' "${n:-none}" >> "$T/steps.tsv"
[ -z "$n" ] || cozy --json run show "$n" > "$T/download-cut.json" 2>> "$T/download-cut.err" || true
say "cut: run ${n:-none} $(jq -r .status "$T/download-cut.json" 2> /dev/null) at ${moved:-?} B held"
t0=$(date +%s%3N)
cozy --json model download sdxl --lane 16 --on local -y > "$T/download-resume.ndjson" 2> "$T/download-resume.err" \
  || check "the resumed download failed: $(tail -c 300 "$T/download-resume.err")"
m=$(grep -o '"number":[0-9]*' "$T/download-resume.ndjson" | head -1 | cut -d: -f2 || true)
printf 'download-resume\t%s\t%s\n' "${m:-none}" "$(( $(date +%s%3N) - t0 ))" >> "$T/steps.tsv"
[ -z "$m" ] || { cozy --json run show "$m" > "$T/download-resume.json" 2>> "$T/download-resume.err"; cozy run show "$m" > "$T/download-resume.txt" 2>&1; } || true
if python3 -I - "$T" "${moved:-0}" <<'PY' | tee -a "$SUM"; then :; else [ -z "$has_2014" ] || check "cl-662: the resume's record does not show what was held"; fi
import json, os, sys
t, moved = sys.argv[1], int(sys.argv[2])
rec = json.load(open(os.path.join(t, 'download-resume.json')))
b = [m['bytes'] for m in (rec.get('result') or {}).get('models') or [] if 'bytes' in m]
hub, vol = (sum(x.get(k) or 0 for x in b) for k in ('hub', 'volume'))
local = sum(x.get('local') or 0 for x in b) if b and all('local' in x for x in b) else None
total = 0
for line in open(os.path.join(t, 'download-resume.ndjson')):
    try: e = json.loads(line)
    except ValueError: continue
    p = e.get('progress') or {}
    if e.get('type') == 'run.progress' and 'downloading' in (p.get('stage') or ''): total = p.get('bytes_total') or total
print(f"resume: run {rec.get('number')} {rec.get('status')}: bytes {b}; total {total}; the cut had held {moved}")
ok = local is not None and moved > 0 and local >= 0.8 * moved and hub + vol <= total - local + (64 << 20)
if local is not None:
    print(f"hub + volume = {hub + vol}; total - local = {total - local}; difference {hub + vol - (total - local)} B")
print(("PASS" if ok else "NO") + " cl-662: the resume held what the cut landed (bytes.local) and fetched only the rest")
sys.exit(0 if ok else 1)
PY
# The release gate's verdict on these records: the change's check.py, then master's.
python3 -I - "$OUT" "$SRC/release-gate/suites/tensorfs/check.py" "$CREATOR/release-gate/suites/tensorfs/check.py" <<'PY' | tee -a "$SUM" || true
import importlib.util, sys
for label, path in (('change', sys.argv[2]), ('master', sys.argv[3])):
    spec = importlib.util.spec_from_file_location('check_' + label, path)
    m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
    fails, notes = m.check({'out': sys.argv[1], 'baseline': {}})
    for line in fails + notes:
        if 'download-resume' in line:
            print(f"{label} check.py: {'FAIL' if line in fails else 'pass'}: {line}")
PY
df -h / | tee -a "$SUM"

step logs; (cd ~/.cozy && find . -name '*.log' -size -20M | tar -czf - -T - > "$OUT/cozy-logs.tgz") || true
finish "proof se-085/cl-662"
