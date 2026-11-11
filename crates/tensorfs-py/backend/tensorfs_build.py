"""The PEP 517 backend: maturin, refusing at once when Rust is missing.

pip reaches it only when no prebuilt wheel fits. Unpatched, maturin would download a Rust
toolchain into a temporary directory and fail later, deep in a compile, on a missing linker.
"""

import platform
import shutil
import sys
from collections.abc import Mapping

import maturin
from maturin import build_sdist, get_requires_for_build_sdist  # noqa: F401

# PEP 517's config_settings.
ConfigSettings = Mapping[str, str | list[str]] | None


def _rust() -> None:
    if shutil.which("cargo") is None:
        sys.exit(
            f"tensorfs has no prebuilt wheel for CPython {platform.python_version()} on "
            f"{platform.system()} {platform.machine()}, so the installer is building it from source, which needs "
            "Rust (https://rustup.rs) and a C linker. Prebuilt wheels cover CPython 3.10 and later on "
            "Linux x86_64/aarch64 (glibc 2.17+), macOS arm64/x86_64 and Windows x86_64/arm64."
        )


def get_requires_for_build_wheel(config_settings: ConfigSettings = None) -> list[str]:
    _rust()
    requires: list[str] = maturin.get_requires_for_build_wheel(config_settings)
    return requires


def prepare_metadata_for_build_wheel(metadata_directory: str, config_settings: ConfigSettings = None) -> str:
    _rust()
    name: str = maturin.prepare_metadata_for_build_wheel(metadata_directory, config_settings)
    return name


def build_wheel(wheel_directory: str, config_settings: ConfigSettings = None, metadata_directory: str | None = None) -> str:
    _rust()
    _cli(config_settings)
    name: str = maturin.build_wheel(wheel_directory, config_settings, metadata_directory)
    return name


get_requires_for_build_editable = get_requires_for_build_wheel
prepare_metadata_for_build_editable = prepare_metadata_for_build_wheel


def build_editable(wheel_directory: str, config_settings: ConfigSettings = None, metadata_directory: str | None = None) -> str:
    _rust()
    _cli(config_settings)
    name: str = maturin.build_editable(wheel_directory, config_settings, metadata_directory)
    return name


def _cli(config_settings: ConfigSettings) -> None:
    from stage_cli import stage

    stage(maturin.get_maturin_pep517_args(config_settings))
