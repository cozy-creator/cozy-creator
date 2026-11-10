"""A fixture's waivers (`WAIVE` in tiny/<package>.py): each `module:function` named in CPU_E2E_WAIVE returns None
instead of running once its module is imported, and each `module:NAME` in CPU_E2E_PATCH (`PATCH`) takes the given
literal: a release geometry a package states as a constant, shrunk. drive.py puts this module and a `.pth` importing it
in the package environment's site-packages, so it holds in the executor and every rank it spawns (`python -I` reads no
PYTHONPATH). Only release-geometry pins a tiny model cannot meet are waived; the coverage table lists each as GPU-only."""

from __future__ import annotations

import ast
import importlib.abc
import importlib.machinery
import importlib.util
import json
import os
import sys
from collections.abc import Sequence
from types import ModuleType


def _waived(*_args: object, **_kwargs: object) -> None:
    return None


class _Loader(importlib.abc.Loader):
    def __init__(self, inner: importlib.abc.Loader, functions: list[str], values: dict[str, str]) -> None:
        self.inner, self.functions, self.values = inner, functions, values

    def create_module(self, spec: importlib.machinery.ModuleSpec) -> ModuleType | None:
        return self.inner.create_module(spec)

    def exec_module(self, module: ModuleType) -> None:
        self.inner.exec_module(module)
        for function in self.functions:
            if not callable(getattr(module, function, None)):
                raise RuntimeError(f"cpu-e2e waives {module.__name__}:{function}, which the package no longer has")
            setattr(module, function, _waived)
        for name, literal in self.values.items():
            # `NAME`, or a dotted path into it (`Class.__init__.__defaults__`: a constructor's defaults).
            *path, last = name.split(".")
            owner: object = module
            for part in path:
                owner = getattr(owner, part, None)
            if owner is None or not hasattr(owner, last):
                raise RuntimeError(f"cpu-e2e patches {module.__name__}:{name}, which the package no longer has")
            setattr(owner, last, ast.literal_eval(literal))


class _Finder(importlib.abc.MetaPathFinder):
    def __init__(self, names: dict[str, list[str]], values: dict[str, dict[str, str]]) -> None:
        self.names, self.values = names, values

    def find_spec(self, fullname: str, path: Sequence[str] | None,
                  target: ModuleType | None = None) -> importlib.machinery.ModuleSpec | None:
        if fullname not in self.names and fullname not in self.values:
            return None
        sys.meta_path.remove(self)
        try:
            spec = importlib.util.find_spec(fullname)
        finally:
            sys.meta_path.insert(0, self)
        if spec is not None and spec.loader is not None:
            spec.loader = _Loader(spec.loader, self.names.get(fullname, []), self.values.get(fullname, {}))
        return spec


def _install() -> None:
    names: dict[str, list[str]] = {}
    for target in json.loads(os.environ.get("CPU_E2E_WAIVE", "{}")):
        module, function = str(target).split(":")
        names.setdefault(module, []).append(function)
    values: dict[str, dict[str, str]] = {}
    for target, literal in json.loads(os.environ.get("CPU_E2E_PATCH", "{}")).items():
        module, name = str(target).split(":")
        values.setdefault(module, {})[name] = str(literal)
    if names or values:
        sys.meta_path.insert(0, _Finder(names, values))


_install()
