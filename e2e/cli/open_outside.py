"""cl-1179: `cozy open` outside a project folder opens Cozy Creator at its start, `/`, says so in one line, and exits 0.

Paul, 11-17, after `cozy open` in his home folder answered `error[not_in_a_project]`: "I think in this case, we should tell the user
'you are not in a project folder, please navigate to one or create one using cozy new' but we should also open the webui anyway,
because they can select between different projects in it anyway, so opening the cozy webui generally is fine."

Outside any project: a private window of $BROWSER's Chrome starts at Calcifer's root, which serves the page; stdout has the root URL and
stderr the one notice. `--json` gives the root URL and a `note` (`not_in_a_project`, naming `cozy new`) and starts no browser. With no
browser that has a private window (headless, SSH), it prints the root URL. Inside a project, `/p/<code>` is entrypoints.py's.

open_outside.py --cozy BIN --out DIR
"""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import urllib.request

NOTICE = ("You're not in a project folder, so this opens Cozy Creator, where you can pick one. "
          "To open a project directly, run cozy open in its folder, or create one with cozy new.")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cozy", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    binary = str(Path(args.cozy).resolve())
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    outside = Path(tempfile.mkdtemp(prefix="cozy-open-outside-"))
    browser_log = out / "browser-opened.txt"
    browser_log.unlink(missing_ok=True)
    environment = os.environ.copy()

    # The user's browser, as they set it ($BROWSER): a Chrome that writes down what it was asked to open (cl-1067).
    def recording(name: str) -> str:
        program = out / (name + (".cmd" if os.name == "nt" else ""))
        if os.name == "nt":
            program.write_text(f'@echo %*>> "{browser_log}"\r\n')
        else:
            program.write_text(f"#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{browser_log}'\n")
            program.chmod(0o755)
        return str(program)
    environment["BROWSER"] = recording("google-chrome")

    def cozy(name: str, *words: str) -> subprocess.CompletedProcess[str]:
        if log := environment.get("ARGV_LOG"):
            with open(log, "a", encoding="utf-8") as stream:
                stream.write("\x1f".join(words) + "\x1f\x1e")
        said = subprocess.run([binary, *words], cwd=outside, env=environment, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                              timeout=300)
        (out / f"{name}.out").write_text(said.stdout)
        (out / f"{name}.err").write_text(said.stderr)
        assert said.returncode == 0, f"{name}: exit {said.returncode}, not 0: {said.stdout[-300:]}{said.stderr[-300:]}"
        return said

    def opened() -> list[str]:
        return browser_log.read_text().split() if browser_log.exists() else []

    plain = cozy("open", "open")
    holder = json.loads((Path.home() / ".cozy" / "calcifer" / "calcifer.lock.holder").read_text())
    root = f"http://{holder['address']}/"
    assert root in plain.stdout, f"open: not the root URL {root}: {plain.stdout}"
    assert "/p/" not in plain.stdout, f"open printed a project's address: {plain.stdout}"
    said = [line for line in plain.stderr.splitlines() if line.strip()]
    assert said == [NOTICE], f"open: stderr is not the one notice: {plain.stderr}"
    assert opened() == ["--incognito", root], f"open didn't start a private window at {root}: {opened() or 'nothing started'}"
    # The page's start is the page: what the browser was sent to answers it.
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(root, timeout=30) as page:
        assert page.status == 200 and page.headers.get_content_type() == "text/html", f"{root}: {page.status} {page.headers}"

    document = json.loads(cozy("open-json", "--json", "open").stdout)
    assert document == {"url": root, "note": {"code": "not_in_a_project", "message": NOTICE, "next": ["cozy new"]}}, f"--json open: {document}"
    assert opened() == ["--incognito", root], f"--json open started a browser: {opened()}"

    # Headless or over SSH: no browser with a private window, so it prints the root URL, the notice and why nothing opened. Linux only: a
    # macOS or Windows runner has real browsers installed, which would open.
    if sys.platform.startswith("linux"):
        for key in ("BROWSER", "DISPLAY", "WAYLAND_DISPLAY"):
            environment.pop(key, None)
        headless = cozy("open-headless", "open")
        assert root in headless.stdout and "/p/" not in headless.stdout, f"headless open: {headless.stdout}"
        lines = [line for line in headless.stderr.splitlines() if line.strip()]
        assert len(lines) == 2 and lines[0] == NOTICE and "no browser with a private window" in lines[1], f"headless open: {headless.stderr}"
        assert opened() == ["--incognito", root], f"headless open started a browser: {opened()}"
    assert not (outside / "project.db").exists(), "open outside a project made one"
    print("open_outside: cozy open outside a project opens Cozy Creator's root, says so, exits 0")


if __name__ == "__main__":
    main()
