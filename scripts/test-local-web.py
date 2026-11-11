#!/usr/bin/env python3
"""Run the vault browser fixture against an owned local Calcifer profile."""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
import zipfile


ROOT = Path(__file__).resolve().parents[1]
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def command(argv, *, cwd=ROOT, env=None, stdout=None):
    subprocess.run([str(arg) for arg in argv], cwd=cwd, env=env, stdout=stdout,
                   stderr=subprocess.STDOUT if stdout else None, check=True)


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def snapshot_binary(source, profile):
    sha256 = digest(source)
    directory = profile / "binaries" / sha256
    directory.mkdir(parents=True, exist_ok=True)
    binary = directory / "cozy"
    if not binary.is_file() or digest(binary) != sha256:
        staged = directory / "cozy.part"
        shutil.copyfile(source, staged)
        staged.chmod(0o700)
        if digest(staged) != sha256:
            staged.unlink()
            raise RuntimeError("the build changed while it was copied; rerun against the completed binary")
        staged.replace(binary)
    return binary, sha256


def request(holder, path, method="GET"):
    url = f"http://{holder['address']}{path}"
    req = urllib.request.Request(url, method=method,
                                 headers={"Authorization": f"Bearer {holder['token']}"})
    with HTTP.open(req, timeout=2) as response:
        body = response.read()
        return json.loads(body) if body else None


