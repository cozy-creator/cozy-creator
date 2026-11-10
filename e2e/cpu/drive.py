"""e2e/cpu: one package's cells through the Runtime's real executor on the CPU, with tiny random weights.

Runs in the package's own environment (run.sh). It stands where TensorD stands: the executor process is the
Runtime's (`python -m cozy_runtime.internal.executor`), and this sends it TensorD's frames over the seam (hello,
start, load, activate, prepare_request, invoke, shutdown), the bindings built from the package interface as
`tensord/src/gpu_service.rs` builds them. The weights are the fixture's (`tiny/<package>.py`): the model's real
classes at a tiny config, ingested by TensorFS's own import into a throwaway store.

    drive.py <package dir> <interface.json> <fixture.py> <out dir> [--only cell,...] [--record]

Prints one line per cell: PASS|FAIL <package> <entry> <cell> <seconds>s[: first error line]. Exit 1 on any FAIL.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import time
import traceback
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
THREADS = os.environ.get("CPU_E2E_THREADS", "2")


class Failed(Exception):
    pass


def first_line(text: object) -> str:
    return next((line.strip() for line in str(text).splitlines() if line.strip()), "")[:300]


class Executor:
    """One executor process and its seam."""

    def __init__(self, work: Path, store: Path, ranks: int = 1) -> None:
        from cozy_runtime.internal import seam

        work.mkdir(parents=True, exist_ok=True)
        self.work, self.ranks = work, ranks
        (work / "seam").unlink(missing_ok=True)
        # A socket path is at most 107 bytes: bound and dialed through this directory's descriptor.
        self.fd = os.open(work, os.O_RDONLY)
        listening = seam.listener(f"/proc/self/fd/{self.fd}/seam")
        env = {
            **{k: v for k, v in os.environ.items() if k in ("PATH", "HOME", "LANG", "TMPDIR", "VIRTUAL_ENV")},
            "CUDA_VISIBLE_DEVICES": "",
            "TENSORFS_HOME": str(store),
            "OMP_NUM_THREADS": THREADS,
            "MKL_NUM_THREADS": THREADS,
            "COZY_CPU_RANKS": str(ranks),
        }
        self.stderr = work / "executor.log"
        self.child = subprocess.Popen(
            [sys.executable, "-m", "cozy_runtime.internal.executor", "--socket", f"/proc/{os.getpid()}/fd/{self.fd}/seam",
             "--root", str(work)],
            env=env, stderr=open(self.stderr, "w"), stdout=subprocess.DEVNULL,
        )
        listening.settimeout(180)
        sock, _ = listening.accept()
        sock.settimeout(None)
        listening.close()
        self.channel = seam.Channel(sock)
        self.events: list[dict[str, Any]] = []

    def ask(self, frame: dict[str, Any], timeout: float = 900) -> dict[str, Any]:
        self.channel.send(frame)
        deadline = time.monotonic() + timeout
        while True:
            got = self.channel.recv(timeout=max(1.0, deadline - time.monotonic()))
            if got is None:
                raise Failed(f"the executor left during {frame['cmd']}: {self.last_error()}")
            if got.get("reply") == frame["cmd"]:
                return got
            self.events.append(got)
            if time.monotonic() > deadline:
                raise Failed(f"{frame['cmd']} did not answer in {timeout:.0f} s")

    def must(self, frame: dict[str, Any], timeout: float = 900) -> dict[str, Any]:
        reply = self.ask(frame, timeout)
        if not reply.get("ok"):
            raise Failed(f"{frame['cmd']}: {reply.get('code')}: {first_line(reply.get('detail', ''))}")
        return reply

    def last_error(self) -> str:
        lines = [line for line in self.stderr.read_text(errors="replace").splitlines() if line.strip()]
        errors = [line for line in lines if "Error" in line or "error" in line]
        return first_line((errors or lines or ["no stderr"])[-1])

    def close(self) -> None:
        try:
            if self.child.poll() is None:
                self.channel.send({"cmd": "shutdown"})
                self.child.wait(20)
        except Exception:
            pass
        if self.child.poll() is None:
            self.child.kill()
            self.child.wait()
        os.close(self.fd)


def bindings(interface: dict[str, Any], interface_path: str, entry: dict[str, Any], manifests: dict[str, str],
             store: Path, release: str, package: str) -> list[dict[str, Any]]:
    """The entry point's constructed slots as TensorD binds them (`gpu_service.rs`, `Binding`)."""
    import tensorfs

    rows = []
    for slot in entry.get("models", []):
        if slot.get("weights"):
            continue
        ref = slot.get("default") or slot["path"]
        manifest = manifests.get(slot["path"]) or manifests.get(ref)
        if manifest is None:
            raise Failed(f"the fixture builds no model for {slot['path']} ({ref})")
        held = list(tensorfs.open(manifest, cache_dir=store, local_files_only=True).components)
        wanted = slot.get("components") or sorted({c for use in slot.get("component_use", {}).values() for c in use})
        components = [c for c in held if not wanted or c in wanted]
        missing = [c for c in wanted if c not in held]
        if missing or not components:
            raise Failed(f"{slot['path']} uses {', '.join(missing) or 'components'} that the tiny {ref} does not hold ({', '.join(held)})")
        parameter = slot["path"].rsplit(".", 1)[-1]
        rows.append({"binding": {
            "application": interface["application"], "package_interface": interface_path, "model_class": slot["class"],
            "model_binding_path": slot["path"], "model_parameter_name": parameter, "model_parameter_names": [parameter],
            "component": components[0], "components": components, "snapshots": {c: manifest for c in components},
            "snapshot": manifest, "store": str(store), "release": release, "package": package, "model": f"tiny/{ref}",
        }})
    return rows


