#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["diffusers==0.43.0", "transformers==5.23.0"]
# ///
"""Generate the keep-in-fp32 table (`crates/tensorfs/keeps.json`) from Diffusers' and Transformers' model classes.

    uv run scripts/keeps.py           # regenerate and compare byte for byte
    uv run scripts/keeps.py --write   # rewrite it

An import stores each F32 tensor at its model's compute width unless the class its component loads into keeps it
wider (`src/import/keep.rs`). The rule is each class's own: Diffusers' `_keep_in_fp32_modules`, Transformers'
`_keep_in_fp32_modules_strict` (fp16 and bf16) and `_keep_in_fp32_modules` (fp16 only). Read from the installed
sources by `ast`, nothing imported: a class without its own list takes its nearest base's, and a Transformers class
also takes every list its modeling file declares (Transformers unites a model's submodels' lists when it builds one,
so this holds at least what it does). Every model class is listed, with or without a rule: a class the table lacks
keeps its F32 as stored.
"""

from __future__ import annotations

import argparse
import ast
import importlib.metadata
import importlib.util
import json
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parents[2] / "tensorfs" / "keeps.json"
ROOTS = {"diffusers": "ModelMixin", "transformers": "PreTrainedModel"}
ATTRS = {"_keep_in_fp32_modules": "fp32", "_keep_in_fp32_modules_strict": "strict"}


class Defined:
    def __init__(self, file: Path, bases: list[str], own: dict[str, list[str]]) -> None:
        self.file, self.bases, self.own = file, bases, own


def literal(node: ast.expr, where: str) -> list[str]:
    try:
        value = ast.literal_eval(node)
    except ValueError:
        raise SystemExit(f"{where}: not a literal list of names") from None
    if value is None:
        return []
    if isinstance(value, str):
        return [value]
    if not isinstance(value, (list, tuple, set)) or not all(isinstance(v, str) for v in value):
        raise SystemExit(f"{where}: not a literal list of names")
    return sorted(set(value))


def classes(library: str) -> dict[str, list[Defined]]:
    spec = importlib.util.find_spec(library)
    assert spec is not None and spec.submodule_search_locations, library
    root = Path(next(iter(spec.submodule_search_locations)))
    found: dict[str, list[Defined]] = {}
    for file in sorted(root.rglob("*.py")):
        # Transformers' modular_*.py are templates its modeling_*.py are generated from.
        if library == "transformers" and file.name.startswith("modular_"):
            continue
        for node in ast.walk(ast.parse(file.read_text(encoding="utf-8"))):
            if not isinstance(node, ast.ClassDef):
                continue
            bases = [b.id if isinstance(b, ast.Name) else b.attr for b in node.bases if isinstance(b, (ast.Name, ast.Attribute))]
            own: dict[str, list[str]] = {}
            for st in node.body:
                target = st.targets[0] if isinstance(st, ast.Assign) and len(st.targets) == 1 else getattr(st, "target", None)
                value = getattr(st, "value", None)
                if isinstance(target, ast.Name) and target.id in ATTRS and value is not None:
                    own[ATTRS[target.id]] = literal(value, f"{file}:{st.lineno}")
            found.setdefault(node.name, []).append(Defined(file, bases, own))
    return found


def table(library: str) -> dict[str, dict[str, list[str]]]:
    found = classes(library)

    known: dict[str, bool] = {}

    def model(name: str, seen: frozenset[str] = frozenset()) -> bool:
        if name not in known:
            known[name] = name == ROOTS[library] or any(
                b not in seen and model(b, seen | {name}) for d in found.get(name, []) for b in d.bases
            )
        return known[name]

    def inherited(name: str, attr: str, seen: frozenset[str] = frozenset()) -> list[str] | None:
        # A name defined twice keeps both definitions' lists.
        lists = [d.own[attr] for d in found.get(name, []) if attr in d.own]
        if lists:
            return sorted({m for lst in lists for m in lst})
        for d in found.get(name, []):
            for b in d.bases:
                if b not in seen and (got := inherited(b, attr, seen | {name})) is not None:
                    return got
        return None

    by_file: dict[Path, dict[str, set[str]]] = {}
    for defs in found.values():
        for d in defs:
            for attr, lst in d.own.items():
                by_file.setdefault(d.file, {}).setdefault(attr, set()).update(lst)
    out: dict[str, dict[str, list[str]]] = {}
    for name in sorted(found):
        if name == ROOTS[library] or not model(name):
            continue
        rule: dict[str, list[str]] = {}
        for attr in ATTRS.values():
            held = set(inherited(name, attr) or [])
            if library == "transformers":
                for d in found[name]:
                    held |= by_file.get(d.file, {}).get(attr, set())
            if held:
                rule[attr] = sorted(held)
        out[name] = rule
    return out


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    doc = {
        "generator": "scripts/keeps.py",
        "versions": {lib: importlib.metadata.version(lib) for lib in ROOTS},
        "classes": {lib: table(lib) for lib in ROOTS},
    }
    text = json.dumps(doc, indent=0, sort_keys=True, separators=(",", ":")) + "\n"
    if args.write:
        OUT.write_text(text, encoding="utf-8")
        return
    if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
        sys.exit(f"{OUT} differs from what {doc['generator']} writes: run it with --write")


if __name__ == "__main__":
    main()
