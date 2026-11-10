"""Exercise numeric gate provenance before artifact claims are trusted. No network calls."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

type Json = None | bool | int | str | list[Json] | dict[str, Json]

VERIFY = Path(__file__).resolve().parents[1] / "verify.sh"
TRUSTED: dict[str, Json] = {
    "repository": {"full_name": "cozy-creator/cozy-creator"},
    "head_repository": {"full_name": "cozy-creator/cozy-creator"},
    "event": "workflow_dispatch",
    "path": ".github/workflows/release-gate.yml",
    "status": "completed",
    "conclusion": "success",
}
FAKE_GH = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
with open(os.environ["AUDIT_CALLS"], "a") as f:
    f.write(json.dumps(args) + "\n")
if args[0] == "api":
    print(os.environ["AUDIT_METADATA"])
elif args[:2] == ["run", "download"]:
    dest = pathlib.Path(args[args.index("-D") + 1]) / "gate-one"
    dest.mkdir(parents=True)
    (dest / "candidate.json").write_text('{"cozy_sha256":"' + 'a'*64 + '"}')
    (dest / "verdict.txt").write_text(os.environ["AUDIT_VERDICT"] + "\n")
else:
    sys.exit("unexpected gh operation")
'''

class VerifyTest(unittest.TestCase):
    def run_gate(
        self, metadata: dict[str, Json], verdict: str = "VERDICT PASS", local: bool = False
    ) -> tuple[subprocess.CompletedProcess[str], list[list[str]]]:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            gh = root / "gh"
            gh.write_text(FAKE_GH)
            gh.chmod(0o755)
            log = root / "calls.jsonl"
            env = {**os.environ, "PATH": str(root) + os.pathsep + os.environ["PATH"],
                   "AUDIT_METADATA": json.dumps(metadata), "AUDIT_VERDICT": verdict,
                   "AUDIT_CALLS": str(log)}
            gate = "123456789"
            if local:
                directory = root / "hand-gate"
                directory.mkdir()
                (directory / "candidate.json").write_text('{"cozy_sha256":"' + 'a'*64 + '"}')
                (directory / "verdict.txt").write_text(verdict + "\n")
                gate = str(directory)
            result = subprocess.run(["bash", str(VERIFY), gate], env=env, text=True,
                                    capture_output=True, check=False)
            calls: list[list[str]] = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
            return result, calls

    def test_trusted_dispatch_accepts_matching_pass(self) -> None:
        result, calls = self.run_gate(TRUSTED)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["cozy_sha256"], "a" * 64)
        self.assertEqual(calls[0][0], "api")
        self.assertEqual(calls[1][:2], ["run", "download"])

    def test_untrusted_or_incomplete_run_cannot_supply_artifacts(self) -> None:
        cases: list[dict[str, Json]] = [
            {"event": "pull_request"}, {"event": "pull_request_target"},
            {"path": ".github/workflows/forged.yml"},
            {"head_repository": {"full_name": "attacker/cozy-creator"}},
            {"repository": {"full_name": "attacker/cozy-creator"}},
            {"status": "in_progress"}, {"conclusion": "cancelled"},
            {"conclusion": None},
        ]
        for change in cases:
            with self.subTest(change=change):
                result, calls = self.run_gate({**TRUSTED, **change})
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(len(calls), 1, "untrusted artifacts must not be downloaded")
        result, calls = self.run_gate({})
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(calls), 1)

    def test_lead_accepted_failure_preserves_explicit_policy(self) -> None:
        metadata: dict[str, Json] = {**TRUSTED, "conclusion": "failure"}
        result, _ = self.run_gate(metadata, "VERDICT FAIL (accepted by the lead)")
        self.assertEqual(result.returncode, 0, result.stderr)
        result, _ = self.run_gate(metadata, "VERDICT FAIL (numerical mismatch)")
        self.assertNotEqual(result.returncode, 0)

    def test_hand_gate_remains_an_explicit_local_receipt(self) -> None:
        result, calls = self.run_gate({}, local=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, [])

if __name__ == "__main__":
    unittest.main()
