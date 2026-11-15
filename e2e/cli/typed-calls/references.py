# /// script
# requires-python = ">=3.12"
# ///
"""Qualify generated references with a real fixture wheel and public SDK commands."""

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import tomllib
from collections.abc import Sequence
from pathlib import Path
from typing import TypedDict, cast


class SelectedWheel(TypedDict):
    path: str
    sha256: str


class Prepared(TypedDict):
    fixture: str
    sdk: dict[str, SelectedWheel]


class SdkError(TypedDict):
    code: int
    name: str
    message: str


class SdkFailure(TypedDict):
    error: SdkError


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--prepared", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    prepared = cast(Prepared, json.loads(args.prepared.read_text()))
    fixture = Path(prepared["fixture"]).resolve()
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=False)
    sdk = [
        str((fixture / "caller" / prepared["sdk"][name]["path"]).resolve())
        for name in ("cozy-runtime", "tensorfs")
    ]
    commands: list[dict[str, object]] = []

    def run(label: str, command: Sequence[str], *, stale: bool = False) -> Path:
        stdout, stderr = out / (label + ".stdout"), out / (label + ".stderr")
        with stdout.open("w") as output, stderr.open("w") as errors:
            result = subprocess.run(command, stdout=output, stderr=errors, check=False)
        commands.append(
            {"case": label, "command": list(command), "exit_code": result.returncode}
        )
        (out / "commands.json").write_text(json.dumps(commands, indent=2) + "\n")
        if stale:
            assert result.returncode != 0, (label, "changed declaration was accepted")
            failure = cast(
                SdkFailure, json.loads(stderr.read_text() or stdout.read_text())
            )
            assert (
                failure["error"]["code"] == 3
                and failure["error"]["name"] == "validation"
            ), failure
            assert "stale_call_reference" in failure["error"]["message"], failure
        else:
            assert result.returncode == 0, (
                label,
                stderr.read_text(),
                stdout.read_text(),
            )
        return stdout

    def reader(python: str, *words: str | Path) -> list[str]:
        return [
            "uv",
            "run",
            "--no-project",
            "--python",
            python,
            "--with",
            sdk[0],
            "--with",
            sdk[1],
            "cozy-runtime",
            *map(str, words),
        ]

    def build_leaf(at: Path, label: str) -> Path:
        run(
            label,
            [
                "uv",
                "build",
                "--wheel",
                "--project",
                str(at / "leaf"),
                "--out-dir",
                str(at / "wheels"),
            ],
        )
        version = tomllib.loads((at / "leaf/pyproject.toml").read_text())["project"][
            "version"
        ]
        assert isinstance(version, str)
        wheel = at / "wheels" / f"xs198_typed_leaf-{version}-py3-none-any.whl"
        assert wheel.is_file()
        for package in ("caller", "child"):
            project = at / package / "pyproject.toml"
            text, changed = re.subn(
                r"(?m)^xs198-typed-leaf = .*$",
                "xs198-typed-leaf = { path = "
                + json.dumps("../wheels/" + wheel.name)
                + " }",
                project.read_text(),
            )
            assert changed == 1, project
            project.write_text(text)
        return wheel

    wheel = build_leaf(fixture, "build-leaf")
    run("lock", ["uv", "lock", "--project", str(fixture / "caller")])
    packages = tomllib.loads((fixture / "caller/uv.lock").read_text())["package"]
    expected = {
        **prepared["sdk"],
        "xs198-typed-leaf": SelectedWheel(
            path="../wheels/" + wheel.name,
            sha256=hashlib.sha256(wheel.read_bytes()).hexdigest(),
        ),
    }
    for name, selected in expected.items():
        matches = [package for package in packages if package["name"] == name]
        assert len(matches) == 1, (name, matches)
        assert matches[0]["source"] == {"path": selected["path"]}, matches
        assert [item["hash"] for item in matches[0]["wheels"]] == [
            "sha256:" + selected["sha256"]
        ]

    targets = {
        "leaf": fixture / "child/typed_child/leaf_calls.py",
        "child": fixture / "caller/typed_caller/child_calls.py",
    }
    for distribution, module in targets.items():
        run(
            "generate-312-" + distribution,
            reader(
                "3.12",
                "--dir",
                fixture / "caller",
                "calls",
                "xs198-typed-" + distribution,
                module,
            ),
        )
    for distribution, module in targets.items():
        other = out / (distribution + "-313.py")
        run(
            "generate-313-" + distribution,
            reader(
                "3.13",
                "--dir",
                fixture / "caller",
                "calls",
                "xs198-typed-" + distribution,
                other,
            ),
        )
        assert module.read_bytes() == other.read_bytes(), (
            distribution,
            "Python ABI changed references",
        )

    document = run(
        "leaf-source-interface",
        reader("3.12", "--json", "--dir", fixture / "leaf", "describe"),
    )
    from_interface = out / "leaf-interface.py"
    run(
        "generate-interface",
        reader(
            "3.12",
            "--dir",
            fixture / "caller",
            "--package-interface",
            document,
            "calls",
            "xs198-typed-leaf",
            from_interface,
        ),
    )
    assert targets["leaf"].read_bytes() == from_interface.read_bytes(), (
        "wheel/source declaration changed references"
    )
    run("describe", reader("3.12", "--json", "--dir", fixture / "caller", "describe"))

    declaration = "number: Annotated[int, msgspec.Meta(ge=0, le=1000)] = 17"
    for label, version, changed in (
        ("default", "0.1.1", declaration.replace("= 17", "= 19")),
        ("bound", "0.1.2", declaration.replace("ge=0", "ge=1")),
    ):
        at = out / ("stale-" + label)
        shutil.copytree(
            fixture, at, ignore=shutil.ignore_patterns("__pycache__", ".venv")
        )
        source = at / "leaf/typed_leaf/__init__.py"
        text = source.read_text()
        assert text.count(declaration) == 1
        source.write_text(text.replace(declaration, changed))
        project = at / "leaf/pyproject.toml"
        text = project.read_text()
        assert text.count('version = "0.1.0"') == 1
        project.write_text(text.replace('version = "0.1.0"', f'version = "{version}"'))
        build_leaf(at, "build-stale-" + label)
        run("lock-stale-" + label, ["uv", "lock", "--project", str(at / "caller")])
        run(
            "refuse-stale-" + label,
            reader("3.12", "--json", "--dir", at / "caller", "describe"),
            stale=True,
        )

    (out / "qualified.json").write_text(
        json.dumps(
            {
                "leaf_wheel": expected["xs198-typed-leaf"],
                "python_312_313_equal": True,
                "wheel_source_interface_equal": True,
                "stale_default_refused": True,
                "stale_bound_refused": True,
                "scope": (
                    "real builds, locks and public SDK source commands; "
                    "execution is run.py's separate proof"
                ),
            },
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
