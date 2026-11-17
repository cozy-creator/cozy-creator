#!/usr/bin/env python3
"""shared_scratch (cl-984): an ordinary and an unlocked encrypted project run on the machine's executors, one code path, each
call in its own scratch inside its project. Paul (11-15): "I ONLY WANT file isolation / record isolation (i.e., the record of
runs); tensord processes running, executors, packages installed, and models downloaded are machine-specific"; once an
encrypted project is locked, nothing of it is on the computer outside it.

Each run writes its marker to a temporary file, its working directory and `~`, and prints it to stdout and stderr. The
proof: those paths are in its own project's `.vault-machine/scratch` (the encrypted one's view), that folder is gone after
the run, a run's live log is in its own project while it runs, a failed run's printed stderr is in its own triage, and
neither project's markers reach the other project, the machine's home (its executors' logs), the TensorFS store, `/tmp`
or `~`. The two projects' runs, one after the other, share one executor process; one sent while the other runs keeps
each in its own place. BiRefNet in the ordinary project, then in the encrypted one, runs on the same executor with its model
still loaded (no new executor, no load stage). After the encrypted project locks, none of its markers is outside it."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent))
from private_execution import command, scan  # noqa: E402
from stages import MAP, compiled, said  # noqa: E402

TARGET = "e2e-scratch/probe"
BIREFNET = "birefnet/remove_background"


def executors() -> set[int]:
    """This machine's executor processes now."""
    found = subprocess.run(["pgrep", "-f", "cozy_runtime.internal.executor"], capture_output=True, text=True, check=False)
    return {int(pid) for pid in found.stdout.split()}


def stages(body: bytes) -> list[str]:
    """Every stage a followed run went through, in order."""
    seen: list[str] = []
    for line in body.splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        stage = event.get("progress", {}).get("stage") if isinstance(event, dict) and event.get("type") == "run.progress" else None
        if isinstance(stage, str) and (not seen or seen[-1] != stage):
            seen.append(stage)
    return seen


def lock(cli: Path, out: Path, project: Path) -> None:
    """The encrypted project locked: its runs ended first, its view closed."""
    done = subprocess.run([str(cli), "--json", "vault", "lock"], cwd=project, capture_output=True, check=False)
    (out / "lock.json").write_bytes(done.stdout)
    for _ in range(120):
        status = subprocess.run([str(cli), "--json", "vault", "status"], cwd=project, capture_output=True, check=False)
        if json.loads(status.stdout or b"{}").get("state") == "locked":
            return
        time.sleep(0.5)
    raise AssertionError(f"the encrypted project did not lock: {done.stdout[-300:]!r}")


def answer(body: bytes) -> dict[str, object]:
    """The probe's reply, wherever the run's events carry it."""

    def find(value: object) -> dict[str, object] | None:
        if isinstance(value, dict):
            if {"pid", "temporary", "working", "home"} <= value.keys():
                return value
            return next((found for v in value.values() if (found := find(v)) is not None), None)
        if isinstance(value, list):
            return next((found for v in value if (found := find(v)) is not None), None)
        return None

    for line in reversed(body.splitlines()):
        try:
            found = find(json.loads(line))
        except ValueError:
            continue
        if found is not None:
            return found
    raise AssertionError("the run returned no probe reply")


def run(cli: Path, out: Path, name: str, project: Path, marker: str, *extra: str, fails: bool = False) -> bytes:
    return command(cli, out, name, "run", TARGET, f"marker={marker}", *extra, "--on", "local", cwd=project, fails=fails)


