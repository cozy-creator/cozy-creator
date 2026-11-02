"""Load and retain TensorFS's private Rust dependency without duplicating its core."""

from __future__ import annotations

import _ctypes
import ctypes
import importlib.metadata
import os
import struct
import sys
import threading
from contextlib import ExitStack
from pathlib import Path

_libraries: list[ctypes.CDLL] = []
_directories: list[object] = []

# Package aliases/reloads share one acquisition lock, rather than each protecting only
# its own module. Native participants using the canonical name are handled by the
# second loader lookup after an absolute-path acquisition.
_state = ctypes.__dict__.setdefault(
    "_tensorfs_varena_loader_v1", {"lock": threading.RLock()}
)
with _state["lock"]:
    if hasattr(os, "register_at_fork") and not _state.get("fork_registered"):
        os.register_at_fork(
            after_in_child=lambda: _state.__setitem__("lock", threading.RLock())
        )
        _state["fork_registered"] = True


class _Reference:
    """One loader reference acquired here, never a borrowed module handle."""

    def __init__(self, library):
        self.library = library

    def close(self):
        if self.library is None:
            return
        handle = self.library._handle
        if sys.platform == "win32":
            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            release = kernel.FreeLibrary
            release.argtypes = [ctypes.c_void_p]
            release.restype = ctypes.c_int
            if not release(handle):
                raise OSError(ctypes.get_last_error(), "FreeLibrary")
        else:
            _ctypes.dlclose(handle)
        self.library._handle = 0
        self.library = None

    def retain(self):
        library, self.library = self.library, None
        return library


def _loaded(name):
    """Acquire a reference to an already loaded canonical core, or return None."""
    if sys.platform == "win32":
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        acquire = kernel.GetModuleHandleExW
        acquire.argtypes = [
            ctypes.c_uint32,
            ctypes.c_wchar_p,
            ctypes.POINTER(ctypes.c_void_p),
        ]
        acquire.restype = ctypes.c_int
        handle = ctypes.c_void_p()
        # Flags 0 increment the reference count. GetModuleHandleW and UNCHANGED_REFCOUNT
        # would leave validation racing an existing native owner's FreeLibrary.
        if not acquire(0, name, ctypes.byref(handle)):
            error = ctypes.get_last_error()
            if error == 126:  # ERROR_MOD_NOT_FOUND
                return None
            raise OSError(error, "GetModuleHandleExW")
        return _Reference(ctypes.CDLL(name, handle=handle.value))
    try:
        return _Reference(ctypes.CDLL(name, mode=os.RTLD_NOLOAD | os.RTLD_GLOBAL))
    except OSError:
        return None


def _validate(library, expected):
    from ._native_identity import loaded_identity

    try:
        actual = loaded_identity(library, sys.platform == "win32")
    except (OSError, ValueError, AttributeError, struct.error) as error:
        raise ImportError(
            "cannot validate the loaded private Rust dependency"
        ) from error
    if actual != expected:
        raise ImportError(
            f"private Rust dependency mismatch: TensorFS needs {expected}, loaded Varena is {actual}; "
            "use matching native artifacts in a fresh process (public C ABI compatibility is separate)"
        )


def load() -> None:
    with _state["lock"]:
        _load()


def _load() -> None:
    from ._native_identity import file_identity

    names = {
        "linux": "libvarena.so",
        "darwin": "libvarena.dylib",
        "win32": "varena.dll",
    }
    name = names.get(sys.platform)
    if name is None:
        raise ImportError("Varena shared core is unavailable on this platform")
    directory = Path(__file__).parent / ".native"
    if not directory.is_dir():
        directory = Path(sys.prefix) / "lib" / "varena-v1"
        try:
            distribution = importlib.metadata.distribution("tensorfs")
            for file in distribution.files or ():
                if file.as_posix().endswith("lib/varena-v1/" + name):
                    directory = Path(distribution.locate_file(file)).parent
                    break
        except importlib.metadata.PackageNotFoundError:
            pass
    core = directory / name
    if not core.is_file():
        raise ImportError("TensorFS is missing its bundled private Varena dependency")
    try:
        expected = file_identity(core)
    except (OSError, ValueError, struct.error) as error:
        raise ImportError(
            "cannot validate the bundled private Rust dependency"
        ) from error

    with ExitStack() as cleanup:
        canonical = _loaded(name)
        if canonical is None:
            keep_directory = [False]
            if sys.platform == "win32":
                search = os.add_dll_directory(str(directory))
                cleanup.callback(lambda: None if keep_directory[0] else search.close())
            for path in sorted(directory.glob("*std-*")):
                if path.suffix in (".so", ".dylib", ".dll"):
                    runtime = _Reference(
                        ctypes.CDLL(str(path), mode=ctypes.RTLD_GLOBAL)
                    )
                    cleanup.callback(runtime.close)
            candidate = _Reference(ctypes.CDLL(str(core), mode=ctypes.RTLD_GLOBAL))
            cleanup.callback(candidate.close)
            # A native participant may have loaded the canonical name since the first
            # lookup. Validate the object that DT_NEEDED/imports will actually bind to.
            canonical = _loaded(name)
            if canonical is None:
                raise ImportError(
                    "loaded Varena is not available under its canonical dependency name"
                )
            cleanup.callback(canonical.close)
            candidate.close()  # Only our acquisition; existing native owners keep theirs.
            _validate(canonical.library, expected)
            if sys.platform == "win32":
                _directories.append(search)
                keep_directory[0] = True
        else:
            cleanup.callback(canonical.close)
            _validate(canonical.library, expected)
        _libraries.append(canonical.retain())
        # Temporary runtime references are released last. The canonical core owns the
        # dependencies it resolved, so an unused losing runtime copy is not retained.
