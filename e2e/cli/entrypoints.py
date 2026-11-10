"""Check the small public CLI surface against the installed binary, without running models."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time


PUBLIC = {
    "new", "open", "run", "status", "machine", "model", "package", "push", "pull", "update", "doctor", "completion", "help",
}
HIDDEN = {"login", "logout", "whoami"}
REMOVED = {
    "org", "project", "import", "export", "add", "set", "delete", "render", "approve", "unapprove", "cancel",
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
    case = Path(tempfile.mkdtemp(prefix="case-", dir=out))
    outside = tempfile.TemporaryDirectory(prefix="cozy-entrypoints-outside-")
    empty = Path(outside.name)
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
        bare = run("bare requires project", [], code=2)
        outside_status = run("status requires project", ["status"], code=2)
        assert bare.stdout == outside_status.stdout and bare.stderr == outside_status.stderr, "bare cozy differs from status"
        help_text = run("explicit help", ["--help"]).stdout
        section = help_text.split("Commands:\n", 1)[1].split("\nOptions:", 1)[0]
        commands = set(re.findall(r"^  ([a-z][a-z-]*)\s", section, re.MULTILINE))
        assert commands == PUBLIC, f"public commands: {sorted(commands)}"
        assert "Exit codes:" not in help_text, "the removed exit-code footer is still shown"
        assert "--full" not in help_text and "--yes" not in help_text, "advanced root flags are visible"
        assert "next: cozy new my-project" in help_text
        assert "next: cd my-project && cozy open" in help_text
        assert "next: cozy run --help" in help_text
        run("hidden full flag", ["--full", "--help"])
        run("hidden yes flag", ["-y", "--help"])
        assert not (empty / "project.db").exists(), "a status or help read created a project"

        for command in REMOVED:
            run(f"removed {command}", [command, "--help"], code=2)
        for command in HIDDEN:
            run(f"hidden {command}", [command, "--help"])
        run("nested cancellation", ["run", "cancel", "--help"])
        run("bash completion", ["completion", "bash"])

        project = case / "film with spaces"
        created = json.loads(run("new directory", ["--json", "new", str(project)]).stdout)
        assert Path(created["path"]).resolve() == project
        assert (project / "project.db").is_file(), "new did not create project.db"
        refused = json.loads(run("new existing project", ["--json", "new"], project, code=1).stdout)
        assert refused["error"]["code"] == "project_exists", "new did not refuse an existing project"
        existing_directory = case / "existing directory"
        existing_directory.mkdir(exist_ok=True)
        initialized = json.loads(run("new current directory", ["--json", "new"], existing_directory).stdout)
        assert Path(initialized["path"]).resolve() == existing_directory
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
        bare_status = json.loads(run("bare project status", ["--json"], project).stdout)
        assert bare_status == status, "bare cozy JSON differs from status"
        assert "rentals" not in status, "project status contains account-wide rental totals"
        full = json.loads(run("full project diagnostics", ["--json", "--full", "status"], project).stdout)
        assert full["project"]["id"] == created["id"] and "calcifer" in full
        child = project / "subdirectory"
        child.mkdir(exist_ok=True)
        nested = json.loads(run("containing project", ["--json", "status"], child).stdout)
        assert nested["project"]["id"] == created["id"]
        run("removed run selector", ["run", "-p", str(project), "example/generate", "prompt=test"], project, code=2)
        run("removed open selector", ["open", str(project), "--no-open"], project, code=2)
        print(f"entrypoints: {len(records)} checks passed")
    finally:
        receipt = json.dumps(records, indent=2) + "\n"
        (case / "results.json").write_text(receipt)
        (out / "results.json").write_text(receipt)
        outside.cleanup()


if __name__ == "__main__":
    main()
