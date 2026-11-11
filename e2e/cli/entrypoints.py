"""Check the small public CLI surface against the installed binary, without running models."""

import argparse
import html
import json
import os
from pathlib import Path
from urllib.parse import urlsplit
import re
import subprocess
import sys
import tempfile
import time


PUBLIC = {
    "new", "open", "run", "status", "vault", "machine", "model", "package", "update", "doctor", "completion", "login", "logout", "whoami",
}
REMOVED = {
    "help",
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
    browser_log = case / "browser-opened.txt"
    if os.name == "posix":
        browser_bin = case / "browser-bin"
        browser_bin.mkdir()
        browser = browser_bin / ("open" if sys.platform == "darwin" else "xdg-open")
        browser.write_text("#!/bin/sh\nprintf '%s\\n' \"$1\" >> \"$COZY_TEST_BROWSER_LOG\"\n")
        browser.chmod(0o755)
        environment["PATH"] = str(browser_bin) + os.pathsep + environment.get("PATH", "")
        environment["COZY_TEST_BROWSER_LOG"] = str(browser_log)

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
        assert "--full" in help_text and "--yes" in help_text, "global options are missing from help"
        assert 'next: cozy new "<directory>"' in help_text
        assert "next: cozy open" in help_text
        assert "next: cozy run --help" in help_text
        run("full flag", ["--full", "--help"])
        run("yes flag", ["-y", "--help"])
        assert not (empty / "project.db").exists(), "a status or help read created a project"

        for command in REMOVED:
            run(f"removed {command}", [command, "--help"], code=2)
        run("account help", ["whoami", "--help"])
        run_help = run("run help", ["run", "--help"]).stdout
        assert "cozy run <ls|watch|cancel>" in run_help, "run usage does not name its subcommands"
        assert "<COMMAND>" not in run_help, "run usage still uses the unexplained COMMAND placeholder"
        run("nested cancellation", ["run", "cancel", "--help"])
        assert "Usage: cozy run watch" in run("watch help", ["run", "watch", "--help"]).stdout
        run("removed run show", ["run", "show", "1", "--once"], code=2)
        run("removed run play", ["run", "play", "1", "--expires", "1h"], code=2)
        run("internal controller help", ["calcifer", "--help"])
        run("internal worker help", ["machine", "serve", "--help"])
        run("removed worker bootstrap flag", ["machine", "serve", "--service-ready", "1"], code=2)
        run("removed image override", ["machine", "rent", "example", "--image-digest", "sha256:" + "0" * 64], code=2)
        run("removed memo bypass", ["run", "example/function", "--no-memo"], code=2)
        for flag in ("--estimate", "--warm"):
            assert flag not in run_help, f"removed run flag {flag} is still in help"
            run(f"removed run flag {flag}", ["run", "example/function", flag], code=2)
        assert "--model <SLOT=REF>" in run_help
        for value in ("slot", "=ref", "slot=", " =ref", "slot= "):
            run(f"invalid model override {value!r}", ["run", "example/function", "--model", value], code=2)
        for words in (
            ["machine", "help"],
            ["machine", "rent", "--unknown-option"],
            ["run", "example/function", "ls", "--unknown-option"],
        ):
            error = json.loads(run("usage remedy " + " ".join(words), ["--json", *words], code=2).stdout)["error"]
            reference = error["remedy"]
            help_words = ["machine", "rent", "--help"] if words[:2] == ["machine", "rent"] else [words[0], "--help"]
            expected = "cozy " + " ".join(help_words)
            assert reference == expected, "usage remedy must name a concise help command"
            run("reference " + " ".join(help_words), help_words)
        run("removed nested help", ["machine", "help"], code=2)
        run("local port option", ["open", "--port", "8818", "--help"])
        run("bash completion", ["completion", "bash"])

        project = case / "film with spaces"
        created = json.loads(run("new directory", ["--json", "new", str(project)]).stdout)
        assert Path(created["path"]).resolve() == project
        assert (project / "project.db").is_file(), "new did not create project.db"
        assert created["created"] is True, "new did not report creating the project"
        assert re.fullmatch(r"[0123456789ABCDEFGHJKMNPQRSTVWXYZ]{9}", created["code"])
        assert re.search(r"[A-Z]", created["code"]), "project code has no letter"
        again = json.loads(run("new existing project", ["--json", "new"], project).stdout)
        assert again["created"] is False and again["id"] == created["id"], "repeating new replaced project identity"
        assert again["code"] == created["code"], "repeating new replaced public project code"
        friendly = run("friendly existing project", ["new"], project).stdout
        assert "Project not created." in friendly and created["code"] in friendly
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
        listed = json.loads(run("empty project runs", ["--json", "run", "ls"], project).stdout)
        assert listed["runs"] == [], "a new project has unexpected runs"
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
        assert set(opened) == {"project", "url"}, "open should show the project name and URL"
        assert opened["project"] == project.name
        url = urlsplit(opened["url"])
        assert url.hostname in {"127.0.0.1", "localhost", "::1"}, "open did not print a local URL"
        assert url.path == f"/p/{created['code']}" and url.fragment.startswith("code=")
        plain_url = run("default project URL", ["open"], project).stdout
        assert not plain_url.lstrip().startswith("{") and "url" in plain_url and project.name in plain_url
        assert "expires_ms" not in plain_url
        if os.name == "posix":
            until = time.monotonic() + 5
            opened_files: list[str]  # the loop runs at least once: its deadline is ahead
            while time.monotonic() < until:
                opened_files = browser_log.read_text().splitlines() if browser_log.exists() else []
                if len(opened_files) >= 2:
                    break
                time.sleep(0.05)
            assert len(opened_files) >= 2, "open did not launch the default browser"
            first = Path(opened_files[0])
            assert opened["url"] in html.unescape(first.read_text()), "browser received a different URL"
            assert first.stat().st_mode & 0o777 == 0o600, "browser redirect exposes its sign-in code"
            saved_path = environment["PATH"]
            empty_path = case / "empty-path"
            empty_path.mkdir()
            environment["PATH"] = str(empty_path)
            try:
                fallback = run("browser unavailable fallback", ["--json", "open"], project)
            finally:
                environment["PATH"] = saved_path
            assert json.loads(fallback.stdout)["project"] == project.name
            assert "could not open the browser" in fallback.stderr
        for flag in ("--no-open", "--print-url"):
            run(f"removed open flag {flag}", ["open", flag], project, code=2)
        run("removed run selector", ["run", "-p", str(project), "example/generate", "prompt=test"], project, code=2)
        run("removed open selector", ["open", str(project)], project, code=2)
        readable_errors = subprocess.run(
            [sys.executable, str(Path(__file__).with_name("errors.py")), "--cozy", binary],
            capture_output=True, text=True,
        )
        assert readable_errors.returncode == 0, readable_errors.stdout + readable_errors.stderr
        print(f"entrypoints: {len(records)} checks passed")
    finally:
        receipt = json.dumps(records, indent=2) + "\n"
        (case / "results.json").write_text(receipt)
        (out / "results.json").write_text(receipt)
        if browser_log.exists():
            for file in browser_log.read_text().splitlines():
                Path(file).unlink(missing_ok=True)
        outside.cleanup()


if __name__ == "__main__":
    main()
