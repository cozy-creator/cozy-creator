#!/usr/bin/env bash
# The one wheel build (tfs-051). Builds the `tfs` binary and the extension module from
# the SAME tree at the SAME version and ships both in one wheel: the binary is staged
# into wheel-data/scripts/ (pyproject `data`), so installers put it on PATH beside the
# module.
#
#   scripts/build-wheel.sh [--out <dir>] [--torch-python <python>] [maturin build args...]
#
# Portable builds use the existing CI/release manylinux environment for BOTH
# cargo and maturin. A host build can explicitly pass --compatibility linux.
set -euo pipefail
cd "$(dirname "$0")/.."
py="${PYTHON:-python3}"

out=target/wheels
torch_python=
args=()
while [ $# -gt 0 ]; do
    case "$1" in
        --out) out="$2"; shift ;;
        --torch-python) torch_python="$2"; shift ;;
        --plane|--no-default-features)
            echo "tensorfs ships one full Python+CLI wheel; $1 is not a supported wheel option" >&2
            exit 2 ;;
        *) args+=("$1") ;;
    esac
    shift
done

if [ -n "$torch_python" ]; then
    nice -n 19 "$torch_python" shim/build.py
fi

if [ "$(uname -s)" != Darwin ]; then
    if [ ! -f ../tensorfs/kernels/aot/catalog.rs ]; then
        uv run --no-project --python 3.12 --with triton==3.8.0 python kernels/build.py
    fi
    export TENSORFS_REQUIRE_AOT=1
fi

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
nice -n 19 "$py" backend/stage_cli.py --release ${args[@]+"${args[@]}"}

nice -n 19 maturin build --release --locked --out "$out" ${args[@]+"${args[@]}"}

# build.rs and pyproject include the private libraries directly, also for PEP 517
# and direct maturin. There is no post-build wheel repair that can swap the core.
