"""Opt-in hosted Vite process and independent source-only HMR probe; never laptop config."""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import tomllib
import urllib.error
import urllib.request

from common import Birth, Failure, birth, alive, require


class Development:
    def __init__(self) -> None:
        require(os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted", "development_hosted_only")
        self.repo = Path(__file__).resolve().parents[3]
        self.config = Path.home() / ".cozy/config.toml"
        self.module = self.repo / "web/app/src/__cozy_hmr_proof.ts"
        require(not self.config.is_symlink() and not self.module.exists(), "development_owned_paths")
        self.original = self.config.read_bytes() if self.config.exists() else None
        self.written: bytes | None = None
        self.identity: tuple[int, int] | None = None
        self.process: subprocess.Popen[bytes] | None = None
        self.birth: Birth | None = None

    def start(self) -> None:
        """Vite and the probe, with `[calcifer] development_web` alone: Calcifer ignores it until `enable`."""
        require(shutil.which("pnpm"), "development_pnpm_missing")
        self.write(dev=False)
        with self.module.open("x") as target:
            target.write(self.code(1))
        metadata = self.module.lstat()
        self.identity = (metadata.st_dev, metadata.st_ino)
        self.process = subprocess.Popen(["pnpm", "--dir", str(self.repo / "web/app"), "dev", "--host", "127.0.0.1", "--port", "5173"], cwd=self.repo, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        self.birth = birth(self.process.pid)
        deadline = time.monotonic() + 90
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        while time.monotonic() < deadline:
            require(alive(self.birth), "development_server_exited")
            try:
                with opener.open("http://127.0.0.1:5173/@vite/client", timeout=2) as response:
                    if response.status == 200:
                        return
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(0.1)
        raise Failure("development_server_readiness")

    def enable(self) -> None:
        """Dev mode: a top-level `dev = true` as well, for the next Calcifer to start."""
        self.write(dev=True)

    def write(self, dev: bool) -> None:
        require(self.written is None or not self.config.is_symlink() and self.config.read_bytes() == self.written, "development_config_changed")
        text = (self.original or b"").decode()
        parsed = tomllib.loads(text)
        # `dev = true` must be top-level, so it goes first, before any table; any other top-level `dev` goes.
        lines, result, section, inserted = text.splitlines(keepends=True), ["dev = true\n"] if dev else [], "", False
        for line in lines:
            clean = line.split("#", 1)[0].strip()
            if clean.startswith("["):
                if section == "[calcifer]" and not inserted:
                    result.append('development_web = "127.0.0.1:5173"\n'); inserted = True
                section = clean
            if section == "" and clean.partition("=")[0].strip() == "dev":
                continue
            if section == "[calcifer]" and clean.partition("=")[0].strip() == "development_web":
                continue
            result.append(line)
        if section == "[calcifer]" and not inserted:
            result.append('\ndevelopment_web = "127.0.0.1:5173"\n'); inserted = True
        if not inserted:
            require("calcifer" not in parsed, "development_config_shape")
            result.append('\n[calcifer]\ndevelopment_web = "127.0.0.1:5173"\n')
        written_bytes = "".join(result).encode()
        written = tomllib.loads(written_bytes.decode())
        require(written.get("dev") is (True if dev else None) and written.get("calcifer", {}).get("development_web") == "127.0.0.1:5173", "development_config_shape")
        # close() restores the original bytes exactly, which takes `dev` and `development_web` out again.
        self.written = written_bytes
        self.config.write_bytes(written_bytes)

    @staticmethod
    def code(value: int) -> str:
        return f'''/// <reference types="vite/client" />
export const value = {value}
const proof = globalThis as typeof globalThis & {{ __cozyHmrProof?: number }}
proof.__cozyHmrProof = value
if (import.meta.hot) import.meta.hot.accept((updated) => {{ if (updated) proof.__cozyHmrProof = updated.value }})
'''

    def no_api_bridge(self) -> None:
        # Vite answers no API, and forwards none: every /v1 path is its fixed 404 (web/app/vite.config.ts).
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        try:
            with opener.open("http://127.0.0.1:5173/v1/project-storage", timeout=5) as response:
                status, body = response.status, response.read()
        except urllib.error.HTTPError as error:
            status, body = error.code, error.read()
            error.close()
        try:
            code = json.loads(body).get("error", {}).get("code")
        except (ValueError, AttributeError):
            code = None
        require(status == 404 and code == "development_api_in_calcifer", "development_native_bearer_bridge_present")

    def update(self) -> None:
        metadata = self.module.lstat()
        require(self.identity == (metadata.st_dev, metadata.st_ino) and not self.module.is_symlink(), "development_probe_replaced")
        self.module.write_text(self.code(2))

    def close(self) -> None:
        try:
            if self.process is not None:
                if self.birth is not None and alive(self.birth):
                    require(birth(self.process.pid) == self.birth, "development_process_replaced")
                    os.killpg(self.process.pid, 9)
                self.process.wait()
        finally:
            try:
                if self.identity is not None:
                    metadata = self.module.lstat()
                    require(self.identity == (metadata.st_dev, metadata.st_ino) and not self.module.is_symlink(), "development_probe_replaced")
                    self.module.unlink()
            finally:
                if self.written is not None:
                    require(not self.config.is_symlink() and self.config.read_bytes() == self.written, "development_config_changed")
                    if self.original is None:
                        self.config.unlink()
                    else:
                        self.config.write_bytes(self.original)
