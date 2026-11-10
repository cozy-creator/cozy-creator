"""The public API through cozy: clients built here against the installed core, each run in a fresh
process. `pack.sh` makes this package and puts their sources in `tree`."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import msgspec
import tensorfs
from cozy_runtime.author import App, Context

app = App()
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


class Request(msgspec.Struct, forbid_unknown_fields=True):
    case: str = "all"
    seed: int = 138


class Report(msgspec.Struct):
    passed: bool
    detail: str


def cc(source, output, *flags):
    return ["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", *flags, f"-I{TREE}/crates/varena/include", f"-L{NATIVE}",
            f"-Wl,-rpath-link,{NATIVE}", str(source), "-lvarena", "-ldl", "-o", str(WORK / output)]


def cargo(*arguments):
    return [str(CARGO), "build", "--manifest-path", str(TREE / "Cargo.toml"), "--target-dir", str(TARGET), "-j", "2", *arguments]


def python(script, *arguments):
    return [sys.executable, "-B", str(script), *arguments]


def mixed(order):
    return python(SUITE / "mixed.py", "--c", str(WORK / "mixed-c.so"), "--rust", str(TARGET / "debug/examples/libmixed.so"),
                  "--order", order, "--gpu")


def cases(seed):
    cell = lambda *arguments: python(SUITE / "cells.py", "--seed", str(seed), *arguments)
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


def run(argv, ctx):
    process = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=ENV)
    assert process.stdout is not None
    lines = []
    for line in process.stdout:
        lines.append(line.rstrip())
        ctx.raise_if_cancelled()
    code = process.wait()
    rows = [line for line in lines if line.startswith("{")]
    return {"step": " ".join(Path(part).name[:40] for part in argv[:3]), "exit": code, "rows": [json.loads(row) for row in rows],
            "tail": lines[-30:] if code else []}


@app.entrypoint(accelerator=True)
def public(request: Request, ctx: Context) -> Report:
    WORK.mkdir(exist_ok=True)
    chosen = cases(request.seed)
    steps = []
    for name in chosen if request.case == "all" else [request.case]:
        for argv in chosen[name]:
            steps.append(run(argv, ctx))
            if steps[-1]["exit"]:
                return Report(False, json.dumps(steps))
    return Report(True, json.dumps(steps))
