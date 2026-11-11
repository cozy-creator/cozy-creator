#!/usr/bin/env python3
"""Triton kernels against what launches them, by reading the source: no GPU, torch or Triton.

    static_check.py <dir>...     exits 1 naming each mismatch

For every `@triton.jit` function of each tree, every `TritonEntry` of it and every launch of it
(`launch(kernel, grid, *args)`, `_LAUNCH[i](kernel, grid, *args)`):
- a signature names exactly the kernel's parameters, each `tl.constexpr` as `constexpr` (the order Triton's
  launcher reads, which is run 28135's Xid 31, is `inventory`'s to check, on the real entries);
- a constexpr has a value and an aligned operand is a parameter;
- a launch passes one argument per parameter (a `*` spread counts as at least none);
- a grid is three dimensions.
A signature built by a helper (`_signature(kernel, ...)`) names the parameters by construction."""

import ast
import sys
from pathlib import Path
from typing import cast


def literal_keys(node: ast.expr) -> list[tuple[str, str]] | None:
    """The keys of a dict literal, `**dict.fromkeys((...), v)` included; None if not all literal."""
    if not isinstance(node, ast.Dict):
        return None
    keys: list[tuple[str, str]] = []
    for key, value in zip(node.keys, node.values, strict=True):
        if key is None:
            if (
                isinstance(value, ast.Call)
                and ast.unparse(value.func) == "dict.fromkeys"
                and isinstance(value.args[0], (ast.Tuple, ast.List))
                and all(isinstance(e, ast.Constant) for e in value.args[0].elts)
            ):
                # A TritonEntry signature's keys are str constants.
                keys += [
                    (cast(str, e.value), ast.unparse(value.args[1]).strip("'\""))
                    for e in value.args[0].elts
                    if isinstance(e, ast.Constant)
                ]
            else:
                return None
        elif isinstance(key, ast.Constant) and isinstance(value, ast.Constant):
            keys.append((key.value, str(value.value)))  # type: ignore[arg-type]  # a TritonEntry signature's keys are str constants
        elif isinstance(key, ast.Constant):
            keys.append((key.value, ast.unparse(value).strip("'\"")))  # type: ignore[arg-type]  # as above
        else:
            return None
    return keys


def jits(tree: ast.AST) -> dict[str, list[tuple[str, bool]]]:
    found: dict[str, list[tuple[str, bool]]] = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef) and any(
            ast.unparse(d) == "triton.jit" for d in node.decorator_list
        ):
            args = node.args.args
            found[node.name] = [
                (a.arg, a.annotation is not None and ast.unparse(a.annotation) == "tl.constexpr") for a in args
            ]
    return found


def name_of(node: ast.expr) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
    return None


def check(path: Path, known: dict[str, list[tuple[str, bool]]]) -> list[str]:
    tree = ast.parse(path.read_text())
    out: list[str] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        func = ast.unparse(node.func)
        at = f"{path}:{node.lineno}"
        if func.endswith("TritonEntry") and node.args:
            kernel = name_of(node.args[0])
            if kernel not in known:
                continue
            params = known[kernel]  # type: ignore[index]  # in known, so not None
            keys = literal_keys(node.args[1]) if len(node.args) > 1 else None
            if keys is not None:
                names = [k for k, _ in keys]
                want = [p for p, _ in params]
                if set(names) != set(want):
                    out.append(f"{at}: {kernel}: signature {sorted(set(names) ^ set(want))} differ from its parameters")
                for param, constexpr in params:
                    typed = dict(keys).get(param)
                    if typed is not None and (typed == "constexpr") != constexpr:
                        out.append(f"{at}: {kernel}: {param} is {typed} in the signature, {'a constexpr' if constexpr else 'a runtime argument'} in the kernel")
            if len(node.args) > 3 and isinstance(node.args[3], ast.Tuple):
                for e in node.args[3].elts:
                    if isinstance(e, ast.Constant) and e.value not in [p for p, _ in params]:
                        out.append(f"{at}: {kernel}: aligned {e.value!s} is not a parameter")
        launched = func == "launch" or func.endswith("launch") or func.startswith("_LAUNCH[") or func.startswith("launch[")
        if launched and len(node.args) >= 2:
            kernel = name_of(node.args[0])
            if kernel not in known:
                continue
            params = known[kernel]  # type: ignore[index]  # in known, so not None
            passed = node.args[2:]
            spread = any(isinstance(a, ast.Starred) for a in passed)
            count = sum(not isinstance(a, ast.Starred) for a in passed)
            if (spread and count > len(params)) or (not spread and count != len(params)):
                out.append(f"{at}: launch of {kernel}: {count}{'+' if spread else ''} arguments, {len(params)} parameters")
            grid = node.args[1]
            if isinstance(grid, ast.Tuple) and len(grid.elts) != 3:
                out.append(f"{at}: launch of {kernel}: a grid of {len(grid.elts)} dimensions")
    return out


def mismatches(roots: list[str] | list[Path]) -> tuple[list[str], int]:
    """Each mismatch in these trees, and how many jit functions it read."""
    files = [f for root in roots for f in Path(root).rglob("*.py") if "research" not in f.parts and ".venv" not in f.parts and "triton" in f.read_text(errors="replace")]
    known: dict[str, list[tuple[str, bool]]] = {}
    for f in files:
        known.update(jits(ast.parse(f.read_text())))
    return [line for f in files for line in check(f, known)], len(known)


if __name__ == "__main__":
    found, read = mismatches(sys.argv[1:])
    print(*found, sep="\n")
    print(f"{read} jit functions: {len(found)} mismatches")
    raise SystemExit(1 if found else 0)
