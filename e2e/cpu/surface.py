"""Static: every TensorFS name the Runtime reads exists in the candidate TensorFS. A CPU request never opens varena's
pages (`residency.Memory` opens them only for CUDA), so a lost `Pages.stats` cannot fail a CPU cell; this reads the
Runtime's source instead and asks the installed TensorFS.

    surface.py <cozy_runtime package dir>

Prints `PASS|FAIL - - surface <seconds>s[: what is missing]` (static, not a run)."""

from __future__ import annotations

import ast
import importlib
import sys
import time
from pathlib import Path

#: Receivers the Runtime names its varena pages by: `memory.pages.stats()`, `self.pages.drain()`.
PAGES = ("tensorfs.varena._runtime", "Pages")


def aliases(tree: ast.Module) -> dict[str, str]:
    """Each local name bound to a TensorFS module: `vt` -> `tensorfs.varena.torch`."""
    names: dict[str, str] = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name.split(".")[0] == "tensorfs":
                    names[alias.asname or alias.name.split(".")[0]] = alias.name if alias.asname else "tensorfs"
        elif isinstance(node, ast.ImportFrom) and (node.module or "").split(".")[0] == "tensorfs":
            for alias in node.names:
                full = f"{node.module}.{alias.name}"
                try:
                    importlib.import_module(full)
                except ImportError:
                    continue
                names[alias.asname or alias.name] = full
    return names


def submodule(module: str, name: str) -> bool:
    try:
        importlib.import_module(f"{module}.{name}")
    except ImportError:
        return False
    return True


def typing_only(tree: ast.Module) -> set[int]:
    """The import nodes under `if TYPE_CHECKING:`: stubs' names, never read at run time."""
    return {id(child) for node in ast.walk(tree) if isinstance(node, ast.If) and "TYPE_CHECKING" in ast.unparse(node.test)
            for statement in node.body for child in ast.walk(statement)}


def missing(path: Path) -> list[str]:
    tree = ast.parse(path.read_text(), str(path))
    names = aliases(tree)
    skipped = typing_only(tree)
    gone: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom) and (node.module or "").split(".")[0] == "tensorfs" and id(node) not in skipped:
            module = importlib.import_module(node.module or "tensorfs")
            gone += [f"{node.module}.{a.name}" for a in node.names
                     if not hasattr(module, a.name) and not submodule(node.module or "tensorfs", a.name)]
        if not isinstance(node, ast.Attribute):
            continue
        if isinstance(node.value, ast.Name) and node.value.id in names:
            module = importlib.import_module(names[node.value.id])
            if not hasattr(module, node.attr):
                gone.append(f"{names[node.value.id]}.{node.attr}")
        receiver = node.value
        if (isinstance(receiver, ast.Name) and receiver.id == "pages") or (
                isinstance(receiver, ast.Attribute) and receiver.attr == "pages"):
            cls = getattr(importlib.import_module(PAGES[0]), PAGES[1])
            if not hasattr(cls, node.attr):
                gone.append(f"{PAGES[1]}.{node.attr}")
    return [f"{path.name}:{name}" for name in gone]


def main() -> int:
    started = time.monotonic()
    root = Path(sys.argv[1])
    gone = sorted({name for path in sorted(root.rglob("*.py")) for name in missing(path)})
    took = f"{time.monotonic() - started:.1f}s"
    print(f"FAIL - - surface {took}: {', '.join(gone)[:300]}" if gone else f"PASS - - surface {took}", flush=True)
    return 1 if gone else 0


if __name__ == "__main__":
    sys.exit(main())
