"""Check the small public CLI surface against the installed binary, without running models."""

import argparse
import json
import os
from pathlib import Path
from urllib.parse import urlsplit
import urllib.error
import urllib.request
import re
import subprocess
import sys
import tempfile
import time


PUBLIC = {
    "new",
    "open",
    "run",
    "get",
    "apply",
    "diff",
    "delete",
    "explain",
    "render",
    "status",
    "vault",
    "machine",
    "model",
    "lora",
    "package",
    "update",
    "restart",
    "doctor",
    "completion",
    "login",
    "logout",
    "whoami",
}
REMOVED = {
    "help",
    "push", "pull", "org", "project", "import", "export", "add", "set", "approve", "unapprove", "cancel",
    "wait", "history", "undo", "redo", "rename", "reclaim",
}


def public_usage(text: str) -> str:
    if os.name == "nt":
        return re.sub(r"(?m)^(Usage: | {7})cozy\.exe(?= |$)", r"\1cozy", text)
    return text


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
        record = {"name": name, "exit": result.returncode, "elapsed_ms": round((time.monotonic() - started) * 1000)}
        if not words or "--help" in words:
            record.update(stdout=result.stdout, stderr=result.stderr)
        records.append(record)
        if "--help" in words:
            result.stdout = public_usage(result.stdout)
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
        run("removed model info", ["model", "info", "--help"], code=2)
        run("model catalog pagination", ["model", "ls", "--offset", "20", "--limit", "10", "--help"])
        run("LoRA catalog help", ["lora", "ls", "--help"])
        run("model unpublish help", ["model", "unpublish", "--help"])
        run("LoRA unpublish help", ["lora", "unpublish", "--help"])
        run("LoRA download help", ["lora", "install", "--help"])
        run("local model removal help", ["model", "uninstall", "--help"])
        run("local LoRA removal help", ["lora", "uninstall", "--help"])
        for group in ("model", "lora"):
            group_help = run(f"{group} help", [group, "--help"]).stdout
            for old in ("download", "remove"):
                run(f"removed {group} {old}", [group, old, "--help"], code=2)
            for hidden in ("upload", "publish", "quantize"):
                assert not re.search(rf"^  {hidden}\s", group_help, re.MULTILINE), f"{group} {hidden} is still public"
        package_help = run("package help", ["package", "--help"]).stdout
        assert not re.search(r"^  publish\s", package_help, re.MULTILINE), "package publish is still public"
        assert re.search(r"^  uninstall\s", package_help, re.MULTILINE), "package uninstall is missing from help"
        assert not re.search(r"^  remove\s", package_help, re.MULTILINE), "retired package remove is still listed"
        run("package uninstall help", ["package", "uninstall", "--help"])
        run("removed package remove", ["package", "remove", "--help"], code=2)
        run("package delete help", ["package", "delete", "--help"])
        run("removed package lock", ["package", "lock", "--help"], code=2)
        run("removed package info", ["package", "info", "--help"], code=2)
        run("package pagination help", ["package", "ls", "--offset", "20", "--limit", "10", "--help"])
        run("LoRA metadata help", ["lora", "edit", "--help"])
        run("LoRA upload help", ["lora", "upload", "--help"])
        run("LoRA cloud deletion help", ["lora", "delete", "--help"])
        run("removed model trigger", ["model", "edit", "author/model", "--trigger", "word"], code=2)
        run("removed model upload strength", ["model", "upload", "file", "author/model", "--lane", "bf16", "--strength", "0.5"], code=2)
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

        missing_credential = case / "encrypted without credential"
        refused = json.loads(run("new requires secure input", ["--json", "new", str(missing_credential)], code=2).stdout)
        assert refused["error"]["code"] == "vault_credential"
        assert not missing_credential.exists(), "refused new created a project without a known credential"
        project = case / "film with spaces"
        created = json.loads(run("new directory", ["--json", "new", "--unencrypted", str(project)]).stdout)
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
        initialized = json.loads(run("new current directory", ["--json", "new", "--unencrypted"], existing_directory).stdout)
        assert Path(initialized["path"]).resolve() == existing_directory
        assert initialized["created"] is True
        # cl-980: `cozy new` and the page's New are one create, Calcifer's `POST /v1/projects {path}`: what the CLI made is in the index
        # the page lists, and a create by name alone, the page's old second way, is gone.
        holder = json.loads((Path.home() / ".cozy" / "calcifer" / "calcifer.lock.holder").read_text())
        def calcifer(method: str, path: str, body: object = None) -> tuple[int, str]:
            request = urllib.request.Request(f"http://{holder['address']}{path}", method=method,
                data=None if body is None else json.dumps(body).encode(),
                headers={"Authorization": f"Bearer {holder['token']}", "Content-Type": "application/json", "cozy-project-storage": "3"})
            try:
                with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=30) as answer:
                    return answer.status, answer.read().decode()
            except urllib.error.HTTPError as refusal:
                return refusal.code, refusal.read().decode()
        listed = json.loads(calcifer("GET", "/v1/projects?limit=1000")[1])["projects"]
        assert any(row["id"] == created["id"] and Path(row["path"]).resolve() == project for row in listed), "new's project is not in the index"
        status, said = calcifer("POST", "/v1/projects", {"name": "by-name-only", "unencrypted": True})
        assert status == 400 and json.loads(said)["error"]["code"] == "body_invalid", f"a create by name alone: {status} {said}"
        assert not (Path.home() / "by-name-only").exists(), "a create by name alone made a folder"
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
        url = urlsplit(opened["url"])
        assert url.hostname in {"127.0.0.1", "localhost", "::1"}, "open did not print a local URL"
        assert url.path == f"/projects/{created['id']}", f"open printed another project's URL: {url.path}"
        # A --json caller is a program: it gets the page's plain address, said not to be private, and no browser starts (cl-985).
        assert set(opened) == {"url", "private"} and opened["private"] is False, f"--json open: {opened}"
        assert not url.fragment and not url.query, f"the --json address carries something besides the page: {opened['url']}"
        plain_open = run("default project URL", ["open"], project)
        assert not plain_open.stdout.lstrip().startswith("{") and "url" in plain_open.stdout
        private = "private: true" in plain_open.stdout
        if private:
            # Cozy's private browser opened it: the URL is the page's, and nothing went with it.
            assert "code=" not in plain_open.stdout, "a private open printed a credential"
        else:
            # No display here, no Chrome, or its launch failed: the page's address, which any browser of this account opens, said not to be private.
            assert "isn't private" in plain_open.stderr and project.name in plain_open.stderr, "the fallback didn't say it isn't private"
        assert "expires_ms" not in plain_open.stdout
        assert not browser_log.exists(), "open launched the default browser"
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
