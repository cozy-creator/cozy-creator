"""Render a box, then cancel it within 150 ms, many times (web-account-rent, run 43401337115: a cancel 80 ms after Render answered
`dropped`, and the run still started, completed and landed). Whatever a cancel answers, nothing of that render lands: a job dropped
before it was sent is never sent, and a run already sent ends canceled at its machine."""
from __future__ import annotations

import json
from pathlib import Path
import sys
import time
from typing import NotRequired, TypedDict, cast

from project_http import project_path, request

ROUNDS = 32
SETTLE_S = 90


class Asset(TypedDict):
    name: str
    versions: int


class Job(TypedDict):
    asset: str
    run: NotRequired[int | None]


class Board(TypedDict):
    jobs: list[Job]


class ProjectState(TypedDict):
    """The project route's answer: the fields this reads."""

    assets: list[Asset]
    busy: bool
    queue: NotRequired[list[object]]
    board: NotRequired[Board]


def main() -> int:
    project, output, tool = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]

    def status() -> ProjectState:
        return cast(ProjectState, json.loads(request(project_path(project))))  # cast: the project route's JSON

    def versions(state: ProjectState) -> int:
        return next((asset["versions"] for asset in state["assets"] if asset["name"] == "shot"), 0)

    def edit(path: str, method: str, body: dict[str, object]) -> str:
        return request(project_path(project, path), method, body)

    def settled() -> ProjectState:
        deadline = time.monotonic() + SETTLE_S
        state: ProjectState | None = None
        while time.monotonic() < deadline:
            state = status()
            if not state["busy"] and not state.get("queue"):
                return state
            time.sleep(0.05)
        raise RuntimeError(f"the project's work did not settle: {state}")

    def run_status(number: int) -> str:
        return cast(str, json.loads(request(project_path(project, f"/runs/{number}")))["status"])  # cast: a run's JSON

    edit("/assets", "POST", {"tool": tool, "name": "shot", "exact": True, "fields": {"seconds": 4, "instructions": "Round 0."}})
    # One that lands first: its package is installed, so a render's send takes milliseconds and races the cancel.
    edit("/runs", "POST", {"names": ["shot"], "on": ["local"]})
    before = versions(settled())
    if before != 1:
        raise RuntimeError(f"the first render made {before} versions, not 1")
    rounds: list[dict[str, object]] = []
    for at in range(1, ROUNDS + 1):
        edit("/assets/shot", "PATCH", {"fields": {"instructions": f"Round {at}."}})
        delay = (at % 16) * 0.01
        asked = json.loads(edit("/runs", "POST", {"names": ["shot"], "on": ["local"]}))
        time.sleep(delay)
        answer = json.loads(edit("/cancel", "POST", {"names": ["shot"]}))
        state = settled()
        runs = sorted({run for job in state.get("board", {}).get("jobs", []) if job["asset"] == "shot" and (run := job.get("run"))})
        ended = {number: run_status(number) for number in runs}
        rounds.append({"round": at, "delay_ms": round(delay * 1000), "added": asked.get("added"), "cancel": answer,
                       "versions": versions(state), "runs": ended})
    after = versions(status())
    landed = [entry for entry in rounds if cast(int, entry["versions"]) > before]
    finished = [entry for entry in rounds if any(state == "completed" for state in cast(dict[int, str], entry["runs"]).values())]
    report = {
        "rounds": len(rounds),
        "dropped": sum(1 for entry in rounds if cast(dict[str, list[object]], entry["cancel"]).get("dropped")),
        "canceled_runs": sum(1 for entry in rounds if cast(dict[str, list[object]], entry["cancel"]).get("canceled")),
        "versions_before": before,
        "versions_after": after,
        "landed": landed[:3],
        "completed_runs": finished[:3],
        "passed": after == before and not landed and not finished,
    }
    (output / "cancel-race-rounds.json").write_text(json.dumps(rounds, indent=1))
    print(json.dumps(report))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(json.dumps({"inconclusive": str(error)}))
        raise
