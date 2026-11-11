"""No production network calls: fixture origins and proxy metadata only."""
import http.server
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest

from test_hub import IsolationError, metadata, require_ci_test_hub, require_test_hub


class IsolationTests(unittest.TestCase):
    def test_manual_production_suite_keeps_provider_checks_without_synthetic_writes(self) -> None:
        root = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory(prefix="hub-suite-flow-") as directory:
            scratch = Path(directory)
            library = scratch / "suites/hub/lib.sh"
            library.parent.mkdir(parents=True)
            names = ["hub_contract", "hub_account", "hub_do", "hub_dev", "hub_pod", "hub_cleanup", "hub_publish", "hub_packages", "hub_models"]
            stubs = [f'{name}() {{ printf "%s\\n" "{name}" >> "$CALL_LOG"; }}' for name in names]
            library.write_text("WHO=fixture\n" + "\n".join(stubs) + "\n")
            log = scratch / "calls"
            script = 'HERE="$1"; OUT="$1/out"; ON=local; HUB_PROVIDERS=fixture; end_it() { :; }; . "$2"'
            environment = {key: value for key, value in os.environ.items() if key not in {"CI", "GITHUB_ACTIONS"}}
            result = subprocess.run(["bash", "-c", script, "suite-test", str(scratch), str(root / "release-gate/suites/hub/cells.sh")],
                env={**environment, "CALL_LOG": str(log)}, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            called = log.read_text().splitlines()
            self.assertIn("hub_pod", called)
            self.assertIn("hub_contract", called)
            self.assertFalse({"hub_publish", "hub_packages", "hub_models"} & set(called))

    def test_ci_guard_also_blocks_production_reads(self) -> None:
        def no_network(_: str) -> dict[str, object]:
            self.fail("CI contacted production")
        require_ci_test_hub("https://tensorhub.com", environment={}, fetch=no_network)
        for environment in [{"CI": "true"}, {"GITHUB_ACTIONS": "true"}]:
            with self.subTest(environment=environment), self.assertRaises(IsolationError):
                require_ci_test_hub("https://tensorhub.com", environment=environment, fetch=no_network)

    def test_ci_entrypoints_stop_before_any_cli_data_request(self) -> None:
        root = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory(prefix="ci-hub-read-guard-") as directory:
            scratch = Path(directory)
            bin_dir = scratch / "bin"
            bin_dir.mkdir()
            cli = bin_dir / "cozy"
            cli.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$TEST_CLI_LOG"\n')
            cli.chmod(0o755)
            log = scratch / "calls"
            environment = {**os.environ, "CI": "true", "GITHUB_ACTIONS": "true", "COZY_HUB_URL": "https://tensorhub.com", "COZY_TEST_HUB_ORIGIN": "https://127.0.0.1:8443", "TEST_CLI_LOG": str(log), "PATH": str(bin_dir) + os.pathsep + os.environ.get("PATH", ""), "OUT": str(scratch / "out")}
            commands = [
                ["bash", str(root / "release-gate/gate.sh"), "--copy", str(root / "release-gate"), "0", "1", "cpu", "--suite", "hub"],
                ["bash", str(root / "release-gate/hubgate.sh"), "--suite", "hub", "--hub", "https://tensorhub.com"],
                ["bash", "-c", '. "$1/release-gate/sessions/lib.sh"', "session-test", str(root)],
                ["bash", str(root / "e2e/capabilities/run.sh")],
            ]
            for command in commands:
                with self.subTest(command=command):
                    result = subprocess.run(command, env=environment, capture_output=True, text=True)
                    self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                    self.assertIn("test Hub required", result.stderr)
                    self.assertFalse(log.exists(), "a CI data request reached the CLI")

    def test_publication_helper_refuses_production_before_running_cli(self) -> None:
        root = Path(__file__).resolve().parents[1]
        library = os.environ.get("TEST_HUB_LIBRARY", str(root / "release-gate/suites/hub/lib.sh"))
        with tempfile.TemporaryDirectory(prefix="hub-publication-guard-") as directory:
            scratch = Path(directory)
            cli = scratch / "cozy"
            cli.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$TEST_CLI_LOG"\n')
            cli.chmod(0o755)
            script = '''set -uo pipefail
HERE="$1/release-gate"; HB="$2/receipt"; CLI="$3"; HUB_URL=https://tensorhub.com
export COZY_TEST_HUB_ORIGIN=https://127.0.0.1:8443
expect() { :; }
log() { :; }
. "$4" || exit $?
WHO=fidika; HUB_PKG=ci-hub-fixture
hub_publish
'''
            log = scratch / "commands"
            result = subprocess.run(["bash", "-c", script, "guard-test", str(root), str(scratch), str(cli), library],
                env={**os.environ, "TEST_CLI_LOG": str(log)}, capture_output=True, text=True)
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertFalse(log.exists(), "a production publication reached the CLI")

    def test_direct_production_and_default_targets_never_reach_network(self) -> None:
        def no_network(_: str) -> dict[str, object]:
            self.fail("the guard contacted a non-test origin")
        for hub in ["", "https://tensorhub.com", "https://api.tensorhub.com", "https://example.test", "http://127.0.0.1.evil.test"]:
            with self.subTest(hub=hub), self.assertRaises(IsolationError):
                require_test_hub(hub, "https://localhost:8443", no_network)

    def test_loopback_proxy_to_production_is_refused_despite_marker(self) -> None:
        for resource, issuer in [
            ("https://tensorhub.com", "https://tensorhub.com/v1/auth"),
            ("https://localhost:8443", "https://tensorhub.com/v1/auth"),
        ]:
            with self.subTest(resource=resource), self.assertRaises(IsolationError):
                require_test_hub("http://127.0.0.1:9999", "https://localhost:8443", lambda _: {
                    "resource": resource, "authorization_servers": [issuer],
                })

    def test_explicit_fixture_origin_and_issuer_are_required(self) -> None:
        document: dict[str, object] = {"resource": "https://localhost:8443", "authorization_servers": ["https://localhost:8443/v1/auth"]}
        require_test_hub("http://127.0.0.1:8819", "https://localhost:8443", lambda _: document)
        with self.assertRaises(IsolationError):
            require_test_hub("http://127.0.0.1:8819", "", lambda _: document)
        with self.assertRaises(IsolationError):
            require_test_hub("http://user:secret@127.0.0.1:8819", "https://localhost:8443", lambda _: document)

    def test_redirects_are_not_followed(self) -> None:
        class Redirect(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                self.send_response(302)
                self.send_header("Location", "https://tensorhub.com/.well-known/oauth-protected-resource")
                self.end_headers()
            def log_message(self, format: str, *args: object) -> None:
                pass
        server = http.server.HTTPServer(("127.0.0.1", 0), Redirect)
        thread = threading.Thread(target=server.handle_request)
        thread.start()
        try:
            with self.assertRaises(IsolationError):
                metadata(f"http://127.0.0.1:{server.server_port}")
        finally:
            thread.join(timeout=5)
            server.server_close()


if __name__ == "__main__":
    unittest.main()
