#!/usr/bin/env bash
# pack.sh OUT: this suite as one package. Local code travels as its own directory, and the pod builds
# the clients from these sources against the core it has installed.
set -euo pipefail
out=$(realpath -m "${1:?usage: pack.sh OUT}")
cd "$(dirname "$0")/../../../.."
suite=release-gate/suites/varena/public-api
tree=$out/ci_varena_public/tree
rm -rf "$out"
mkdir -p "$tree/$suite" "$tree/crates/varena"
cp "$suite/package.toml" "$suite/pyproject.toml" "$out"
cp "$suite/ci_varena_public/__init__.py" "$out/ci_varena_public"
cp -r "$suite"/{Cargo.toml,src,cells.py,mixed.py,native.c,sized_engine.c,streams.py} "$tree/$suite"
cp -r crates/varena/include crates/varena/examples "$tree/crates/varena"
cp -r crates/varena-client "$tree/crates"
cat > "$tree/Cargo.toml" <<TOML
[workspace]
resolver = "3"
members = ["crates/varena-client", "$suite"]

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
