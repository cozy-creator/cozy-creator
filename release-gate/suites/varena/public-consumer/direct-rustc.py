"""Compile only the fixture wrapper against an already-frozen dependency cohort.

Run inside the same pinned manylinux/Rust image after the target owner releases it.
Mount the target and Rust toolchain read-only. The default prints a recipe; --run
executes rustc with no Cargo invocation and verifies the core/extension stay exact.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from pathlib import Path


def sha(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def dependency(profile, extension, name):
    selected = [item for item in extension["deps"] if item[1] == name]
    assert len(selected) == 1, (name, selected)
    # Cargo1.99's dependency fingerprint is serialized as a little-endian u64 in
    # the companion fingerprint file. Verify an exact match, never newest mtime.
    wanted = selected[0][3].to_bytes(8, "little").hex()
    found = []
    for record in (profile / ".fingerprint").glob(f"{name}-*/lib-{name}.json"):
        if record.with_suffix("").read_text().strip() == wanted:
            metadata = json.loads(record.read_text())
            if metadata["rustc"] == extension["rustc"]:
                found.append((record, metadata))
    assert len(found) == 1, (name, wanted, [str(item[0]) for item in found])
    return found[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--profile", type=Path, required=True, help="frozen /target/release directory"
    )
    parser.add_argument("--extension-fingerprint", type=Path, required=True)
    parser.add_argument("--expected-core-sha256", required=True)
    parser.add_argument("--expected-extension-sha256", required=True)
    parser.add_argument("--rustc", type=Path, default=Path("/rust/bin/rustc"))
    parser.add_argument(
        "--source", type=Path, default=Path(__file__).parent / "native/src/lib.rs"
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--run", action="store_true")
    args = parser.parse_args()
    assert not args.output.exists() and not args.receipt.exists(), (
        "preserve previous artifacts"
    )
    profile = args.profile.resolve()
    core, extension = profile / "deps/libvarena.so", profile / "lib_ext.so"
    assert sha(core) == args.expected_core_sha256
    assert sha(extension) == args.expected_extension_sha256
    metadata = json.loads(args.extension_fingerprint.read_text())
    assert metadata["rustflags"] == [], (
        "audit additional producer flags before extending this recipe"
    )
    serde_record, _ = dependency(profile, metadata, "serde_json")
    varena_record, _ = dependency(profile, metadata, "varena")
    serde_suffix = serde_record.parent.name.removeprefix("serde_json-")
    serde = profile / "deps" / f"libserde_json-{serde_suffix}.rlib"
    assert serde.is_file()
    rustc = str(args.rustc)
    std_dir = Path(
        subprocess.check_output([rustc, "--print", "target-libdir"], text=True).strip()
    )
    std = list(std_dir.glob("libstd-*.so"))
    assert len(std) == 1
    command = [
        rustc,
        "--crate-name",
        "varena_public_probe",
        "--edition=2024",
        "--crate-type",
        "cdylib",
        str(args.source.resolve()),
        "-C",
        "opt-level=3",
        "-C",
        "codegen-units=16",
        "-C",
        "strip=symbols",
        "-C",
        "embed-bitcode=no",
        "-C",
        "prefer-dynamic",
        "-L",
        f"dependency={profile / 'deps'}",
        "-L",
        f"native={profile / 'deps'}",
        "-L",
        f"native={std_dir}",
        "--extern",
        f"varena={core}",
        "--extern",
        f"serde_json={serde}",
        "-C",
        f"link-arg=-Wl,-rpath-link,{profile / 'deps'}",
        "-C",
        f"link-arg=-Wl,-rpath-link,{std_dir}",
        "-C",
        "link-arg=-Wl,-rpath,$ORIGIN/lib",
        "-o",
        str(args.output.resolve()),
    ]
    receipt = {
        "command": command,
        "source_sha256": sha(args.source),
        "extension_fingerprint": str(args.extension_fingerprint),
        "extension_fingerprint_sha256": sha(args.extension_fingerprint),
        "varena_fingerprint": str(varena_record),
        "serde_fingerprint": str(serde_record),
        "serde_rlib_sha256": sha(serde),
        "core_sha256": sha(core),
        "extension_sha256": sha(extension),
        "std_name": std[0].name,
        "std_sha256": sha(std[0]),
        "rustc": subprocess.check_output([rustc, "-vV"], text=True),
        "executed": False,
        "scope": "one fixture cdylib; no Cargo or dependency rebuild",
    }
    if args.run:
        subprocess.run(command, check=True)
        assert (
            sha(core) == receipt["core_sha256"]
            and sha(extension) == receipt["extension_sha256"]
        )
        receipt.update(executed=True, output_sha256=sha(args.output))
    args.receipt.write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt, indent=2))


if __name__ == "__main__":
    main()
