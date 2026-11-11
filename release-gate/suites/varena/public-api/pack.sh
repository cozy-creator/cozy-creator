#!/usr/bin/env bash
# pack.sh OUT: this suite as one program directory for release-gate/programs.sh (xs-174): public_api.py, and the clients'
# sources in OUT/tree, which the pod builds against the core it has installed.
set -euo pipefail
out=$(realpath -m "${1:?usage: pack.sh OUT}")
cd "$(dirname "$0")/../../../.."
suite=release-gate/suites/varena/public-api
tree=$out/tree
rm -rf "$out"
mkdir -p "$tree/$suite" "$tree/crates/varena"
cp "$suite/public_api.py" "$out"
cp -r "$suite"/{cells.py,mixed.py,native.c,sized_engine.c,streams.py} "$tree/$suite"
cp -r crates/varena/include crates/varena/examples "$tree/crates/varena"
cp -r crates/varena-client "$tree/crates"
cat > "$tree/Cargo.toml" <<TOML
[workspace]
resolver = "3"
members = ["crates/varena-client"]

[workspace.package]
edition = "2024"
rust-version = "1.99"
license = "MIT"
publish = false

[workspace.dependencies]
libloading = "0.9"

[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"
TOML
