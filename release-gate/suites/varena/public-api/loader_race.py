"""Host-only private loader regression using real SDK images, never a fake allocator.

PACKAGE and OTHER_PACKAGE are isolated SDK copies containing the loader under test,
its identity helper, the actual private extension, and actual native dependencies.
The scheduling hooks only interleave real loader calls; they do not change results.
"""

from __future__ import annotations

import _ctypes
import argparse
import ctypes
import importlib.util
import json
import os
import sys
import threading
import types
from pathlib import Path
from typing import Any, NotRequired, TypedDict

sys.dont_write_bytecode = True


class Args(argparse.Namespace):
    package: Path
    other_package: Path
    mode: str
    loader_source: Path | None
    no_extension: bool
    c_client: Path | None


class Attempt(TypedDict):  # one load's outcome, as printed
    accepted: bool
    package: NotRequired[str]
    extension_import: NotRequired[bool]
    identity: NotRequired[int | None]
    error: NotRequired[str]
    native_owner_closed: NotRequired[bool]
    old_c_client: NotRequired[bool]
    fork_lock_reinitialized: NotRequired[bool]
    cuda_fork_qualification: NotRequired[bool]


def loader(package: Path, index: int, source: Path | None) -> tuple[str, types.ModuleType]:
    name = f"varena_loader_proof_{index}"
    parent = types.ModuleType(name)
    parent.__path__ = [str(package)]
    sys.modules[name] = parent
    for stem in ("_native_identity", "_native"):
        module_name = f"{name}.{stem}"
        spec = importlib.util.spec_from_file_location(
            module_name, (source or package) / f"{stem}.py"
        )
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        sys.modules[module_name] = module
        spec.loader.exec_module(module)
        if stem == "_native":
            module.__file__ = str(package / "_native.py")
            return name, module
    raise AssertionError("loader not imported")


def extension(package: Path, name: str) -> types.ModuleType:
    (library,) = package.glob("_ext*.so")
    spec = importlib.util.spec_from_file_location(f"{name}._ext", library)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def mapped_cores() -> list[str]:
    maps = Path("/proc/self/maps").read_text().splitlines()
    assert not any("/libcuda.so" in line for line in maps), "host proof loaded CUDA"
    return sorted({line.split()[-1] for line in maps if line.endswith("/libvarena.so")})


