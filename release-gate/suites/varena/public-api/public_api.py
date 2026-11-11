"""varena's public API across bindings (README.md): C, Rust and Python clients built on the pod against the installed
core, each in a fresh process, on one core. A plain program on the rental (`release-gate/programs.sh`, xs-174), not a
package: the package contract keeps packages off varena. `pack.sh` lays this directory out with the clients' sources in
`tree`. One ROW per case: `public-mixed`, `public-engines`, `public-sizing`, `public-priority`."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Protocol, TypedDict

import tensorfs

TREE = Path(__file__).with_name("tree")
SUITE = TREE / "release-gate/suites/varena/public-api"
EXAMPLES = TREE / "crates/varena/examples"
NATIVE = Path(tensorfs.__file__).parent / ".native"
WORK = Path(tempfile.gettempdir()) / "ci-varena-public"
CARGO = WORK / "cargo/bin/cargo"
TARGET = WORK / "target"
ENV = {**os.environ, "PYTHONPATH": str(EXAMPLES), "LD_LIBRARY_PATH": str(NATIVE), "VARENA_LIB_DIR": str(NATIVE),
       "RUSTUP_HOME": str(WORK / "rustup"), "CARGO_HOME": str(WORK / "cargo"), "RUSTUP_TOOLCHAIN": "1.99.1"}
# Pod images have neither curl nor wget.
RUSTUP = [sys.executable, "-c", f"""
import os, pathlib, urllib.request
if not pathlib.Path({str(CARGO)!r}).exists():
    init = {str(WORK / "rustup-init")!r}
    urllib.request.urlretrieve("https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init", init)
    os.chmod(init, 0o755)
    os.execv(init, [init, "-y", "-q", "--profile", "minimal", "--no-modify-path", "--default-toolchain", "1.99.1"])
"""]
type JSON = None | bool | int | float | str | list[JSON] | dict[str, JSON]
SEED = 138


class Cell(Protocol):
    def __call__(self, *arguments: str) -> list[str]: ...


class Step(TypedDict):  # one command's outcome
    step: str
    exit: int
    rows: list[JSON]  # its JSON lines
    tail: list[str]


def cc(source: Path, output: str, *flags: str) -> list[str]:
    return ["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", *flags, f"-I{TREE}/crates/varena/include", f"-L{NATIVE}",
            f"-Wl,-rpath-link,{NATIVE}", str(source), "-lvarena", "-ldl", "-o", str(WORK / output)]


def cargo(*arguments: str) -> list[str]:
    return [str(CARGO), "build", "--manifest-path", str(TREE / "Cargo.toml"), "--target-dir", str(TARGET), "-j", "2", *arguments]


def python(script: Path, *arguments: str) -> list[str]:
    return [sys.executable, "-B", str(script), *arguments]


def mixed(order: str) -> list[str]:
    return python(SUITE / "mixed.py", "--c", str(WORK / "mixed-c.so"), "--rust", str(TARGET / "debug/examples/libmixed.so"),
                  "--order", order, "--gpu")


def cases(seed: int) -> dict[str, list[list[str]]]:
    cell: Cell = lambda *arguments: python(SUITE / "cells.py", "--seed", str(seed), *arguments)
    return {
        "mixed": [cc(SUITE / "native.c", "mixed-c.so", "-fPIC", "-shared"), RUSTUP, cargo("-p", "varena-client", "--example", "mixed"),
                  mixed("native-first"), mixed("python-first")],
        "engines": [cc(EXAMPLES / "native_engine.c", "native-engine"), [str(WORK / "native-engine")],
                    RUSTUP, cargo("-p", "varena-client", "--example", "engine"), [str(TARGET / "debug/examples/engine")],
                    python(EXAMPLES / "cache_application.py"), python(EXAMPLES / "python_client.py")],
        "sizing": [cc(SUITE / "sized_engine.c", "sized-engine.so", "-fPIC", "-shared"),
                   cell("sizing", "--engine", str(WORK / "sized-engine.so"))],
        "priority": [cell("priority", "high"), cell("priority", "normal"), cell("priority", "peer")],
    }


def run(argv: list[str]) -> Step:
    process = subprocess.run(argv, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=ENV)
    lines = process.stdout.splitlines()
    rows = [line for line in lines if line.startswith("{")]
    return {"step": " ".join(Path(part).name[:40] for part in argv[:3]), "exit": process.returncode,
            "rows": [json.loads(row) for row in rows], "tail": lines[-30:] if process.returncode else []}


if __name__ == "__main__":
    WORK.mkdir(exist_ok=True)
    for name, commands in cases(SEED).items():
        steps: list[Step] = []
        for argv in commands:
            steps.append(run(argv))
            if steps[-1]["exit"]:
                break
        passed = all(step["exit"] == 0 for step in steps) and len(steps) == len(commands)
        print("ROW " + json.dumps({"cell": f"public-{name}", "result": "PASS" if passed else "FAIL",
                                   "detail": steps if not passed else [step["rows"] for step in steps]}), flush=True)
