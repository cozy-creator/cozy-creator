"""Actual ledger initialization in private Docker tmpfs; never fill or mount host /dev/shm."""
from pathlib import Path
import argparse
import hashlib
import json
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("peer", type=Path)
parser.add_argument("image", help="already cached image with compatible libc and /bin/sh")
parser.add_argument("output", type=Path, help="new receipt directory")
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
binary = args.peer.resolve()
image = subprocess.check_output(["docker", "image", "inspect", args.image, "--format", "{{.Id}}"], text=True).strip()
cases = {
    "tiny-repeat": ("16k", r'''mkdir /dev/shm/ledger
/proof/peer inspect /dev/shm/ledger; first=$?
set -- /dev/shm/ledger/varena-*; file=$1
printf 'first=%s size=%s blocks=%s\n' "$first" "$(stat -c %s "$file")" "$(stat -c %b "$file")"
hash1=$(sha256sum "$file" | cut -d' ' -f1)
/proof/peer inspect /dev/shm/ledger; second=$?
hash2=$(sha256sum "$file" | cut -d' ' -f1)
printf 'second=%s same_bytes=%s\n' "$second" "$([ "$hash1" = "$hash2" ] && echo true || echo false)"
[ "$first" = 1 ] && [ "$second" = 1 ] && [ "$hash1" = "$hash2" ]'''),
    "roomy-repeat": ("64k", r'''mkdir /dev/shm/ledger
/proof/peer inspect /dev/shm/ledger || exit 1
set -- /dev/shm/ledger/varena-*; file=$1
hash1=$(sha256sum "$file" | cut -d' ' -f1)
/proof/peer inspect /dev/shm/ledger || exit 1
hash2=$(sha256sum "$file" | cut -d' ' -f1)
printf 'same_bytes=%s size=%s blocks=%s\n' "$([ "$hash1" = "$hash2" ] && echo true || echo false)" "$(stat -c %s "$file")" "$(stat -c %b "$file")"
[ "$hash1" = "$hash2" ]'''),
    "incompatible-sparse": ("16k", r'''mkdir /dev/shm/ledger
/proof/peer inspect /dev/shm/ledger; first=$?
[ "$first" = 1 ] || exit 2
set -- /dev/shm/ledger/varena-*; file=$1
printf 'VARENA3\000' | dd of="$file" bs=1 conv=notrunc status=none
printf '\347\003\000\000\000\000\000\000' | dd of="$file" bs=1 seek=8 conv=notrunc status=none
hash1=$(sha256sum "$file" | cut -d' ' -f1)
/proof/peer inspect /dev/shm/ledger; second=$?
hash2=$(sha256sum "$file" | cut -d' ' -f1)
printf 'incompatible_exit=%s same_bytes=%s\n' "$second" "$([ "$hash1" = "$hash2" ] && echo true || echo false)"
[ "$second" = 1 ] && [ "$hash1" = "$hash2" ]'''),
}
report = {"passed": False, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
          "image": image, "host_shm_mounted": False, "cases": {}}
try:
    for name, (size, script) in cases.items():
        command = ["docker", "run", "--rm", "--pull=never", "--runtime=runc", "--ipc=private",
                   "--network=none", "--read-only", "--cpus=1", "--memory=128m", "--ulimit", "core=0",
                   "--env", "LC_ALL=C", "--shm-size=" + size,
                   "--mount", f"type=bind,source={binary},target=/proof/peer,readonly",
                   "--entrypoint", "/bin/sh", image, "-c", script]
        result = subprocess.run(command, capture_output=True, text=True)
        receipt = {"command": command, "exit_code": result.returncode,
                   "stdout": result.stdout, "stderr": result.stderr}
        (args.output / (name + ".json")).write_text(json.dumps(receipt, indent=2) + "\n")
        assert result.returncode == 0, receipt
        if name == "tiny-repeat":
            assert result.stderr.count("No space left on device") == 2, receipt
        if name == "incompatible-sparse":
            assert result.stderr.count("No space left on device") == 1 and "incompatible ledger" in result.stderr, receipt
        report["cases"][name] = {"passed": True, "receipt": name + ".json"}
    report["passed"] = True
finally:
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report, indent=2))
