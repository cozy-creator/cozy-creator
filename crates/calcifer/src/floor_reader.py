"""Each name a package wheel (argv[2]) imports from `cozy_runtime` that a Runtime wheel (argv[1]) lacks, read from both's
source with `ast` alone: nothing is imported or installed (cl-952). Prints {"missing": [[module, name or null, file, line]]}."""

import ast
import json
import sys
import zipfile

runtime = zipfile.ZipFile(sys.argv[1])
held = set(runtime.namelist())
bound: dict[str, set[str] | None] = {}
listed: dict[str, list[str] | None] = {}


def source(module: str) -> tuple[bytes | None, bool]:
    """A Runtime module's source, and whether it is a package."""
    base = module.replace(".", "/")
    for path, package in ((base + "/__init__.py", True), (base + ".py", False), (base + ".pyi", False)):
        if path in held:
            return runtime.read(path), package
    return None, False


def scope(module: str) -> set[str] | None:
    """The names a Runtime module binds at module scope; None when the Runtime has no such module."""
    if module in bound:
        return bound[module]
    bound[module] = set()
    text, package = source(module)
    if text is None:
        bound[module] = None
        return None
    names: set[str] = set()
    here = module if package else module.rpartition(".")[0]

    def bind(body: list[ast.stmt]) -> None:
        for node in body:
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                names.add(node.name)
            elif isinstance(node, ast.Assign):
                for target in node.targets:
                    names.update(leaf.id for leaf in ast.walk(target) if isinstance(leaf, ast.Name))
            elif isinstance(node, (ast.AnnAssign, ast.AugAssign)):
                names.update(leaf.id for leaf in ast.walk(node.target) if isinstance(leaf, ast.Name))
            elif isinstance(node, ast.Import):
                names.update(alias.asname or alias.name.split(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom):
                base = node.module or ""
                if node.level:
                    parent = here
                    for _ in range(node.level - 1):
                        parent = parent.rpartition(".")[0]
                    base = parent + ("." + base if base else "")
                for alias in node.names:
                    if alias.name == "*":
                        names.update(exported(base))
                    else:
                        names.add(alias.asname or alias.name)
            elif isinstance(node, ast.If):
                bind(node.body)
                bind(node.orelse)
            elif isinstance(node, ast.Try):
                bind(node.body)
                bind(node.orelse)
                bind(node.finalbody)
                for handler in node.handlers:
                    bind(handler.body)
            elif isinstance(node, ast.With):
                bind(node.body)

    tree = ast.parse(text)
    bind(tree.body)
    bound[module] = names
    listed[module] = None
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == "__all__" for target in node.targets):
            listed[module] = [str(name) for name in ast.literal_eval(node.value)]
    return names


def exported(module: str) -> set[str]:
    """What `from module import *` binds: its `__all__`, else its public names."""
    names = scope(module) or set()
    named = listed.get(module)
    return set(named) if named is not None else {name for name in names if not name.startswith("_")}


def has(module: str, name: str) -> bool:
    names = scope(module)
    return names is not None and (name in names or source(module + "." + name)[0] is not None)


def runtime_module(name: str) -> bool:
    return name == "cozy_runtime" or name.startswith("cozy_runtime.")


missing: dict[tuple[str, str | None], tuple[str, int]] = {}


def read(path: str, tree: ast.Module) -> None:
    """What one of the package's files imports from the Runtime, and what it reads of a Runtime module it imported."""
    modules: dict[str, str] = {}

    def lack(module: str, name: str | None, node: ast.stmt | ast.expr) -> None:
        missing.setdefault((module, name), (path, node.lineno))

    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom) and not node.level and node.module and runtime_module(node.module):
            if scope(node.module) is None:
                lack(node.module, None, node)
                continue
            for alias in node.names:
                if alias.name == "*":
                    continue
                if not has(node.module, alias.name):
                    lack(node.module, alias.name, node)
                elif source(node.module + "." + alias.name)[0] is not None:
                    modules[alias.asname or alias.name] = node.module + "." + alias.name
        elif isinstance(node, ast.Import):
            for alias in node.names:
                if not runtime_module(alias.name):
                    continue
                if scope(alias.name) is None:
                    lack(alias.name, None, node)
                elif alias.asname:
                    modules[alias.asname] = alias.name
                else:
                    modules["cozy_runtime"] = "cozy_runtime"

    def uses(node: ast.AST, shadowed: frozenset[str]) -> None:
        # A function's own name for something else (`fusion = transformer.text_fusion`) is no Runtime module.
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)):
            stored = {leaf.id for leaf in ast.walk(node) if isinstance(leaf, ast.Name) and isinstance(leaf.ctx, ast.Store)}
            stored |= {arg.arg for arg in ast.walk(node.args) if isinstance(arg, ast.arg)}
            shadowed = shadowed | (stored & modules.keys())
        if isinstance(node, ast.Attribute):
            chain: list[str] = []
            root: ast.expr = node
            while isinstance(root, ast.Attribute):
                chain.insert(0, root.attr)
                root = root.value
            if isinstance(root, ast.Name) and root.id in modules and root.id not in shadowed:
                module = modules[root.id]
                for attr in chain:
                    if source(module + "." + attr)[0] is not None:
                        module = module + "." + attr
                        continue
                    if not has(module, attr):
                        lack(module, attr, node)
                    break
                return
        for child in ast.iter_child_nodes(node):
            uses(child, shadowed)

    uses(tree, frozenset())


package = zipfile.ZipFile(sys.argv[2])
for name in package.namelist():
    if name.endswith(".py"):
        try:
            parsed = ast.parse(package.read(name))
        except SyntaxError:
            continue
        read(name, parsed)
rows = sorted(missing.items(), key=lambda row: row[1])
print(json.dumps({"missing": [[module, name, path, line] for (module, name), (path, line) in rows]}))
