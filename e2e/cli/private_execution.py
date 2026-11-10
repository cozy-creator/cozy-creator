#!/usr/bin/env python3
"""A real installed-CLI check inside an already-unlocked synthetic vault.

Called by the release owner after installing the candidate normally. No second home,
mock service, rented machine, or account is created. The caller owns lock/unlock.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid


def command(cli: Path, output: Path, name: str, *args: str, fails: bool = False) -> bytes:
    stdout = output / f"{name}.ndjson"
    stderr = output / f"{name}.stderr"
    # Preserve real partial output even if the runner is interrupted. A long-running
    # call reports progress without imposing a new execution timeout or retry.
    with stdout.open("wb") as out, stderr.open("wb") as err:
        process = subprocess.Popen([str(cli), "--json", *args], stdout=out, stderr=err)
        print(f"vault proof: {name} started", flush=True)
        report = time.monotonic() + 30
        while process.poll() is None:
            time.sleep(1)
            if time.monotonic() >= report:
                state = ""
                for line in reversed(stdout.read_bytes()[-16384:].splitlines()):
                    try:
                        event = json.loads(line)
                    except (ValueError, UnicodeDecodeError):
                        continue
                    run = event.get("run", {})
                    if run.get("status") or event.get("type"):
                        state = f" event={event.get('type', '')} status={run.get('status', '')} machine_state={run.get('machine_state', '')}"
                        break
                print(f"vault proof: {name} still active stdout={stdout.stat().st_size} stderr={stderr.stat().st_size}{state}", flush=True)
                report = time.monotonic() + 30
    print(f"vault proof: {name} exited {process.returncode}", flush=True)
    if bool(process.returncode) != fails:
        raise AssertionError(f"{name} returned unexpected exit {process.returncode}; see its captured output")
    return stdout.read_bytes()


def contains(path: Path, markers: list[bytes]) -> bool:
    overlap = max(map(len, markers)) - 1
    tail = b""
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            text = tail + chunk
            if any(marker in text for marker in markers):
                return True
            tail = text[-overlap:]
    return False


def scan(roots: list[Path], exclusions: list[Path], since_ns: int, markers: list[bytes]) -> tuple[list[str], int]:
    leaked: list[str] = []
    checked = 0
    seen: set[Path] = set()
    for root in roots:
        if not root.is_dir():
            continue
        for directory, dirs, files in os.walk(root, followlinks=False):
            parent = Path(directory)
            dirs[:] = [name for name in dirs if not any((parent / name).is_relative_to(excluded) for excluded in exclusions)]
            for name in files:
                path = parent / name
                if path in seen or path.is_symlink() or any(path.is_relative_to(excluded) for excluded in exclusions):
                    continue
                seen.add(path)
                try:
                    stat = path.stat()
                    if stat.st_mtime_ns < since_ns or not path.is_file():
                        continue
                    checked += 1
                    if any(marker.decode() in name for marker in markers) or contains(path, markers):
                        leaked.append(str(path))
                except FileNotFoundError:
                    # An unrelated cache file was collected while this read-only audit ran.
                    continue
    return leaked, checked


def observed_run(body: bytes) -> dict:
    for line in reversed(body.splitlines()):
        try:
            row = json.loads(line).get("run", {})
        except (ValueError, AttributeError):
            continue
        if isinstance(row, dict) and row.get("id") and row.get("number") and row.get("target"):
            return {key: row[key] for key in ("id", "number", "target")}
    raise AssertionError("the real CLI returned no identifiable private run")


def runs_page(prefix: str, before: int | None = None) -> dict:
    # Use the installed Calcifer's ordinary authenticated API. Never save its bearer
    # or a complete private response in the proof's artifacts.
    holder = json.loads((Path.home() / ".cozy/calcifer/calcifer.lock.holder").read_text())
    query = {"package": prefix, "limit": 1}
    if before is not None:
        query["before"] = before
    request = urllib.request.Request(
        f"http://{holder['address']}/v1/runs?{urllib.parse.urlencode(query)}",
        headers={"Authorization": f"Bearer {holder['token']}"},
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        raise AssertionError(f"the authenticated private history query returned HTTP {error.code}") from None
    except urllib.error.URLError:
        raise AssertionError("the authenticated private history query could not reach Calcifer") from None


def target_history(cli: Path, output: Path, label: str, proof: dict, *, locked: bool = False) -> list[str]:
    target = proof["target"]
    # An actual partial function prefix, rather than an exact-target shortcut.
    prefix = target[:-8]
    listed = json.loads(command(cli, output, label, "run", "ls", "--all", "--package", prefix, "--limit", "0"))
    rows = listed["runs"]
    if locked:
        assert not rows and listed["older"] == 0, "locked private target history remained visible through the CLI"
        page = runs_page(prefix)
        assert not page["runs"] and page["older"] == 0, "locked private target history remained visible through the API"
        return []
    ids = [row["id"] for row in rows]
    assert set(proof["ids"]).issubset(ids), "authorized private target history lost an original run"
    assert all(row["target"] == target for row in rows), "private prefix query returned an unrelated target"
    assert len(ids) == len(set(ids)), "private target history duplicated a run"
    assert [row["number"] for row in rows] == sorted((row["number"] for row in rows), reverse=True), "private target history is not newest first"
    assert listed["older"] == 0, "the CLI did not finish its requested complete private history"
    before = None
    for index, expected in enumerate(rows):
        page = runs_page(prefix, before)
        assert len(page["runs"]) == 1 and page["runs"][0]["id"] == expected["id"], "private target cursor skipped or repeated a run"
        assert page["runs"][0]["target"] == target, "private target cursor returned an unrelated run"
        assert page["older"] == len(rows) - index - 1, "private target pagination counted unrelated or unreadable rows"
        before = expected["number"]
    page = runs_page(prefix, before)
    assert not page["runs"] and page["older"] == 0, "private target pagination did not end"
    (output / f"{label}-receipt.json").write_text(json.dumps({"prefix_filter": True, "cursor_pages": len(rows), "exact_older": True, "retained_run_ids": ids}, indent=2))
    return ids


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--view", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cli", type=Path, default=Path.home() / ".cozy/bin/cozy")
    args = parser.parse_args()
    if os.name != "posix" or not Path("/proc").is_dir():
        raise SystemExit("private local execution is qualified on Linux; other OSes use trusted remote execution")
    args.out.mkdir(parents=True, exist_ok=True)
    view = args.view.resolve(strict=True)
    project = args.project.resolve(strict=True)
    assert (view / ".vault-local-execution.json").is_file(), "the live vault needs its normal execution marker"
    home = Path.home() / ".cozy"
    assert not args.out.resolve().is_relative_to(home), "proof artifacts belong outside the user's managed cache"
    stamp = time.time_ns() - 2_000_000_000
    unique = uuid.uuid4().hex
    source_canary = f"XS160_SOURCE_{unique}"
    input_canary = f"XS160_INPUT_{unique}"
    payload_canary = f"XS160_PAYLOAD_{unique}"
    target_canary = f"xs160_target_{unique}"
    folder = view / "private-execution-proof"
    folder.mkdir(exist_ok=True)
    script = folder / "probe.py"
    script.write_text('''# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
import os
from pathlib import Path
import tempfile
import msgspec
from cozy_runtime.author import App, Outputs
from cozy_runtime.media import File
app = App()
SOURCE = "''' + source_canary + '''"
class Request(msgspec.Struct, forbid_unknown_fields=True):
    source: File
    text: str = "''' + source_canary + '''"
    fail: bool = False
class Answer(msgspec.Struct):
    file: File
    text: str
    scratch_private: bool
@app.entrypoint(accelerator=False)
def ''' + target_canary + '''(payload: Request, out: Outputs) -> Answer:
    content = payload.source.read_bytes() + b"\\n" + payload.text.encode() + b"\\n" + SOURCE.encode()
    with tempfile.NamedTemporaryFile(delete=False) as scratch:
        scratch.write(content)
        scratch_path = Path(scratch.name)
    private = ".vault-machine" in scratch_path.parts and ".vault-machine" in Path.cwd().parts
    print(SOURCE + ":" + payload.text, flush=True)
    if payload.fail:
        raise RuntimeError(SOURCE + ":" + payload.text)
    target = out.temporary_file(".txt")
    target.write_bytes(content)
    return Answer(out.save_file(target, media_type="text/plain"), payload.text, private)
''')
    compile(script.read_text(), str(script), "exec")
    source = folder / "input.txt"
    source.write_text(input_canary)
    request = folder / "input.json"
    request.write_text(json.dumps({"text": payload_canary, "source": str(source)}))
    # --describe and its source staging must use the same project scope as execution.
    described = command(args.cli, args.out, "describe", "run", "-p", str(project), str(script), "--describe", "--on", "local")
    schema = json.loads(described)
    assert schema.get("accelerator") is False and {field["name"] for field in schema.get("fields", [])} == {"text", "source", "fail"}, "private source did not describe its input schema"
    run = command(args.cli, args.out, "run", "run", "-p", str(project), str(script), "--input", str(request), "--on", "local")
    assert payload_canary.encode() in run, "private run did not return its payload"
    assert b'"scratch_private":true' in run.replace(b" ", b""), "runtime scratch or working directory escaped the private namespace"
    request.write_text(json.dumps({"text": payload_canary, "source": str(source), "fail": True}))
    failed = command(args.cli, args.out, "failure", "run", "-p", str(project), str(script), "--input", str(request), "--on", "local", fails=True)
    assert payload_canary.encode() in failed + (args.out / "failure.stderr").read_bytes(), "private failure did not expose its authorized detail to the caller"
    completed, failed_run = observed_run(run), observed_run(failed)
    assert completed["target"].endswith(f"/{target_canary}") and failed_run["target"] == completed["target"], "authorized private target was not preserved"
    marker = json.loads((view / ".vault-local-execution.json").read_text())["storage"]
    proof = {"target": completed["target"], "function": target_canary, "ids": [completed["id"], failed_run["id"]], "project_id": marker["project_id"], "generation": marker["generation"]}
    proof["ids"] = target_history(args.cli, args.out, "target-history", proof)
    (folder / "target-proof.json").write_text(json.dumps(proof))
    markers = [value.encode() for value in (source_canary, input_canary, payload_canary, target_canary)]
    files = [path for path in view.rglob("*") if path.is_file() and not path.is_symlink()]
    assert any(contains(path, markers) for path in files if ".vault-machine" in path.parts), "no protected machine journal/source/result was retained"
    store = Path(os.environ.get("TENSORFS_HOME", str(Path.home() / ".cache/tensorfs")))
    roots = [home, store, project]
    exclusions = [home / "calcifer/vaults", args.out.resolve()]
    leaked, checked = scan(roots, exclusions, stamp, markers)
    (args.out / "scan.json").write_text(json.dumps({"checked_new_or_modified_files": checked, "leaked_paths": leaked}, indent=2))
    assert not leaked, "private test markers reached global plaintext storage; see scan.json"
    (args.out / "receipt.json").write_text(json.dumps({"describe": True, "local_file_execution": True, "protected_scratch": True, "private_failure": True, "private_target_metadata": True, "target_filter_pagination": True, "global_plaintext_scan": True, "synthetic_only": True}, indent=2))


if __name__ == "__main__":
    main()
