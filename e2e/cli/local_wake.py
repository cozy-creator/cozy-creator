"""Real authorized-controller rebinds contending with local binary updates, on a fresh CI runner.

The existing startup log schedules an ordinary File update in the observed narrow window.
Update calls local_up before checking for no change; completion, a fresh worker and exact
candidate identity are all required. No model, production hook, fake binary or process kill.
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import subprocess
import threading
import time
import tomllib
from typing import TypeAlias, cast
from urllib.parse import quote
import uuid


Json: TypeAlias = str | int | float | bool | None | list["Json"] | dict[str, "Json"]
Row: TypeAlias = dict[str, Json]
START = re.compile(r"starting this computer's machine pid=(\d+)")
OWNER = re.compile(r"another machine owns the root .* \(pid (\d+)\)")


def row(value: Json) -> Row:
    if not isinstance(value, dict):
        raise ValueError("expected an API object")
    return value


def text(value: Json) -> str:
    if not isinstance(value, str):
        raise ValueError("expected API text")
    return value


def load(path: Path) -> Row:
    return row(cast(Json, json.loads(path.read_text())))


@dataclass
class Reply:
    status: int
    body: Json

    def error(self) -> str:
        if not isinstance(self.body, dict):
            return ""
        error = self.body.get("error")
        return text(error.get("code", "")) if isinstance(error, dict) else ""


@dataclass
class Context:
    address: str
    token: str = field(repr=False)


class Session:
    def __init__(self, context: Context) -> None:
        self.context = context
        self.connection = http.client.HTTPConnection(context.address, timeout=30)
        self.connection.connect()

    def call(self, method: str, path: str, body: Json = None, raw: bytes | None = None,
             sent: threading.Event | None = None) -> Reply:
        data = raw if raw is not None else json.dumps(body).encode() if body is not None else None
        headers = {"Authorization": "Bearer " + self.context.token}
        if data is not None:
            headers.update({"Content-Type": "application/octet-stream" if raw is not None else "application/json",
                            "Content-Length": str(len(data))})
        if method == "POST":
            headers["Idempotency-Key"] = "wake-fixture-" + uuid.uuid4().hex
        try:
            self.connection.request(method, path, body=data, headers=headers)
            if sent is not None:
                sent.set()
            answer = self.connection.getresponse()
            payload = answer.read()
            return Reply(answer.status, cast(Json, json.loads(payload)) if payload else None)
        except (OSError, http.client.HTTPException) as error:
            return Reply(0, {"error": {"code": "fixture_transport", "message": type(error).__name__}})

    def close(self) -> None:
        self.connection.close()


class Proof:
    def __init__(self, cli: Path, installed: Path, output: Path) -> None:
        self.cli, self.installed, self.out = cli, installed, output
        self.home = Path.home() / ".cozy"
        holder = load(self.home / "calcifer/calcifer.lock.holder")
        self.context = Context(text(holder["address"]), text(holder["token"]))
        pid = holder["pid"]
        if not isinstance(pid, int):
            raise ValueError("the fixture Calcifer has no process ID")
        self.controller = Path(f"/proc/{pid}/exe")
        self.candidate_sha = hashlib.sha256(cli.read_bytes()).hexdigest()
        if hashlib.sha256(self.controller.read_bytes()).hexdigest() != self.candidate_sha:
            raise ValueError("the fixture Calcifer is not the current candidate")
        config = row(cast(Json, tomllib.loads((self.home / "config.toml").read_text())))
        machine = row(config.get("machine", {}))
        if machine.get("listen_host") or os.environ.get("COZY_LISTEN_HOST"):
            raise ValueError("the rebind fixture needs the ordinary automatic listen address")
        configured = os.environ.get("COZY_MACHINE_ROOT") or machine.get("root")
        self.root = Path(text(configured)).resolve() if configured else self.home / "machine"
        if not self.root.is_relative_to(self.home.resolve()):
            raise ValueError("the fixture machine root must belong to this fresh runner's Cozy home")
        self.log = self.home / "calcifer/calcifer.log"
        self.start_log = self.root / "var/lib/cozy/machine/start.log"
        self.ready = self.root / "var/lib/cozy/tensord/api-ready.json"
        self.failures: list[Json] = []
        self.episodes: list[Json] = []
        self.exact_failure = False
        self.owned_root_refused = False
        self.expected_commit = ""
        self.expected_version = ""

    def call(self, method: str, path: str, body: Json = None, raw: bytes | None = None) -> Reply:
        session = Session(self.context)
        try:
            return session.call(method, path, body, raw)
        finally:
            session.close()

    def command(self, label: str, arguments: list[str]) -> subprocess.CompletedProcess[str]:
        result = subprocess.run([str(self.cli), *arguments], capture_output=True, text=True, check=False)
        for suffix, value in [("out", result.stdout), ("err", result.stderr)]:
            (self.out / f"{label}.{suffix}").write_text(value.replace(self.context.token, "<REDACTED>"))
        return result

    def wait_ready(self, opened: bool) -> Row:
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            reply = self.call("GET", "/v1/machines/local")
            if reply.error() == "machine_start_failed":
                self.failures.append({"stage": "readiness", "code": reply.error()})
            if reply.status == 200:
                machine = row(reply.body)
                status = machine.get("status")
                address = text(load(self.ready)["address"]) if self.ready.is_file() else ""
                if isinstance(status, dict) and machine.get("state") == "ready" and status.get("phase") == "ready" and address.startswith("0.0.0.0:" if opened else "127.0.0.1:"):
                    return status
            time.sleep(0.025)
        raise ValueError("the actual worker did not finish its controller rebind")

    def terminal(self, reply: Reply, label: str) -> tuple[str, str, int]:
        if reply.status not in (200, 202):
            return "refused", reply.error(), 0
        run = row(row(reply.body).get("run"))
        number = run.get("number")
        if not isinstance(number, int) or number == 0:
            raise ValueError("the File software update did not create a real run")
        watched = self.command(label, ["--json", "--full", "run", "watch", str(number)])
        for line in watched.stdout.splitlines():
            if not line.startswith("{"):
                continue
            event = row(cast(Json, json.loads(line)))
            candidate = event.get("run", event)
            if isinstance(candidate, dict) and candidate.get("number") == number:
                run = candidate
        if run.get("kind") != "update":
            raise ValueError("the followed request was not a software update")
        reason = run.get("reason")
        code = text(reason.get("code", "")) if isinstance(reason, dict) else ""
        return text(run.get("status", "")), code, number

    def episode(self, index: int, key: str, update: Row) -> None:
        path = "/v1/machines/local/authorized-keys/" + quote(key, safe="")
        removed = self.call("DELETE", path)
        if removed.status != 200:
            raise ValueError("the control authorization removal failed")
        before = self.wait_ready(False)
        if hashlib.sha256(self.controller.read_bytes()).hexdigest() != self.candidate_sha:
            raise ValueError("the fixture no longer runs the candidate Calcifer")
        offset, start_offset = self.log.stat().st_size, self.start_log.stat().st_size
        sent = threading.Event()

        def update_after_start() -> Reply:
            session = Session(self.context)  # Connected before the event: no CLI-spawn latency in the 8–21 ms race.
            try:
                deadline = time.monotonic() + 30
                with self.log.open("rb") as stream:
                    stream.seek(offset)
                    pending = b""
                    while time.monotonic() < deadline:
                        lines = (pending + stream.read()).split(b"\n")
                        pending = lines.pop()
                        for raw in lines:
                            line = raw.decode(errors="replace")
                            match = START.search(line)
                            if match:
                                return session.call("POST", "/v1/machines/local/update", update, sent=sent)
                        time.sleep(0.001)
                return Reply(0, {"error": {"code": "fixture_no_automatic_start"}})
            finally:
                sent.set()
                session.close()

        def status_calls() -> list[Json]:
            if not sent.wait(30):
                return [{"code": "fixture_no_update_request"}]
            replies: list[Json] = []
            for _ in range(8):
                reply = self.call("GET", "/v1/machines/local")
                replies.append({"http": reply.status, "code": reply.error()})
            return replies

        with ThreadPoolExecutor(max_workers=2) as pool:
            updating = pool.submit(update_after_start)
            statuses = pool.submit(status_calls)
            authorized = self.call("PUT", path, {})
            reply = updating.result()
            state, code, number = self.terminal(reply, f"episode-{index}-update")
            status_results = statuses.result()
        after = self.wait_ready(True)
        installed = hashlib.sha256(self.installed.read_bytes()).hexdigest()
        completed_here = (state == "completed" and after.get("version") == self.expected_version
                          and after.get("commit") == self.expected_commit and installed == self.candidate_sha
                          and after.get("started_at_ms") != before.get("started_at_ms"))
        starts: list[Json] = [int(match[1]) for match in START.finditer(self.log.read_bytes()[offset:].decode(errors="replace"))]
        errors: list[Json] = [int(match[1]) for match in OWNER.finditer(self.start_log.read_bytes()[start_offset:].decode(errors="replace"))]
        owner_pid = load(self.root / "var/lib/cozy/machine/agent.lock.holder").get("pid")
        observed = (code == "machine_start_failed" and len(starts) >= 2 and owner_pid in errors
                    and after.get("started_at_ms") != before.get("started_at_ms"))
        self.exact_failure |= observed
        self.episodes.append({"episode": index, "authorized_http": authorized.status, "update_http": reply.status,
                              "update_run": number, "update_state": state, "update_error": code,
                              "status_replies": status_results, "start_pids": starts, "root_owner_refusals": errors,
                              "ready_root_owner_pid": owner_pid,
                              "before_started_at_ms": before.get("started_at_ms"), "after_started_at_ms": after.get("started_at_ms"),
                              "before_version": before.get("version"), "before_dev": before.get("dev"),
                              "version": after.get("version"), "commit": after.get("commit"),
                              "actual_update_completed_on_candidate": completed_here, "exact_observed_failure": observed})
        if not completed_here or code or authorized.status != 200:
            self.failures.append({"stage": "update", "episode": index, "state": state, "code": code})
        if errors or not starts:
            self.failures.append({"stage": "single_start", "episode": index, "starts": len(starts), "owner_refusals": len(errors)})
        if any(isinstance(value, dict) and value.get("code") == "machine_start_failed" for value in status_results):
            self.failures.append({"stage": "status_start", "episode": index})

    def run(self) -> int:
        initial = row(self.call("GET", "/v1/machines/local").body)
        initial_status = row(initial.get("status"))
        if initial_status.get("gpus") != []:
            raise ValueError("this regression is CPU-only")
        self.expected_commit = text(initial_status.get("commit"))
        key = text(initial["controller_key"])
        # Public product bytes, staged through exactly the Files API used by the CLI's update component.
        binary = self.cli.read_bytes()
        digest = "sha256:" + hashlib.sha256(binary).hexdigest()
        version = self.command("version", ["--version"]).stdout.strip().removeprefix("cozy ")
        self.expected_version = version
        staged = self.call("PUT", "/v1/files/" + digest, raw=binary)
        if staged.status not in (200, 201, 204):
            raise ValueError("the actual candidate binary was not staged")
        update: Row = {"tensord": {"file": {"sha256": digest, "name": "cozy", "version": version}}}
        try:
            for index in range(1, 6):
                self.episode(index, key, update)
                if self.exact_failure:
                    break
            duplicate = self.command("owned-root-refusal", ["machine", "serve"])
            refused = duplicate.returncode != 0 and "another machine owns the root" in duplicate.stdout + duplicate.stderr
            self.owned_root_refused = refused
            if not refused:
                self.failures.append({"stage": "real_owned_root_was_not_refused"})
        except (ValueError, OSError, http.client.HTTPException) as error:
            self.failures.append({"stage": "fixture", "error": type(error).__name__,
                                  "message": str(error).replace(self.context.token, "<REDACTED>")})
        finally:
            self.call("DELETE", "/v1/machines/local/authorized-keys/" + quote(key, safe=""))
        result: Row = {"passed": not self.failures, "exact_observed_failure": self.exact_failure,
                       "episodes": self.episodes, "failures": self.failures, "binary_sha256": digest,
                       "candidate_version": version, "candidate_controller_sha256": self.candidate_sha,
                       "real_owned_root_refused": self.owned_root_refused,
                       "no_models": True, "no_hooks_or_kills": True}
        (self.out / "receipt.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))
        return 1 if self.failures else 0


class Args(argparse.Namespace):
    cli: Path
    installed: Path
    out: Path


def main() -> int:
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_OS") != "Linux":
        raise ValueError("this real lifecycle fixture runs only on a fresh Linux GitHub runner")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", required=True, type=Path)
    parser.add_argument("--installed", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = cast(Args, parser.parse_args(namespace=Args()))
    args.out.mkdir(parents=True, exist_ok=True)
    try:
        return Proof(args.cli, args.installed, args.out).run()
    except (ValueError, OSError, http.client.HTTPException) as error:
        (args.out / "receipt.json").write_text(json.dumps({"passed": False, "setup_error": type(error).__name__, "message": str(error)}) + "\n")
        raise


if __name__ == "__main__":
    raise SystemExit(main())
