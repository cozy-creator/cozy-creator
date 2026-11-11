"""Run existing CLI coverage against verified candidate and retained Actions call logs."""
import argparse
import hashlib
import http.client
import io
import json
from pathlib import Path
import re
import subprocess
import urllib.error
import urllib.request
import zipfile

REPOSITORY = "cozy-creator/cozy-creator"
SHARDS = {
    "runs", "executors", "commands", "follow", "web-core", "web-layout-1280",
    "web-layout-1600", "web-layout-2560", "projectdb", "projectvault", "local-code",
    "web", "project-stream-web", "web-scale", "projectdb-latency", "projectdb-existing",
    "projectdb-cancel", "projectdb-live", "projectdb-instant", "projectdb-offline",
}


def api(path: str) -> dict:
    return json.loads(subprocess.check_output(["gh", "api", f"repos/{REPOSITORY}/{path}"], text=True))


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class Artifact(io.RawIOBase):
    """Read ZIP members by exact byte range without downloading the entire artifact."""
    def __init__(self, metadata: dict):
        self.length = metadata["size_in_bytes"]
        self.position = 0
        self.start = 0
        self.buffer = b""
        self.transferred = 0
        token = subprocess.check_output(["gh", "auth", "token"], text=True).strip()
        connection = http.client.HTTPSConnection("api.github.com", timeout=30)
        try:
            connection.request("GET", f"/repos/{REPOSITORY}/actions/artifacts/{metadata['id']}/zip",
                               headers={"Authorization": f"Bearer {token}", "User-Agent": "Cozy-coverage"})
            response = connection.getresponse()
            if response.status != 302:
                raise RuntimeError(f"artifact API returned HTTP {response.status}")
            self.url = response.getheader("Location")
            if not self.url or not self.url.startswith("https://"):
                raise RuntimeError("artifact API did not supply an HTTPS download")
        finally:
            connection.close()

    def seekable(self):
        return True

    def tell(self):
        return self.position

    def seek(self, offset, whence=io.SEEK_SET):
        base = 0 if whence == io.SEEK_SET else self.position if whence == io.SEEK_CUR else self.length
        self.position = base + offset
        if self.position < 0:
            raise ValueError("negative artifact seek")
        return self.position

    def read(self, count=-1):
        count = min(self.length - self.position, count if count >= 0 else self.length - self.position)
        if count <= 0:
            return b""
        if not self.start <= self.position <= self.position + count <= self.start + len(self.buffer):
            amount = min(self.length - self.position, max(count, 65536))
            end = self.position + amount - 1
            request = urllib.request.Request(self.url, headers={
                "Range": f"bytes={self.position}-{end}", "Accept-Encoding": "identity",
            })
            try:
                with urllib.request.urlopen(request, timeout=30) as response:
                    if response.status != 206 or response.headers.get("Content-Range") != f"bytes {self.position}-{end}/{self.length}":
                        raise RuntimeError("artifact range was not exact")
                    self.buffer = response.read(amount + 1)
            except urllib.error.HTTPError as error:
                raise RuntimeError(f"artifact download returned HTTP {error.code}") from None
            if len(self.buffer) != amount:
                raise RuntimeError("artifact range had an unexpected length")
            self.start = self.position
            self.transferred += amount
        offset = self.position - self.start
        self.position += count
        return self.buffer[offset:offset + count]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True)
    parser.add_argument("--checkout", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--candidate", type=int, required=True)
    parser.add_argument("--runs", required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.source):
        parser.error("source must be an exact commit")
    runs = [int(value) for value in args.runs.split(",")]
    if not runs or len(set(runs)) != len(runs) or any(run <= 0 for run in runs):
        parser.error("runs must be distinct positive Actions run IDs")
    actual_source = subprocess.check_output(["git", "-C", str(args.checkout), "rev-parse", "HEAD"], text=True).strip()
    if actual_source != args.source:
        raise RuntimeError("coverage checkout differs from the requested source")
    args.out.mkdir(parents=True, exist_ok=True)
    record = {"source": args.source, "runs": [], "logs": []}
    logs = []
    primary_shards = set()
    for index, run in enumerate(runs):
        metadata = api(f"actions/runs/{run}")
        if metadata["status"] != "completed" or metadata["conclusion"] == "cancelled":
            raise RuntimeError(f"run {run} has not completed with retained test results")
        jobs = api(f"actions/runs/{run}/jobs?per_page=100")
        affected = next(job for job in jobs["jobs"] if job["name"] == "affected")
        text = subprocess.check_output(["gh", "api", "--allow-escape-sequences", f"repos/{REPOSITORY}/actions/jobs/{affected['id']}/logs"], text=True)
        sources = re.findall(r"Z ref=([0-9a-f]{40})\s*$", text, re.MULTILINE)
        if sources != [args.source]:
            raise RuntimeError(f"run {run} did not test the requested product source")
        record["runs"].append({"id": run, "head": metadata["head_sha"], "conclusion": metadata["conclusion"]})
        artifacts = api(f"actions/runs/{run}/artifacts?per_page=100")
        if artifacts["total_count"] > 100:
            raise RuntimeError("unexpected artifact pagination")
        for artifact in artifacts["artifacts"]:
            name = artifact["name"].removeprefix("e2e-cli-linux-")
            if artifact["name"] == name or name not in SHARDS or artifact["expired"]:
                continue
            remote = Artifact(artifact)
            with zipfile.ZipFile(remote) as archive:
                data = archive.read("argv.log")
            path = args.out / "argv" / str(run) / name / "argv.log"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            logs.extend(["--log", str(path)])
            record["logs"].append({"run": run, "shard": name, "artifact": artifact["id"],
                                   "sha256": digest(data), "bytes": len(data), "downloaded_bytes": remote.transferred})
            if index == 0:
                primary_shards.add(name)
    if primary_shards != SHARDS:
        raise RuntimeError(f"primary run is missing shard logs: {sorted(SHARDS - primary_shards)}")
    candidate = api(f"actions/artifacts/{args.candidate}")
    if candidate["workflow_run"]["id"] != runs[0] or candidate["workflow_run"]["head_sha"] != args.source:
        raise RuntimeError("candidate artifact is not from the primary source run")
    archive_path = args.out / "candidate.zip"
    with archive_path.open("wb") as output:
        subprocess.run(["gh", "api", "--allow-escape-sequences", f"repos/{REPOSITORY}/actions/artifacts/{args.candidate}/zip"], stdout=output, check=True)
    archive_bytes = archive_path.read_bytes()
    if candidate.get("digest") != "sha256:" + digest(archive_bytes):
        raise RuntimeError("candidate archive differs from its Actions digest")
    with zipfile.ZipFile(io.BytesIO(archive_bytes)) as archive:
        members = [name for name in archive.namelist() if Path(name).name == "cozy"]
        if len(members) != 1:
            raise RuntimeError("candidate does not contain exactly one Cozy binary")
        binary = archive.read(members[0])
    executable = args.out / "cozy"
    executable.write_bytes(binary)
    executable.chmod(0o755)
    version = subprocess.check_output([str(executable), "--version"], text=True).strip()
    if not version.endswith("+dev.g" + args.source[:8]):
        raise RuntimeError("candidate version differs from its source")
    record["candidate"] = {"artifact": args.candidate, "archive_sha256": digest(archive_bytes),
                           "binary_sha256": digest(binary), "version": version}
    (args.out / "INPUTS.json").write_text(json.dumps(record, indent=2) + "\n")
    command = ["python3", "-I", str(args.checkout / "e2e/coverage.py"), "check", str(executable),
               str(args.checkout / "e2e/coverage.toml"), "--tiers", "pr", *logs,
               "--source", str(args.checkout / "crates/cli/src")]
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    (args.out / "coverage.log").write_text(result.stdout)
    print(result.stdout, end="")
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
