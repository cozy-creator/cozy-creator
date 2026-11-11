"""Check readable parser and runtime errors without starting services or running models."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cozy", required=True)
    args = parser.parse_args()
    binary = str(Path(args.cozy).resolve())
    cases = [
        (["ls"], "cozy --help"),
        (["machine", "unknown-command"], "cozy machine --help"),
        (["--unknown-option"], "cozy --help"),
        (["machine", "ls", "--unknown-option"], "cozy machine ls --help"),
        (["run", "watch", "1", "--unknown-option"], "cozy run watch --help"),
        (["--full", "machine", "rent", "--unknown-option"], "cozy machine rent --help"),
        (["machine", "show"], "cozy machine show --help"),
    ]
    for words, help_command in cases:
        plain = subprocess.run([binary, *words], capture_output=True, text=True)
        assert plain.returncode == 2, (words, plain.returncode)
        assert not plain.stdout, (words, "human diagnostic was written to stdout", plain.stdout)
        assert plain.stderr.startswith("error"), (words, plain.stderr)
        assert "\\n" not in plain.stderr and "Commands:" not in plain.stderr, (words, plain.stderr)
        assert f"next: {help_command}" in plain.stderr, (words, plain.stderr)
        if words == ["machine", "show"]:
            assert "<NAME>" in plain.stderr, plain.stderr
        structured = subprocess.run([binary, "--json", *words], capture_output=True, text=True)
        assert structured.returncode == 2 and not structured.stderr, (words, structured.stderr)
        error = json.loads(structured.stdout)["error"]
        assert error["kind"] == "usage" and error["code"] == "usage"
        assert error["remedy"] == help_command, (words, error)

    if os.name == "posix":
        with tempfile.TemporaryDirectory(prefix="cozy-error-config-") as directory:
            home = Path(directory)
            (home / ".cozy").mkdir()
            (home / ".cozy/config.toml").write_text("this = [\n")
            env = {**os.environ, "HOME": str(home)}
            plain = subprocess.run([binary, "status"], env=env, capture_output=True, text=True)
            assert plain.returncode == 2 and not plain.stdout, plain.stdout + plain.stderr
            assert "config_invalid" in plain.stderr and "hint:" in plain.stderr, plain.stderr
            structured = subprocess.run([binary, "--json", "status"], env=env, capture_output=True, text=True)
            error = json.loads(structured.stdout)["error"]
            assert structured.returncode == 2 and not structured.stderr
            assert error["code"] == "config_invalid" and "config.toml" in error["remedy"]
    print("CLI errors: parser guidance, stderr output and structured runtime errors passed")


if __name__ == "__main__":
    main()
