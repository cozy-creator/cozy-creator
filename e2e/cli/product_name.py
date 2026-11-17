#!/usr/bin/env python3
"""product_name: what a user reads names the product "Cozy Creator", never a bare "Cozy" (Paul, 2026-11-17). It reads the product's
code, page and docs as hints.py does: Rust string literals (but a tracing or log macro's line), TS/TSX strings and JSX text, and
Markdown prose (code spans and fences aside). The command `cozy` is lowercase and never matches; neither do identifiers and headers
(`CozyCap`, `Cozy-Edit`, `COZY_HOME`). Prints each `path:line` that names the product bare and exits non-zero."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess
import sys

SCOPES = ("crates/", "web/app/src/", "docs/", "changelog.d/", "README.md")
SKIP = re.compile(r"(^|/)(tests?|e2e|fixtures?)(/|$)|_tests?\.rs$|\.test\.tsx?$|\.spec\.tsx?$")
# "Cozy" as a word of its own, not "Cozy Creator".
BARE = re.compile(r"(?<![\w-])Cozy(?![\w-]|\s+Creator\b)")

RUST = re.compile(r"""//[^\n]*|/\*.*?\*/|(?P<s>(?<!\w)b?r(?P<h>\#*)".*?"(?P=h)|b?"(?:\\.|[^"\\])*")|'(?:\\.[^'\n]*|[^'\\\n])'""", re.S)
TS = re.compile(r"""//[^\n]*|/\*.*?\*/|(?P<s>"(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])*'|`(?:\\.|[^`\\])*`)""", re.S)
LOG = re.compile(r"\b(trace|debug|info|warn|error)!\s*\(")
# A Rust file's test module: from `#[cfg(test)] mod … {` to its closing brace in the first column.
RUST_TESTS = re.compile(r"^#\[cfg\(test\)\]\s*\nmod .*?^\}", re.S | re.M)
MARKDOWN_CODE = re.compile(r"^(```|~~~).*?^\1|`[^`\n]*`", re.S | re.M)


def sources(root: Path) -> list[Path]:
    listed = subprocess.run(["git", "ls-files", "--", *SCOPES], cwd=root, capture_output=True, text=True, check=True).stdout.split()
    return [root / name for name in listed if not SKIP.search(name)]


def blank(match: re.Match[str]) -> str:
    """The match's text as spaces, its line breaks kept: what follows keeps its place."""
    return re.sub(r"[^\n]", " ", match.group())


def offsets(text: str) -> list[int]:
    """Each place the product is named bare in `text`, by the scan its file's kind takes."""
    return [m.start() for m in BARE.finditer(text)]


def bare_rust(text: str) -> list[int]:
    text = RUST_TESTS.sub(blank, text)
    found = []
    for token in RUST.finditer(text):
        if token.group("s") is None:
            continue
        line = text[text.rfind("\n", 0, token.start()) + 1 : text.find("\n", token.start())]
        if not LOG.search(line):
            found += [token.start() + at for at in offsets(token.group())]
    return found


def bare_ts(text: str, jsx: bool) -> list[int]:
    found, end = [], 0
    for token in TS.finditer(text):
        if jsx:  # JSX text sits between the tokens
            found += [end + at for at in offsets(text[end : token.start()])]
        if token.group("s") is not None:
            found += [token.start() + at for at in offsets(token.group())]
        end = token.end()
    if jsx:
        found += [end + at for at in offsets(text[end:])]
    return found


def bare(path: Path, text: str) -> list[int]:
    if path.suffix == ".rs":
        return bare_rust(text)
    if path.suffix in (".ts", ".tsx"):
        return bare_ts(text, jsx=path.suffix == ".tsx")
    if path.suffix == ".md":
        return offsets(MARKDOWN_CODE.sub(blank, text))
    return []


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, type=Path)
    root = parser.parse_args().root.resolve()
    found = []
    for path in sources(root):
        try:
            text = path.read_text(errors="replace")
        except OSError:
            continue
        lines = sorted({text.count("\n", 0, at) + 1 for at in bare(path, text)})
        found += [f"{path.relative_to(root)}:{line}: {text.splitlines()[line - 1].strip()[:160]}" for line in lines]
    print(f"product name: {len(found)} line(s) name the product a bare \"Cozy\"; user-facing text says \"Cozy Creator\"", flush=True)
    for line in found:
        print(f"  {line}", flush=True)
    sys.exit(1 if found else 0)


if __name__ == "__main__":
    main()