def signature(data: bytes, parts: int = 64) -> list[float]:
    """A coarse picture of an output's bytes: the mean of each of `parts` equal runs."""
    import numpy as np

    values = np.frombuffer(data, dtype=np.uint8).astype(np.float64)
    return [round(float(chunk.mean()), 2) if chunk.size else 0.0 for chunk in np.array_split(values, parts)]


def judge(cell: dict[str, Any], reply: dict[str, Any], spool: Path, reference: dict[str, Any] | None) -> dict[str, Any]:
    """The cell's output judged, never only its completion: present, not flat, and the stored reference's."""
    import numpy as np

    outcome = reply.get("outcome") or {}
    if not reply.get("ok") or outcome.get("terminal") != "succeeded":
        raise Failed(f"{outcome.get('code') or reply.get('code')}: {first_line(outcome.get('message') or reply.get('detail', ''))}")
    result_file = spool / "result.canonical"
    result = json.loads(result_file.read_bytes()) if result_file.exists() else reply.get("result")
    seen: dict[str, Any] = {"result": result, "outputs": []}
    files = sorted(p for p in spool.iterdir() if p.name != "result.canonical" and p.is_file())
    if len(files) < len(reply.get("outputs") or []):
        raise Failed(f"{len(reply['outputs'])} outputs declared, {len(files)} files in the spool")
    for path in files:
        data = path.read_bytes()
        if not data:
            raise Failed(f"output {path.name} is empty")
        values = np.frombuffer(data, dtype=np.uint8)
        if path.suffix == ".raw" and float(values.std()) < 1.0:
            raise Failed(f"output {path.name} is flat (spread {float(values.std()):.3f})")
        seen["outputs"].append({"name": path.name, "bytes": len(data), "signature": signature(data)})
    check = cell.get("check")
    if check is not None:
        problem = check(result, files)
        if problem:
            raise Failed(str(problem))
    if reference is not None:
        compare(seen, reference, float(cell.get("tolerance", 3.0)))
    return seen


def compare(seen: dict[str, Any], reference: dict[str, Any], tolerance: float) -> None:
    want, got = reference.get("outputs", []), seen["outputs"]
    if [(o["name"], o["bytes"]) for o in want] != [(o["name"], o["bytes"]) for o in got]:
        raise Failed(f"outputs {[(o['name'], o['bytes']) for o in got]} differ from the reference's {[(o['name'], o['bytes']) for o in want]}")
    for a, b in zip(want, got, strict=True):
        worst = max(abs(x - y) for x, y in zip(a["signature"], b["signature"], strict=True))
        if worst > tolerance:
            raise Failed(f"output {b['name']} differs from its reference by {worst:.1f}/255 (tolerance {tolerance})")
    if "result" in reference and reference["result"] != seen["result"]:
        raise Failed(f"result {json.dumps(seen['result'])[:160]} differs from the reference's {json.dumps(reference['result'])[:160]}")


def inputs(cell: dict[str, Any], work: Path) -> tuple[dict[str, Any], dict[str, Any]]:
    """The cell's input files as the machine hands them: verified, in the attempt's spool."""
    granted: dict[str, Any] = {}
    for order, (name, source) in enumerate((cell.get("files") or {}).items()):
        path, media_type = source
        data = Path(path).read_bytes()
        granted[name] = {"local": str(path), "media_type": media_type, "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
                         "length": len(data), "order": order}
    return granted, {}


