"""Check the small public CLI surface against the installed binary, without running models."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time


PUBLIC = {
    "new", "open", "run", "status", "machine", "model", "package", "push", "pull", "update", "doctor", "help",
}
HIDDEN = {"login", "logout", "whoami", "org", "completion"}
REMOVED = {
    "project", "import", "export", "add", "set", "delete", "render", "approve", "unapprove", "cancel",
    "wait", "history", "undo", "redo", "rename", "reclaim",
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cozy", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    binary = str(Path(args.cozy).resolve())
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    empty = out / "outside"
    empty.mkdir(exist_ok=True)
    records = []
    environment = os.environ.copy()

    def run(name: str, words: list[str], directory: Path = empty, code: int = 0) -> subprocess.CompletedProcess[str]:
        if log := environment.get("ARGV_LOG"):
            with open(log, "a", encoding="utf-8") as stream:
                stream.write("\x1f".join(words) + "\x1f\x1e")
        started = time.monotonic()
        result = subprocess.run([binary, *words], cwd=directory, env=environment, capture_output=True, text=True)
        records.append({"name": name, "exit": result.returncode, "elapsed_ms": round((time.monotonic() - started) * 1000)})
        assert result.returncode == code, f"{name}: expected exit {code}, got {result.returncode}: {result.stdout}{result.stderr}"
        return result

    try:
        bare = run("bare help", []).stdout
        help_text = run("explicit help", ["--help"]).stdout
        assert bare.strip() == help_text.strip(), "bare cozy differs from --help"
        section = bare.split("Commands:\n", 1)[1].split("\nOptions:", 1)[0]
        commands = set(re.findall(r"^  ([a-z][a-z-]*)\s", section, re.MULTILINE))
        assert commands == PUBLIC, f"public commands: {sorted(commands)}"
        assert not (empty / "project.db").exists(), "help created a project"

        for command in REMOVED:
            run(f"removed {command}", [command, "--help"], code=2)
        for command in HIDDEN:
            run(f"hidden {command}", [command, "--help"])
        run("nested cancellation", ["run", "cancel", "--help"])
        run("bash completion", ["completion", "bash"])

        project = out / "film with spaces"
        created = json.loads(run("new directory", ["--json", "new", str(project)]).stdout)
        assert Path(created["path"]).resolve() == project
        assert (project / "project.db").is_file(), "new did not create project.db"
        again = json.loads(run("new existing project", ["--json", "new"], project).stdout)
        assert again["id"] == created["id"], "new replaced an existing project's identity"
        environment["COZY_PROJECT"] = str(project)

        for name, words in (
            ("status requires project", ["--json", "status"]),
            ("open requires project", ["--json", "open", "--no-open"]),
            ("run requires project", ["--json", "run", "example/generate", "prompt=test"]),
            ("run list requires project", ["--json", "run", "ls"]),
        ):
            result = run(name, words, code=2)
            refusal = json.loads(result.stdout)["error"]
            assert refusal["code"] == "not_in_a_project", f"{name}: {refusal}"
            assert "cozy new" in json.dumps(refusal), f"{name}: no creation guidance"
        assert not (empty / "project.db").exists(), "a read or refused run initialized a project"

        status = json.loads(run("project status", ["--json", "status"], project).stdout)
        assert status["project"]["id"] == created["id"]
        assert Path(status["project"]["path"]).resolve() == project
        assert status["runs"] == [] and status["machines"] == [], "new project inherited another project's activity"
        assert "rentals" not in status, "project status contains account-wide rental totals"
        child = project / "subdirectory"
        child.mkdir(exist_ok=True)
        nested = json.loads(run("containing project", ["--json", "status"], child).stdout)
        assert nested["project"]["id"] == created["id"]
        run("removed run selector", ["run", "-p", str(project), "example/generate", "prompt=test"], project, code=2)
        run("removed open selector", ["open", str(project), "--no-open"], project, code=2)
        print(f"entrypoints: {len(records)} checks passed")
    finally:
        (out / "results.json").write_text(json.dumps(records, indent=2) + "\n")


if __name__ == "__main__":
    main()
