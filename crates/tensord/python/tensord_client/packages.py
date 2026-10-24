"""Generation records: a live project's metadata and its environment described by the
environment's own Runtime static reader, which never imports or runs authored modules."""
from __future__ import annotations

import os
import subprocess
import sys
import tomllib
from pathlib import Path

import msgspec

from . import layout
from .package_records import (
    ENVIRONMENT_DECODER, Dependency, DescribeEnvironment, DescribeFailed, Generation, PackageMetadata, Pyproject,
)


class PackageError(Exception):
    def __init__(self, code: str, detail: str):
        self.code = code
        super().__init__(detail)


def read_metadata(project: Path) -> PackageMetadata:
    parsed = msgspec.convert(tomllib.loads((project / "pyproject.toml").read_text()),
                             type=Pyproject)
    metadata = parsed.project
    applications = list(metadata.entry_points.application.values())
    if len(applications) != 1:
        raise PackageError("application_entrypoint_count",
                           f"{project / 'pyproject.toml'} names {len(applications)} cozy.application "
                           "entry points; exactly one is required")
    return PackageMetadata(metadata.name, metadata.version, applications[0])


# -S prevents authored .pth startup code; only the owned venv's regular site-packages paths
# are inserted before loading the installed SDK reader.
BOOTSTRAP = ("import pathlib,runpy,sys;"
             "root=pathlib.Path(sys.executable).absolute().parent.parent;"
             "sys.path[:0]=[str(p) for p in (*(root/'lib').glob('python*/site-packages'),root/'Lib'/'site-packages') if p.is_dir()];"
             "sys.argv=['cozy-runtime','machine-describe'];"
             "runpy.run_module('cozy_runtime.cli.main',run_name='__main__')")


def in_environment(interpreter: Path, request: msgspec.Struct, decoder: msgspec.json.Decoder):
    reply = decoder.decode(subprocess.check_output([str(interpreter), "-I", "-S", "-c", BOOTSTRAP],
                                                   input=msgspec.json.encode(request)))
    if isinstance(reply, DescribeFailed):
        raise PackageError(reply.code, reply.detail)
    return reply


def publish_generation(root: Path, metadata: PackageMetadata, interface: msgspec.Raw,
                       callees: dict[str, str] | None = None, sdk_fallback: str = "",
                       source_digest: str = "") -> Generation:
    """Record and publish the environment. `source_digest` is a live project's own code, whose
    installed files only point at it."""
    interpreter = layout.interpreter(root / "env")
    inventory = subprocess.check_output(["uv", "pip", "list", "--python", str(interpreter), "--format", "json"])
    dependencies = msgspec.json.decode(inventory, type=list[Dependency])
    environment = in_environment(interpreter, DescribeEnvironment(metadata.name, callees or {}), ENVIRONMENT_DECODER)
    generation = Generation(root.name, metadata.name, metadata.version, metadata.application,
                            str(interpreter), dependencies, interface,
                            source_digest or environment.source_digest, environment.callees, sdk_fallback)
    with (root / ".generation.json.new").open("wb") as output:
        output.write(msgspec.json.encode(generation))
        output.flush()
        os.fsync(output.fileno())
    (root / ".generation.json.new").replace(root / "generation.json")
    # Windows has no directory descriptor; NTFS journals the rename itself.
    for directory in [root, root.parent] if sys.platform != "win32" else []:
        fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
    return generation