def main() -> int:
    package_dir, interface_path, fixture_path, out = (Path(a).resolve() for a in sys.argv[1:5])
    flags = sys.argv[5:]
    only = set(flags[flags.index("--only") + 1].split(",")) if "--only" in flags else None
    record = "--record" in flags
    package = package_dir.name
    out.mkdir(parents=True, exist_ok=True)
    store = out / "store"
    interface = json.loads(interface_path.read_text())
    spec = importlib.util.spec_from_file_location(f"tiny_{package.replace('-', '_').replace('.', '_')}", fixture_path)
    assert spec and spec.loader
    fixture = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fixture)
    references_path = HERE / "refs" / f"{package}.json"
    references = json.loads(references_path.read_text()) if references_path.exists() else {}
    failed = 0

    def line(ok: bool, entry: str, name: str, started: float, why: str = "") -> None:
        nonlocal failed
        failed += not ok
        print(f"{'PASS' if ok else 'FAIL'} {package} {entry} {name} {time.monotonic() - started:.1f}s{': ' + why if why else ''}", flush=True)

    started = time.monotonic()
    try:
        import tensorfs

        manifests = {}
        for ref, source in fixture.models(out / "source").items():
            manifests[ref] = source if str(source).startswith("sha256:") else tensorfs.convert_to_cozytensors(str(source), cache_dir=store).id
        line(True, "-", "tiny-weights", started)
    except Exception as exc:
        traceback.print_exc(file=open(out / "tiny-weights.err", "w"))
        line(False, "-", "tiny-weights", started, f"{type(exc).__name__}: {first_line(exc)}")
        return 1
    version = next((l.split('"')[1] for l in (package_dir / "pyproject.toml").read_text().splitlines() if l.startswith("version")), "0")
    entries = {row["name"]: row for row in interface["entrypoints"]}
    cells = [c for c in fixture.CELLS if only is None or c["name"] in only or f"{c['entry']}/{c['name']}" in only]
    executors: dict[int, Executor] = {}
    loaded: set[tuple[int, str]] = set()
    try:
        for index, cell in enumerate(cells):
            entry, name, ranks = cell["entry"], cell["name"], int(cell.get("ranks", 1))
            started = time.monotonic()
            request = f"{entry}-{name}-{index}"
            try:
                row = entries.get(entry)
                if row is None:
                    raise Failed(f"the package interface has no entry point {entry}")
                executor = executors.get(ranks)
                if executor is None:
                    executor = executors[ranks] = Executor(out / f"executor-{ranks}", store, ranks)
                    executor.must({"cmd": "hello"})
                    executor.must({"cmd": "start", "devices": "", "application": interface["application"],
                                   "package_interface": str(interface_path), "sequence_parallel_degree": ranks, "model_choices": True})
                construction = f"{entry}@{ranks}"
                if (ranks, construction) not in loaded:
                    rows = bindings(interface, str(interface_path), row, manifests, store, f"{package}@{version}", package)
                    load: dict[str, Any] = {"cmd": "load", "construction": construction, "devices": "", "sequence_parallel_degree": ranks}
                    load["binding"] = rows[0]["binding"] if rows else {"application": interface["application"], "package_interface": str(interface_path)}
                    if len(rows) > 1:
                        load["models"] = rows
                    executor.must(load)
                    loaded.add((ranks, construction))
                executor.must({"cmd": "activate", "construction": construction})
                executor.must({"cmd": "prepare_request", "request_id": request, "entrypoint": entry, "payload": cell["payload"],
                               "construction": construction})
                spool = out / "spool" / request
                spool.mkdir(parents=True)
                granted, _ = inputs(cell, spool)
                reply = executor.ask({"cmd": "invoke", "request_id": request, "entrypoint": entry, "spool": str(spool),
                                      "construction": construction, "deadline_s": 900, "inputs": granted})
                (out / f"{request}.reply.json").write_text(json.dumps(reply, indent=1))
                key = f"{entry}/{name}"
                seen = judge(cell, reply, spool, None if record else references.get(cell.get("same_as", key)))
                if record and "same_as" not in cell:
                    references[key] = seen
                elif key not in references and "same_as" not in cell:
                    raise Failed("no stored reference for this cell: record one (run.sh --record)")
                line(True, entry, name, started)
            except Exception as exc:
                why = first_line(exc) if isinstance(exc, Failed) else f"{type(exc).__name__}: {first_line(exc)}"
                line(False, entry, name, started, why)
                dead = executors.pop(ranks, None)
                if dead is not None:  # the next cell starts a fresh executor
                    dead.close()
                    loaded = {key for key in loaded if key[0] != ranks}
    finally:
        for executor in executors.values():
            executor.close()
    if record:
        references_path.write_text(json.dumps(references, indent=1, sort_keys=True) + "\n")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
