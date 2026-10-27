"""Build the gate package around the exact candidate H3 modules, without copying their logic."""

import argparse
import ast
import hashlib
import json
import shutil
import subprocess
from pathlib import Path

MODULES = ("adaln_pruned.py", "turbo.py", "glue.py", "producer.py", "_prepared.py")


def literal(tree, name):
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(
            isinstance(t, ast.Name) and t.id == name for t in node.targets
        ):
            return list(ast.literal_eval(node.value))
    raise SystemExit(f"official.py has no literal {name}")


def dense_steps(tree):
    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef) and node.name == "denoise":
            for arg, default in zip(node.args.kwonlyargs, node.args.kw_defaults, strict=True):
                if arg.arg == "sol_dense_steps":
                    return ast.literal_eval(default)
    raise SystemExit("official.py denoise has no sol_dense_steps default")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("h3", type=Path, help="candidate packages worktree's minimax-h3 directory")
    parser.add_argument("output", type=Path, help="new build directory; must not exist")
    args = parser.parse_args()
    root = args.h3.resolve() / "minimax_h3"
    target = args.output.resolve()
    if target.exists():
        raise SystemExit("output already exists; choose a new build directory")
    commit = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    dirty = subprocess.check_output(
        ["git", "-C", str(root), "status", "--porcelain", "--", "."], text=True
    ).strip()
    if dirty:
        raise SystemExit(f"{root} has uncommitted changes:\n{dirty}")
    official = ast.parse((root / "official.py").read_text())
    shutil.copytree(Path(__file__).parent, target, ignore=shutil.ignore_patterns("prepare.py"))
    candidate = target / "ci_h3_kitchen_carry" / "candidate"
    candidate.mkdir()
    (candidate / "__init__.py").write_text("")
    files = {}
    for name in MODULES:
        content = (root / name).read_bytes()
        (candidate / name).write_bytes(content)
        files[name] = hashlib.sha256(content).hexdigest()
    source = {
        "package_commit": commit,
        "files": files,
        # The production attention policy, read from official.py at this commit.
        "dense_paths": literal(official, "_DENSE_ATTENTION_PATHS"),
        "exact_paths": literal(official, "_EXACT_ATTENTION_PATHS"),
        "sol_dense_steps": dense_steps(official),
    }
    (target / "ci_h3_kitchen_carry" / "source.json").write_text(json.dumps(source, indent=1) + "\n")
    print(target)


if __name__ == "__main__":
    main()
