#!/usr/bin/env python3
"""hint_commands: every command a hint names is one this cozy has. A hint is a `cozy …` the product tells someone to type: a backticked
`cozy …` anywhere in its code, page and docs (a doc comment, a changelog line), and a quoted one a `next` or remedy line gives, or that
opens with one of its commands. Each is walked down this cozy's own help (`cozy <verb…> --help`): its first word must be a command,
and each word after names a subcommand while the command it reached has subcommands. Where that command takes no subcommand the rest
are its arguments; where it takes both (`cozy run <target>`, `cozy run <ls|watch|…>`), a word it retired (RETIRED) is no target."""

from __future__ import annotations

import argparse
from collections import defaultdict
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

# Where hints live: the product's code, its page and its docs. Its tests and cells name removed commands on purpose.
SCOPES = ("crates/", "web/app/src/", "docs/", "changelog.d/", "README.md")
SKIP = re.compile(r"(^|/)(tests?|e2e|fixtures?)(/|$)|_tests?\.rs$|\.test\.tsx?$|\.spec\.tsx?$")
# `cozy` opening a backtick span, a quote or a `next:` line, and what follows it up to that span's or quote's end.
HINT = re.compile(r"""(`|"|'|next: )cozy ((?:(?!\1)[^`"'\n])*)""")
WORD = re.compile(r"[a-z][a-z0-9-]*")
# Words a command that also takes a positional once had as subcommands: such a word after it is a stale hint, not a target.
RETIRED: dict[tuple[str, ...], set[str]] = {("run",): {"show", "play"}}


def sources(root: Path) -> list[Path]:
    listed = subprocess.run(["git", "ls-files", "--", *SCOPES], cwd=root, capture_output=True, text=True, check=True).stdout.split()
    return [root / name for name in listed if not SKIP.search(name)]


class Cozy:
    def __init__(self, binary: Path) -> None:
        self.binary = binary
        # No home of anyone's: a help read starts nothing, and this proves it reads nothing either.
        self.home = tempfile.mkdtemp(prefix="hints-home-")
        self.seen: dict[tuple[str, ...], tuple[int, str]] = {}

    def help(self, words: tuple[str, ...]) -> tuple[int, str]:
        if words not in self.seen:
            env = {"HOME": self.home, "PATH": os.environ.get("PATH", "/usr/bin:/bin"), "NO_COLOR": "1"}
            done = subprocess.run([str(self.binary), *words, "--help"], capture_output=True, text=True, env=env, timeout=60, check=False)
            self.seen[words] = (done.returncode, done.stdout + done.stderr)
        return self.seen[words]

    def subcommands(self, words: tuple[str, ...]) -> set[str]:
        text = self.help(words)[1]
        block = text.split("\nCommands:\n", 1)[1].split("\n\n", 1)[0] if "\nCommands:\n" in text else ""
        return {line.split()[0] for line in block.splitlines() if line.startswith("  ") and line.split()}

    def takes_target(self, words: tuple[str, ...]) -> bool:
        """Whether the command's usage names a positional besides its subcommand."""
        usages = [line for line in self.help(words)[1].splitlines() if line.startswith(("Usage: ", "       cozy "))]
        return any(re.search(r"<(?!COMMAND>)[A-Z]", line) for line in usages)


def hints(root: Path, commands: set[str]) -> dict[tuple[str, ...], list[str]]:
    found: dict[tuple[str, ...], list[str]] = defaultdict(list)
    for path in sources(root):
        try:
            text = path.read_text(errors="replace")
        except OSError:
            continue
        for number, line in enumerate(text.splitlines(), 1):
            for match in HINT.finditer(line):
                words: list[str] = []
                for token in match.group(2).split():
                    if not WORD.fullmatch(token):
                        break
                    words.append(token)
                if not words:
                    continue
                # A quoted sentence that happens to open with "cozy" (`"cozy may not write there"`) is no hint; one a next or remedy
                # line gives, or that opens with a command, is.
                given = re.search(r"(next|remedy)\W*$", line[: match.start()][-40:], re.IGNORECASE)
                if match.group(1) in "\"'" and not given and words[0] not in commands:
                    continue
                found[tuple(words)].append(f"{path.relative_to(root)}:{number}")
    return found


def verdict(cozy: Cozy, words: tuple[str, ...]) -> str | None:
    """None when `cozy <words>` names a command this cozy has (what follows it its arguments), else why not."""
    code, text = cozy.help(words[:1])
    if code != 0:
        return f"`cozy {words[0]}` is no command ({next((line for line in text.splitlines() if line.strip()), '')[:120]})"
    path = words[:1]
    for word in words[1:]:
        subcommands = cozy.subcommands(path)
        if word in subcommands or (subcommands and hidden(cozy, path, word)):
            path += (word,)
            continue
        if not subcommands or (cozy.takes_target(path) and word not in RETIRED.get(path, set())):
            return None
        return f"`cozy {' '.join(path)}` has no subcommand `{word}`"
    return None


def hidden(cozy: Cozy, path: tuple[str, ...], word: str) -> bool:
    """Whether `word` is a subcommand of `cozy <path>` its help doesn't list (a dev-only or internal one): its own help parses, as that
    subcommand where the command also takes a positional, which would parse it as that positional."""
    code, text = cozy.help(path + (word,))
    if code != 0:
        return False
    if not cozy.takes_target(path):
        return True
    named = next((line for line in text.splitlines() if line.startswith("Usage: ")), "").removeprefix("Usage: ").split()
    return named[: len(path) + 2] == ["cozy", *path, word]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cozy", required=True, type=Path)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    cozy = Cozy(args.cozy.resolve())
    commands = cozy.subcommands(())
    assert {"run", "machine", "model"} <= commands, f"this cozy's help lists no commands: {sorted(commands)}"
    found = hints(args.root.resolve(), commands)
    bad = {" ".join(words): {"why": why, "at": places} for words, places in sorted(found.items()) if (why := verdict(cozy, words))}
    (args.out / "hints.json").write_text(json.dumps({"hints": len(found), "stale": bad}, indent=2))
    print(f"hint commands: {len(found)} named, {len(bad)} stale", flush=True)
    for words, entry in bad.items():
        print(f"  cozy {words}: {entry['why']}; at {', '.join(entry['at'][:6])}", flush=True)
    assert len(found) > 20, f"only {len(found)} hints found: the scan reads nothing it should"
    assert not bad, f"{len(bad)} hint(s) name a command this cozy doesn't have: {', '.join(bad)}"


if __name__ == "__main__":
    main()
