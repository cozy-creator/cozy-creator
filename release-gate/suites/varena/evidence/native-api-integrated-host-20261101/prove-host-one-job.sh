#!/usr/bin/env bash
# Real local CPU-only proof. OUTPUT must be a new durable directory.
set -euo pipefail
cd /home/fidika/cozy/.worktrees/cozy-creator/xs138-native-integrated
out=$(realpath -m "${1:?usage: prove-host.sh OUTPUT}")
if [ -e "$out" ]; then echo "output already exists: $out" >&2; exit 2; fi
mkdir -p "$out"
export CARGO_BUILD_JOBS=1
export CARGO_TARGET_DIR=/home/fidika/cozy_v2/outputs/xs138-native-api-integrated/target
export VARENA_LIB_DIR="/home/fidika/cozy_v2/outputs/xs138-native-api-integrated/target/debug/deps"
# Build the internal Rust cohort together. Do not rebuild varena as a primary
# Cargo target afterward: that selects a different Rust dependency-linkage mode.
nice -n 19 cargo build --locked -p tensorfs-py --features extension-module -p tensorfs \
    -p varena-mixed-client --lib --bin tfs -j 1 > "$out/build.log" 2>&1
# The public client deliberately has no Cargo dependency on the core implementation.
# Its external -lvarena must be available before its final link starts.
nice -n 19 cargo build --locked -p varena-client --example engine -j 1 >> "$out/build.log" 2>&1
nice -n 19 uv run --no-project --with patchelf python crates/varena/bundle.py \
    --target /home/fidika/cozy_v2/outputs/xs138-native-api-integrated/target/debug --sdk "$out/sdk" > "$out/bundle.json"
native="$out/sdk/python/tensorfs/.native"
cc -std=c11 -Wall -Wextra -Werror -fPIC -shared release-gate/suites/varena/public-api/native.c \
    -Icrates/varena/include -L"$native" -lvarena -o "$out/mixed-c.so"
cc -std=c11 -Wall -Wextra -Werror crates/varena/examples/native_engine.c \
    -Icrates/varena/include -L"$native" -Wl,-rpath-link,"$native" -lvarena -ldl -o "$out/native-engine"
printf '#include "varena.h"\nint main() { return VR_ABI_VERSION != 1; }\n' | \
    c++ -x c++ -std=c++17 -Wall -Wextra -Werror -Icrates/varena/include -o "$out/header-cxx" -
export PYTHONPATH="$out/sdk/python"
export LD_LIBRARY_PATH="$native"
for order in native-first python-first; do
    python3 release-gate/suites/varena/public-api/mixed.py --c "$out/mixed-c.so" \
        --rust /home/fidika/cozy_v2/outputs/xs138-native-api-integrated/target/debug/libvarena_mixed_client.so --order "$order" > "$out/$order.json"
done
"$out/native-engine" --host > "$out/c-engine-host.json"
/home/fidika/cozy_v2/outputs/xs138-native-api-integrated/target/debug/examples/engine --host > "$out/rust-engine-host.json"
/home/fidika/cozy_v2/outputs/xs138-native-api-integrated/target/debug/tfs version > "$out/tfs-version.txt"
"$out/header-cxx"
readelf -d "$native/libvarena.so" > "$out/core-dependencies.txt"
readelf -d "$out/sdk/python/tensorfs/_ext.so" > "$out/extension-dependencies.txt"
sha256sum "$native/"*.so "$out/sdk/python/tensorfs/_ext.so" > "$out/artifacts.sha256"
printf 'CPU-only mixed-binding proof passed: %s\n' "$out"
