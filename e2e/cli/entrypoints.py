"""Check the small public CLI surface against the installed binary, without running models."""

import argparse
import json
import os
from pathlib import Path
from urllib.parse import urlsplit
import re
import subprocess
import tempfile
import time


PUBLIC = {
    "new", "open", "run", "status", "vault", "machine", "model", "package", "update", "doctor", "completion", "login", "logout", "help",
}
HIDDEN = {"whoami"}
REMOVED = {
    "push", "pull", "org", "project", "import", "export", "add", "set", "delete", "render", "approve", "unapprove", "cancel",
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
        bare = run("bare help", []).stdout
        help_text = run("explicit help", ["--help"]).stdout
        assert bare.strip() == help_text.strip(), "bare cozy differs from --help"
        section = help_text.split("Commands:\n", 1)[1].split("\nOptions:", 1)[0]
        commands = set(re.findall(r"^  ([a-z][a-z-]*)\s", section, re.MULTILINE))
        assert commands == PUBLIC, f"public commands: {sorted(commands)}"
        assert "Exit codes:" not in help_text, "the removed exit-code footer is still shown"
        assert "--full" not in help_text and "--yes" not in help_text, "advanced root flags are visible"
        assert 'next: cozy new "<directory>"' in help_text
        assert "next: cozy open" in help_text
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
        assert created["created"] is True, "new did not report creating the project"
        again = json.loads(run("new existing project", ["--json", "new"], project).stdout)
        assert again["created"] is False and again["id"] == created["id"], "repeating new replaced project identity"
        existing_directory = case / "existing directory"
        existing_directory.mkdir(exist_ok=True)
        initialized = json.loads(run("new current directory", ["--json", "new"], existing_directory).stdout)
        assert Path(initialized["path"]).resolve() == existing_directory
        assert initialized["created"] is True
        environment["COZY_PROJECT"] = str(project)

        for name, words in (
            ("status requires project", ["--json", "status"]),
            ("open requires project", ["--json", "open"]),
            ("run requires project", ["--json", "run", "example/generate", "prompt=test"]),
            ("run list requires project", ["--json", "run", "ls"]),
        ):
            result = run(name, words, code=2)
            refusal = json.loads(result.stdout)["error"]
            assert refusal["code"] == "not_in_a_project", f"{name}: {refusal}"
            assert "cozy new" in json.dumps(refusal), f"{name}: no creation guidance"
        assert not (empty / "project.db").exists(), "a read or refused run initialized a project"
        plain_error = run("default project refusal", ["status"], code=2)
        refusal_text = plain_error.stdout + plain_error.stderr
        assert not refusal_text.lstrip().startswith("{"), "default refusal is JSON"
        assert "not_in_a_project" in refusal_text and "cozy new" in refusal_text

        status = json.loads(run("project status", ["--json", "status"], project).stdout)
        assert Path(status["project"]["path"]).resolve() == project
        assert status["health"] == "ready" and status["warnings"] == []
        for field in ("unrendered_assets", "outdated_assets", "missing_files", "warnings"):
            assert status["counts"][field] == 0, f"new project health count {field} is nonzero"
        assert not ({"runs", "machines", "rentals"} & status.keys()), "health status contains activity or account totals"
        assert run("bare help in project", [], project).stdout.strip() == help_text.strip()
        plain = run("default health output", ["status"], project).stdout
        assert not plain.lstrip().startswith("{"), "default health output is JSON"
        assert "health" in plain and "ready" in plain
        full = json.loads(run("full project health", ["--json", "--full", "status"], project).stdout)
        assert full["project"]["id"] == created["id"] and full["project"]["assets"] == []
        child = project / "subdirectory"
        child.mkdir(exist_ok=True)
        nested = json.loads(run("containing project", ["--json", "status"], child).stdout)
        assert Path(nested["project"]["path"]).resolve() == project
        opened = json.loads(run("project URL", ["--json", "open"], project).stdout)
        assert set(opened) == {"url", "expires_ms"}, "open includes fields beyond the URL and expiry"
        url = urlsplit(opened["url"])
        assert url.hostname in {"127.0.0.1", "localhost", "::1"}, "open did not print a local URL"
        assert "code=" in url.fragment and f"to=/projects/{created['id']}" in url.fragment
        assert opened["expires_ms"] > 0
        plain_url = run("default project URL", ["open"], project).stdout
        assert not plain_url.lstrip().startswith("{") and "url" in plain_url and "expires_ms" in plain_url
        for flag in ("--no-open", "--print-url"):
            run(f"removed open flag {flag}", ["open", flag], project, code=2)
        run("removed run selector", ["run", "-p", str(project), "example/generate", "prompt=test"], project, code=2)
        run("removed open selector", ["open", str(project)], project, code=2)
        print(f"entrypoints: {len(records)} checks passed")
    finally:
        receipt = json.dumps(records, indent=2) + "\n"
        (case / "results.json").write_text(receipt)
        (out / "results.json").write_text(receipt)
        outside.cleanup()


if __name__ == "__main__":
    main()