def placed(reply: dict[str, object], area: Path, label: str) -> None:
    """Every path the call wrote is in `area`'s scratch."""
    scratch = (area / ".vault-machine" / "scratch").resolve()
    for key in ("temporary", "working", "home"):
        path = Path(str(reply[key])).resolve()
        assert path.is_relative_to(scratch), f"{label}: its {key} file {path} is not in its project's scratch {scratch}"
        assert not path.exists(), f"{label}: its {key} file {path} outlived the call"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plain", required=True, type=Path)
    parser.add_argument("--encrypted", required=True, type=Path)
    parser.add_argument("--view", required=True, type=Path)
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--cli", type=Path, default=Path.home() / ".cozy/bin/cozy")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    plain, encrypted, view = args.plain.resolve(strict=True), args.encrypted.resolve(strict=True), args.view.resolve(strict=True)
    assert os.path.ismount(view), "the encrypted project's view is not a mount"
    stamp = time.time_ns() - 2_000_000_000
    unique = uuid.uuid4().hex
    a, b = f"CL984_PLAIN_{unique}", f"CL984_VAULT_{unique}"
    facts: dict[str, object] = {}

    # One after another: the encrypted project's run takes the executor the ordinary one's left idle.
    first = answer(run(args.cli, args.out, "plain-first", plain, f"{a}_1"))
    second = answer(run(args.cli, args.out, "vault-first", encrypted, f"{b}_1"))
    placed(first, plain, "the ordinary project's run")
    placed(second, view, "the encrypted project's run")
    facts["sequential_pids"] = [first["pid"], second["pid"]]
    assert first["pid"] == second["pid"], f"the encrypted project's run had an executor of its own ({second['pid']}, not {first['pid']})"

    # At once: the encrypted project's run sent while a long ordinary one runs. TensorD gives an executor one call at a
    # time: it runs on another executor or waits for this one (run 43686919083: queued, then pid 13418 again).
    long = subprocess.Popen([str(args.cli), "--json", "run", TARGET, f"marker={a}_2", "seconds:=30", "--on", "local"], cwd=plain,
                            stdout=(args.out / "plain-long.ndjson").open("wb"), stderr=(args.out / "plain-long.stderr").open("wb"))
    live = plain / ".vault-machine" / "scratch"
    deadline = time.monotonic() + 120
    logs: list[Path] = []
    while time.monotonic() < deadline and long.poll() is None:
        logs = [log for log in live.glob("*/stdout.log") if f"{a}_2".encode() in log.read_bytes()]
        if logs:
            break
        time.sleep(0.5)
    assert logs, "the ordinary project's running call has no log of its own in its project's scratch"
    sent = long.poll() is None
    beside = answer(run(args.cli, args.out, "vault-beside", encrypted, f"{b}_2"))
    assert sent, "the ordinary project's long run ended before the encrypted one was sent"
    placed(beside, view, "the encrypted project's run beside the ordinary one")
    assert long.wait(timeout=180) == 0, "the ordinary project's long run failed"
    longer = answer((args.out / "plain-long.ndjson").read_bytes())
    placed(longer, plain, "the ordinary project's long run")
    facts["concurrent_pids"] = [longer["pid"], beside["pid"]]
    facts["concurrent_queued"] = b"\"queue_position\":" in (args.out / "vault-beside.ndjson").read_bytes()

    # A failure: its stderr stays with its run, in its project.
    run(args.cli, args.out, "vault-failure", encrypted, f"{b}_3", "fail:=true", fails=True)

    # A run's folder goes once its call is done; a failed run's settles as its executor's reply is kept.
    for project, area in ((plain, plain), (encrypted, view)):
        left: list[str] = []
        for _ in range(30):
            left = sorted(str(p) for p in (area / ".vault-machine" / "scratch").glob("*"))
            if not left:
                break
            time.sleep(0.5)
        assert not left, f"{project}: its scratch keeps folders of ended calls: {left}"
    printed = f"{b}_3 stderr".encode()
    assert any(printed in path.read_bytes() for path in view.rglob("*") if path.is_file() and not path.is_symlink()), \
        "the encrypted project's failed run kept none of what it printed to stderr (its triage) in its project"

    # One executor, warm: BiRefNet in the ordinary project, then in the encrypted one, back to back. The second starts no
    # executor and loads nothing: the ordinary one's executor serves it with the model it holds.
    command(args.cli, args.out, "plain-birefnet", "run", BIREFNET, "--file", f"image={args.image}", "--on", "local", cwd=plain)
    held = executors()
    vault_birefnet = command(args.cli, args.out, "vault-birefnet", "run", BIREFNET, "--file", f"image={args.image}", "--on", "local", cwd=encrypted)
    after, went = executors(), stages(vault_birefnet)
    entries = compiled(MAP)
    loads = [stage for stage in went if stage.startswith(("importing ", "starting the executor"))
             or any((said(entries, part) or {}).get("phase") in ("loading", "starting") for part in stage.split(" / "))]
    facts["warm"] = {"executors": sorted(held), "after": sorted(after), "stages": went}
    assert after == held, f"the encrypted project's BiRefNet run had an executor of its own: {sorted(held)} then {sorted(after)}"
    assert not loads, f"the encrypted project's BiRefNet run loaded again on the shared executor: {loads}"

    # Locked, the encrypted project leaves nothing outside it; neither project's markers are outside it: the machine's home and
    # executors' logs, the store, /tmp, ~ and the other project.
    lock(args.cli, args.out, encrypted)
    assert not os.path.ismount(view), "the encrypted project's view is still mounted after lock"
    home = Path.home()
    store = Path(os.environ.get("TENSORFS_HOME", str(home / ".cache/tensorfs")))
    common = [home / ".cozy", store, Path("/tmp"), home / ".cache", home / ".config", home / ".local"]
    excluded = [home / ".cozy/calcifer/vaults", args.out.resolve()]
    plain_leaks, checked_a = scan([*common, encrypted], [*excluded, plain, view], stamp, [a.encode()])
    vault_leaks, checked_b = scan([*common, plain, encrypted], excluded, stamp, [b.encode()])
    facts |= {"checked": checked_a + checked_b, "plain_leaks": plain_leaks, "vault_leaks": vault_leaks}
    (args.out / "shared-scratch.json").write_text(json.dumps(facts, indent=2))
    print(f"shared scratch: {json.dumps(facts)}", flush=True)
    assert not (home / ".cache/e2e-scratch").exists(), "a call's `~` file reached the user's home"
    assert not plain_leaks, f"the ordinary project's markers outside it: {plain_leaks[:5]}"
    assert not vault_leaks, f"the encrypted project's markers outside its view: {vault_leaks[:5]}"


if __name__ == "__main__":
    main()