def ready(process, holder_path, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("test Calcifer exited during startup; see calcifer.log")
        try:
            holder = json.loads(holder_path.read_text())
            if holder["pid"] != process.pid:
                time.sleep(0.1)
                continue
            host, _ = holder["address"].rsplit(":", 1)
            if host != "127.0.0.1":
                raise RuntimeError("test Calcifer did not bind IPv4 loopback")
            status = request(holder, "/v1/status")
            return holder, status
        except (OSError, ValueError, KeyError, urllib.error.URLError):
            time.sleep(0.1)
    raise RuntimeError("test Calcifer did not become ready; see calcifer.log")


def stop(process, holder):
    deadline = time.monotonic() + 30
    while process.poll() is None and time.monotonic() < deadline:
        if holder:
            try:
                request(holder, "/v1/down", "POST")
            except (OSError, ValueError, urllib.error.URLError):
                pass
        else:
            process.terminate()
        try:
            process.wait(timeout=1)
        except subprocess.TimeoutExpired:
            pass
    if process.poll() is None:
        raise RuntimeError(f"test Calcifer {process.pid} is still draining; profile retained")


def stop_worker(home, binary):
    machine = home / ".cozy/machine/var/lib/cozy/machine"
    record = machine / "agent.lock.holder"
    if not record.is_file():
        return None
    pid = json.loads(record.read_text())["pid"]
    process = Path("/proc") / str(pid)
    if not process.exists():
        return pid
    if not (process / "exe").samefile(binary) or (process / "cwd").resolve() != home:
        raise RuntimeError("the test worker record names an unexpected process")
    birth = (process / "stat").read_text().rsplit(")", 1)[1].split()[19]
    os.kill(pid, signal.SIGTERM)
    deadline = time.monotonic() + 30
    while process.exists() and time.monotonic() < deadline:
        try:
            current = (process / "stat").read_text().rsplit(")", 1)[1].split()[19]
        except FileNotFoundError:
            break
        if current != birth:
            break
        time.sleep(0.1)
    else:
        if process.exists():
            raise RuntimeError(f"test worker {pid} is still stopping; profile retained")
    lock = machine / "agent.lock"
    if lock.is_file():
        with lock.open("r+") as held:
            fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
    return pid


def own_profile(profile):
    actual_home = Path.home().resolve()
    if profile == actual_home or actual_home.is_relative_to(profile) or profile.is_relative_to(actual_home / ".cozy"):
        raise RuntimeError("choose a dedicated test profile outside the installed Cozy home")
    marker = profile / ".cozy-local-web-profile"
    if profile.exists() and any(profile.iterdir()) and not marker.is_file():
        raise RuntimeError("refusing to reuse an unmarked, nonempty test profile")
    profile.mkdir(parents=True, exist_ok=True, mode=0o700)
    marker.touch(mode=0o600, exist_ok=True)


def prepare_profile(profile):
    for part in ("home", "tmp", "cache", "driver", "results", "tensorfs", "uv-cache"):
        (profile / part).mkdir(exist_ok=True, mode=0o700)
    home = profile / "home"
    cozy_home = home / ".cozy"
    cozy_home.mkdir(exist_ok=True, mode=0o700)
    holder_path = cozy_home / "calcifer/calcifer.lock.holder"
    for record in (holder_path, cozy_home / "machine/var/lib/cozy/machine/agent.lock.holder"):
        if not record.is_file():
            continue
        try:
            prior = json.loads(record.read_text())
            os.kill(prior["pid"], 0)
        except (OSError, ValueError, KeyError):
            pass
        else:
            raise RuntimeError("this test profile already has a running process from an earlier test")
    (cozy_home / "config.toml").write_text(f'hub_url = "http://127.0.0.1:9"\n[calcifer]\nport = {port()}\n')
    return home, holder_path


def provider(args, profile):
    if args.securefs:
        return args.securefs.resolve(strict=True)
    if not os.access("/dev/fuse", os.R_OK | os.W_OK) or not shutil.which("fusermount3"):
        raise RuntimeError("local vault tests require /dev/fuse and fusermount3")
    host = subprocess.check_output(["rustc", "-vV"], text=True, cwd=ROOT)
    target = next(line.split(": ", 1)[1] for line in host.splitlines() if line.startswith("host: "))
    archive = subprocess.check_output([sys.executable, ROOT / "scripts/prepare-securefs.py", "--target", target,
                                       "--out-dir", profile / "provider"], text=True, cwd=ROOT).strip()
    with zipfile.ZipFile(archive) as source:
        data = source.read("securefs")
    executable = profile / "provider/securefs"
    if not executable.exists() or executable.read_bytes() != data:
        executable.write_bytes(data)
        executable.chmod(0o700)
    return executable


def run(args):
    profile = args.profile.resolve()
    os.umask(0o077)
    own_profile(profile)
    with (profile / ".runner.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return run_locked(args, profile)


def run_locked(args, profile):
    home, holder_path = prepare_profile(profile)
    harness = args.harness.resolve()
    fixture = harness / "e2e/cli/web/vault.js"
    if not fixture.is_file():
        raise RuntimeError(f"missing fixture: {fixture}")
    started = time.monotonic()
    build_env = os.environ.copy()
    build_env.update(CARGO_TARGET_DIR=str(args.target_dir.resolve()), CARGO_PROFILE_DEV_DEBUG="0", CARGO_INCREMENTAL="1")
    if args.binary:
        binary = args.binary.resolve(strict=True)
    else:
        if not args.vite:
            command(["bash", "web/build.sh"])
        command(["cargo", "build", "--locked", "--jobs", args.jobs, "-p", "cozy"], env=build_env)
        binary = args.target_dir.resolve() / "debug/cozy"
    binary_source = binary
    binary, binary_sha256 = snapshot_binary(binary_source, profile)
    version = subprocess.check_output([binary, "--version"], text=True).strip()
    if subprocess.run([binary, "vault", "--help"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
        raise RuntimeError("this binary does not contain the vault feature")
    (home / ".cozy/installed").write_text(str(binary) + "\n")
    build_seconds = time.monotonic() - started

    driver = profile / "driver"
    module = driver / "node_modules/playwright-core"
    if not (module / "package.json").is_file():
        (driver / "package.json").write_text('{"private":true}\n')
        command(["pnpm", "--dir", driver, "add", "--ignore-scripts", "playwright-core@1.55.0"])
    if json.loads((module / "package.json").read_text())["version"] != "1.55.0":
        raise RuntimeError("test profile has a different Playwright version")
    chrome = args.chrome or shutil.which("google-chrome")
    if not chrome:
        raise RuntimeError("Google Chrome is required; pass --chrome")
    securefs = provider(args, profile)

    # HOME here is the child process's actual test home, never the invoking shell's home.
    env = os.environ.copy()
    env.update(HOME=str(home), TMPDIR=str(profile / "tmp"), XDG_CACHE_HOME=str(profile / "cache"),
               TENSORFS_HOME=str(profile / "tensorfs"), UV_CACHE_DIR=str(profile / "uv-cache"),
               COZY_HUB_URL="http://127.0.0.1:9", HOLDER=str(holder_path), COZY=str(binary),
               PLAYWRIGHT=str(module), CHROME=str(Path(chrome).resolve()), COZY_TEST_PROGRESS="1")
    if securefs:
        env["COZY_SECUREFS_PROVIDER"] = str(securefs)
    result = profile / "results" / f"{time.strftime('%Y%m%dT%H%M%S')}-{time.time_ns() % 1_000_000_000:09d}"
    result.mkdir()
    holder = None
    vite = None
    server = None
    browser_process = None
    summary = {"binary": str(binary), "binary_source": str(binary_source), "version": version, "binary_sha256": binary_sha256,
               "fixture": str(fixture), "fixture_sha256": hashlib.sha256(fixture.read_bytes()).hexdigest(),
               "build_seconds": round(build_seconds, 3)}
    source = subprocess.run(["git", "rev-parse", "HEAD"], cwd=harness, text=True, capture_output=True)
    if source.returncode == 0:
        summary["harness_commit"] = source.stdout.strip()
    try:
        with (result / "calcifer.log").open("w") as log, (result / "vite.log").open("w") as vite_log:
            server = subprocess.Popen([binary, "calcifer"], cwd=home, env=env, stdout=log, stderr=subprocess.STDOUT)
            holder, status = ready(server, holder_path)
            summary.update(calcifer_pid=server.pid, calcifer_version=status["version"], address=holder["address"])
            if args.vite:
                if not (ROOT / "web/app/node_modules").is_dir():
                    command(["pnpm", "--dir", "web/app", "install", "--frozen-lockfile"])
                web_port = port()
                vite = subprocess.Popen(["pnpm", "--dir", "web/app", "dev", "--host", "127.0.0.1", "--port", str(web_port), "--strictPort"],
                                        cwd=ROOT, env=env, stdout=vite_log, stderr=subprocess.STDOUT, start_new_session=True)
                summary.update(vite_pid=vite.pid, vite_port=web_port)
                env["PAGE_ORIGIN"] = f"http://127.0.0.1:{web_port}"
                deadline = time.monotonic() + 30
                while True:
                    try:
                        with HTTP.open(env["PAGE_ORIGIN"], timeout=1):
                            break
                    except OSError:
                        if vite.poll() is not None or time.monotonic() >= deadline:
                            raise RuntimeError("test Vite did not become ready; see vite.log")
                        time.sleep(0.1)
            else:
                env.pop("PAGE_ORIGIN", None)
            began = time.monotonic()
            with (result / "vault.json").open("w") as out, (result / "vault.err").open("w") as err:
                browser_process = subprocess.Popen(["node", fixture], cwd=harness, env=env, stdout=out,
                                                   stderr=subprocess.PIPE, text=True, start_new_session=True)
                for line in browser_process.stderr:
                    err.write(line)
                    if line.startswith("[local-vault] "):
                        print(line.rstrip(), flush=True)
                code = browser_process.wait()
            summary.update(test_seconds=round(time.monotonic() - began, 3), exit_code=code)
            return code
    except KeyboardInterrupt:
        summary["interrupted"] = True
        raise
    finally:
        cleanup_errors = []
        try:
            if browser_process and browser_process.poll() is None:
                os.killpg(browser_process.pid, signal.SIGINT)
                browser_process.wait(timeout=10)
        except (OSError, subprocess.SubprocessError) as error:
            cleanup_errors.append(str(error))
        try:
            if vite and vite.poll() is None:
                os.killpg(vite.pid, signal.SIGINT)
                vite.wait(timeout=10)
        except (OSError, subprocess.SubprocessError) as error:
            cleanup_errors.append(str(error))
        try:
            if server:
                stop(server, holder)
        except (OSError, RuntimeError, subprocess.SubprocessError) as error:
            cleanup_errors.append(str(error))
        try:
            summary["worker_pid"] = stop_worker(home, binary)
        except (OSError, RuntimeError) as error:
            cleanup_errors.append(str(error))
        summary["cleanup_errors"] = cleanup_errors
        summary["total_seconds"] = round(time.monotonic() - started, 3)
        (result / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
        print(f"Local test results: {result}")
        if cleanup_errors:
            raise RuntimeError(". ".join(cleanup_errors))


def main():
    if sys.platform != "linux":
        raise RuntimeError("this local runner currently supports Linux")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, help="reuse this existing Cozy binary")
    parser.add_argument("--harness", type=Path, default=ROOT, help="checkout containing the vault browser fixture")
    parser.add_argument("--profile", type=Path, default=ROOT / "target/local-web", help="persistent owned test profile")
    parser.add_argument("--target-dir", type=Path, default=ROOT / "target/dev-tests", help="persistent incremental Rust target")
    parser.add_argument("--jobs", default="4", help="Cargo build jobs")
    parser.add_argument("--securefs", type=Path, help="explicit supported securefs helper")
    parser.add_argument("--chrome", help="Google Chrome executable")
    parser.add_argument("--vite", action="store_true", help="serve current WebUI sources through a dedicated Vite process")
    parser.add_argument("--watch", action="store_true", help="queue another build/test after source changes")
    args = parser.parse_args()
    if args.watch:
        words = [word for word in sys.argv[1:] if word != "--watch"]
        os.execvp("watchexec", ["watchexec", "--debounce", "500ms", "--on-busy-update", "queue", "--shell", "none",
                              "--stop-signal", "SIGINT", "--stop-timeout", "90s",
                              "--watch", str(ROOT / "crates"), "--watch", str(ROOT / "web"), "--watch", str(args.harness.resolve() / "e2e/cli/web"),
                              "--watch", str(ROOT / "Cargo.toml"), "--watch", str(ROOT / "Cargo.lock"),
                              "--watch", str(ROOT / "rust-toolchain.toml"), "--watch", str(Path(__file__).resolve()),
                              "--ignore", "**/target/**", "--ignore", "**/node_modules/**", "--ignore", "**/dist/**",
                              "--", sys.executable, str(Path(__file__).resolve()), *words])
    return run(args)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        print("Local test interrupted", file=sys.stderr)
        sys.exit(130)
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Local test failed: {error}", file=sys.stderr)
        sys.exit(1)
