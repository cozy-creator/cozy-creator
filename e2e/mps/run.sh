#!/usr/bin/env bash
# The `mps` suite: the local path on an Apple GPU as a person takes it, on GitHub's macos-15 runner. GitHub's
# macOS VMs give torch 2.14 a working GPU on macOS 15 only (on 26+ its Metal 4 kernels compute wrong there,
# pytorch#196582; torch 2.15+ refuses the virtual GPU). BiRefNet must serve on the GPU; SDXL must serve or,
# past the GPU's working set, be refused naming it (memory_exhausted).
#   e2e/mps/run.sh --out DIR [--tensord F|V] [--tensorfs F|V] [--runtime F|V] [--packages DIR]
. "$(dirname "$0")/../lib.sh"
parse "$@"
[ "$(uname -s)" = Darwin ] || fail "the mps suite runs on macOS"
# Qualify the unchanged development packages; published releases retain their platform gates.
PACKAGES=${PACKAGES:-${PACKAGES_MASTER:-}}
# Preserve the service's actual readiness error when candidate installation exits early.
trap 'mps_exit=$?; if [ "$mps_exit" != 0 ]; then cozy machine logs local tensord --lines 0 --json > "$OUT/machine-failure.ndjson" 2>&1 || true; fi' EXIT

step install; install_cozy
step "the local machine"; machine_ready
sysctl -n machdep.cpu.brand_string hw.memsize | tee "$OUT/apple-hardware.txt"
sw_vers -productVersion | tee "$OUT/macos-version.txt"
uv run -q --no-project python - "$OUT" "$CAND_COZY" "$CAND_TFS" "$CAND_RT" "$PACKAGES" <<'PY'
import hashlib, json, platform, subprocess, sys
from pathlib import Path
out = Path(sys.argv[1])
artifacts = {}
for name, value in zip(("cozy", "tensorfs", "runtime"), sys.argv[2:5], strict=True):
    path = Path(value)
    if path.is_file():
        with path.open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        artifacts[name] = {"filename": path.name, "sha256": digest, "bytes": path.stat().st_size}
    else:
        artifacts[name] = {"release": value}
manifest = {"os": platform.platform(), "arch": platform.machine(), "artifacts": artifacts}
if packages := sys.argv[5]:
    manifest["packages"] = {"path": packages, "commit": subprocess.check_output(["git", "-C", packages, "rev-parse", "HEAD"], text=True).strip()}
(out / "candidate-artifacts.json").write_text(json.dumps(manifest, indent=2) + "\n")
PY
step candidates; candidates
# An Apple GPU, unified.
grep -q '"family": *"apple' "$OUT/machine.json" && grep -q '"memory_class": *"unified"' "$OUT/machine.json" \
  || fail "the local machine reports no unified Apple GPU"

# This marker is emitted only by Runtime's actual MPS residency branch, never CPU/CUDA.
# A visible Apple GPU alone does not prove the model used it.
served_mps() {
  jq -e '.status == "completed" and
    any(.measurements.memory.stats.varena[]?;
      .stats.measurement == "observed_current" and .stats.allocated > 0)' \
    "$OUT/$1.json" > /dev/null || check "$1: no actual MPS allocation record; CPU fallback is not a pass"
}

step "owned MPS checkpoint, alias and refill"
if cell mps-residency "$E2E/../release-gate/suites/execution/mps-residency/gpu" seed:=209103 --gpus=1; then
  jq -e '.result.passed == true' "$OUT/mps-residency.json" > /dev/null || check "MPS residency cell returned no proof"
fi

step "BiRefNet on the Apple GPU"
dog=$(picture dog1.jpg)
if cell birefnet "$(birefnet)" --file "image=$dog" --gpus=1; then
  served_mps birefnet
  white_background "$(output birefnet webp)" "$dog" || check "birefnet: not a background removal"
fi

step "SDXL on the Apple GPU, or refused past its working set"
sdxl=sdxl/generate
[ -z "$PACKAGES" ] || sdxl=./$PACKAGES/sdxl/generate
if ALLOW=memory_exhausted cell sdxl "$sdxl" prompt="a lighthouse on a basalt cliff at dusk, oil painting" seed:=4021 steps:=12 --gpus=1; then
  if [ ! -e "$OUT/sdxl.refused" ]; then
    served_mps sdxl
    [ -n "$(output sdxl png)$(output sdxl webp)$(output sdxl jpg)" ] || check "sdxl completed with no image"
  fi
fi

step logs; cozy machine logs local > "$OUT/machine.log" 2>&1 || true
finish "mps: birefnet run $(cat "$OUT/birefnet.number"), sdxl run $(cat "$OUT/sdxl.number")$([ -e "$OUT/sdxl.refused" ] && echo ' (refused past the working set)')"
