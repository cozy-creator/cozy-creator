#!/usr/bin/env python3
"""Add immutable native dependency installation to dist's generated installers."""

import sys
from pathlib import Path


def replace(path, old, new):
    text = path.read_text()
    if "VARENA_PRIVATE_DEPENDENCIES" in text:
        return
    if text.count(old) != 1:
        raise ValueError(f"installer layout changed: {path}")
    path.write_text(text.replace(old, new))


root = Path(sys.argv[1])
anchor = '    local _arch="$5"\n'
replace(
    root / "cozy-installer.sh",
    anchor,
    anchor
    + """    # VARENA_PRIVATE_DEPENDENCIES: stage before replacing any executable.
    for _native in "$_src_dir"/libvarena-private-* "$_src_dir"/libstd-private-*; do
        [ -f "$_native" ] || continue
        _native_name=$(basename "$_native")
        if [ -e "$_install_dir/$_native_name" ]; then
            cmp -s "$_native" "$_install_dir/$_native_name" || err "different immutable dependency $_native_name"
        else
            ensure cp "$_native" "$_install_dir/$_native_name"
        fi
    done
""",
)
anchor = "  # Just copy the binaries from the temp location to the install dir\n"
replace(
    root / "cozy-installer.ps1",
    anchor,
    """  # VARENA_PRIVATE_DEPENDENCIES: old DLL names remain for rollback/running owners.
  foreach ($candidate in $artifacts["bin_paths"]) {
    $native_dir = Split-Path -Path $candidate -Parent
    foreach ($native in Get-ChildItem -Path "$native_dir/*-private-*.dll") {
      $placed = Join-Path $dest_dir $native.Name
      if (Test-Path $placed) {
        if ((Get-FileHash $placed).Hash -ne (Get-FileHash $native.FullName).Hash) { throw "different immutable native dependency $placed" }
      } else { Copy-Item $native.FullName -Destination $placed -ErrorAction Stop }
    }
  }
"""
    + anchor,
)
anchor = "    install_binary_aliases!\n"
replace(
    root / "cozy.rb",
    anchor,
    """    # VARENA_PRIVATE_DEPENDENCIES: loader paths are relative to the executable.
    bin.install Dir["libvarena-private-*", "libstd-private-*"]
"""
    + anchor,
)