def close_owned(library: ctypes.CDLL) -> None:
    # Independent loader primitive: this is the native participant's own reference.
    _ctypes.dlclose(library._handle)
    library._handle = 0


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument("other_package", type=Path)
    parser.add_argument(
        "--mode",
        choices=["concurrent", "native-race", "existing", "matching", "fork"],
        required=True,
    )
    parser.add_argument(
        "--loader-source", type=Path, help="retained pre-fix source for the red control"
    )
    parser.add_argument(
        "--no-extension",
        action="store_true",
        help="never enter private Rust in the red control",
    )
    parser.add_argument("--c-client", type=Path, help="unchanged older C-only consumer")
    args = parser.parse_args(namespace=Args())
    package, other = args.package.resolve(), args.other_package.resolve()
    name, first = loader(package, 0, args.loader_source)
    results: list[Attempt] = []
    actual_cdll = ctypes.CDLL

    def attempt(label: str, package: Path, name: str, module: types.ModuleType) -> None:
        try:
            module.load()
            identity = None
            if not args.no_extension:
                identity = extension(package, name)._varena_core_identity()
            results.append(
                {
                    "package": label,
                    "accepted": True,
                    "extension_import": not args.no_extension,
                    "identity": identity,
                }
            )
        except ImportError as error:
            assert "private Rust dependency mismatch" in str(error), str(error)
            assert f"{name}._ext" not in sys.modules
            results.append({"package": label, "accepted": False, "error": str(error)})

    if args.mode == "concurrent":
        other_name, second = loader(other, 1, args.loader_source)
        barrier = threading.Barrier(2)

        def scheduled(name: str | None, *a: Any, **kw: Any) -> ctypes.CDLL:  # type: ignore[explicit-any]  # CDLL's own arguments, passed on
            try:
                return actual_cdll(name, *a, **kw)
            except OSError:
                if name == "libvarena.so" and kw.get("mode", 0) & os.RTLD_NOLOAD:
                    # A serialized loader cannot rendezvous with the second call.
                    # The bound only ends our scheduling pause, not the invariant check.
                    try:
                        barrier.wait(0.25)
                    except threading.BrokenBarrierError:
                        pass
                raise

        ctypes.CDLL = scheduled  # type: ignore[misc, assignment]  # the race hooks CDLL itself
        threads = [
            threading.Thread(target=attempt, args=("first", package, name, first)),
            threading.Thread(
                target=attempt, args=("second", other, other_name, second)
            ),
        ]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(10)
        assert not any(thread.is_alive() for thread in threads)
        assert len(results) == 2, results
    elif args.mode == "native-race":
        native: list[ctypes.CDLL] = []

        def scheduled(path: str | None, *a: Any, **kw: Any) -> ctypes.CDLL:  # type: ignore[misc, explicit-any]  # CDLL's own arguments, passed on; each mode's hook has its own shape
            if path == str(package / ".native/libvarena.so") and not native:
                # Another native participant wins after the initial NOLOAD check.
                native.append(
                    actual_cdll(
                        str(other / ".native/libvarena.so"), mode=ctypes.RTLD_GLOBAL
                    )
                )
            return actual_cdll(path, *a, **kw)

        ctypes.CDLL = scheduled  # type: ignore[misc, assignment]  # the race hooks CDLL itself
        attempt("first", package, name, first)
        assert native and not results[0]["accepted"], results
        native[0].vr_v1_abi_version.restype = ctypes.c_uint32
        assert native[0].vr_v1_abi_version() == 1, (
            "losing cleanup released the native owner's reference"
        )
        assert len(mapped_cores()) == 1, mapped_cores()
        close_owned(native[0])
        assert not mapped_cores(), "mismatch cleanup leaked a canonical reference"
        ctypes.CDLL = actual_cdll  # type: ignore[misc]  # CDLL itself again
        attempt("retry_after_native_close", package, name, first)
        assert results[-1]["accepted"]
    elif args.mode == "existing":
        owner = actual_cdll(
            str(other / ".native/libvarena.so"), mode=ctypes.RTLD_GLOBAL
        )
        attempt("first", package, name, first)
        assert not results[0]["accepted"]
        assert owner.vr_v1_abi_version() == 1
        close_owned(owner)
        assert not mapped_cores(), "refused import retained its temporary reference"
        attempt("retry_after_native_close", package, name, first)
        assert results[-1]["accepted"]
    elif args.mode == "matching":
        owner = actual_cdll(
            str(package / ".native/libvarena.so"), mode=ctypes.RTLD_GLOBAL
        )
        first.load()
        close_owned(owner)
        module = extension(package, name)
        identity = module._varena_core_identity()
        if args.c_client:
            client = actual_cdll(str(args.c_client.resolve()), mode=ctypes.RTLD_GLOBAL)
            client.mixed_c_identity.restype = ctypes.c_uint64
            assert client.mixed_c_identity() == identity
            assert client.mixed_c_error() == 1
            assert client.mixed_c_contract() == 1
        results.append(
            {
                "accepted": True,
                "native_owner_closed": True,
                "extension_import": True,
                "identity": identity,
                "old_c_client": bool(args.c_client),
            }
        )
    else:
        # This tests the Python acquisition lock only, not CUDA/private-VM fork use.
        held, release = threading.Event(), threading.Event()

        def hold() -> None:
            with first._state["lock"]:
                held.set()
                release.wait(10)

        thread = threading.Thread(target=hold)
        thread.start()
        assert held.wait(10)
        pid = os.fork()
        if pid == 0:
            import signal

            signal.alarm(5)
            first.load()
            os._exit(0)
        _, status = os.waitpid(pid, 0)
        release.set()
        thread.join()
        assert os.waitstatus_to_exitcode(status) == 0, status
        first.load()
        results.append(
            {
                "accepted": True,
                "fork_lock_reinitialized": True,
                "cuda_fork_qualification": False,
            }
        )
    cores = mapped_cores()
    print(
        json.dumps(
            {"mode": args.mode, "results": results, "mapped_cores": cores, "gpu": False}
        )
    )
    assert len(cores) == 1, "separate allocator images remain mapped"
    assert any(result["accepted"] for result in results), results


if __name__ == "__main__":
    main()
