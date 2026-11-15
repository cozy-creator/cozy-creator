# e2e/cli/endedlogs.sh: an ended machine's logs are still read (cl-1012; Paul 11-15: "you can log ended machines yeah sure",
# "we'd keep other rental data for like 90 days or so, so that users can go back and see what they did"), and a locked project
# leaves nothing on this computer outside its folder (Paul 11-15: "once it's encrypted (locked) there is no traces of it on the
# computer elsewhere, in any way"). Linux, its own shard (`projectvault-ended-logs`: the vault driver is installed for it).
#   A stand-in machine (another user's cozy on this runner: the isolated Hub's docker rentals never serve here) runs an encrypted
#   project's script, which fails naming a random marker: the machine's live TensorD log shows it. Ended, `cozy machine logs`
#   still answers the machine's own lines Calcifer kept from its Status (none of them the marker), its tensorfs log names the
#   next command, and once the project is locked no file outside it holds the marker.
ended_logs() (
step "an ended machine's own lines are still read; a locked project's run is nowhere outside it (cl-1012)"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] \
  || { echo "ended logs: its stand-in machine is another user of a fresh GitHub runner"; return; }
local P=$OUT/ended-logs name=stand-e secrets view marker since hits code
secrets=$(mktemp -d)
trap 'in_project "$P" --json vault lock > /dev/null 2>&1 || true; rm -rf -- "$secrets"' EXIT
cozy --json machine show local > "$OUT/set-local.json" || { check "ended logs: machine show local: $(head -c 300 "$OUT/set-local.json")"; return; }
stand_in "$name" || return
since=$(python3 -c 'import time; print(time.time_ns())')
marker=$(python3 -c 'import secrets; print("CL1012" + secrets.token_hex(16))')
printf '%s\n' 'synthetic ended-logs passphrase' > "$secrets/pass"
chmod 600 "$secrets/pass"
cozy --json new "$P" --passphrase-file "$secrets/pass" --recovery-out "$secrets/recovery" > "$OUT/ended-new.json" 2> "$OUT/ended-new.err" \
  || { check "ended logs: new encrypted project: $(tail -c 300 "$OUT/ended-new.err")"; return; }
view=$(jq -r '.view // empty' "$OUT/ended-new.json")
[ -n "$view" ] && [ -d "$view" ] || { check "ended logs: the encrypted project has no unlocked view: $(head -c 300 "$OUT/ended-new.json")"; return; }

# The marker is only ever in the project's view, never in an argument (argv.log) or a file of $OUT.
cat > "$view/fail.py" <<'PY'
# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.43.1", "msgspec>=0.19"]
# ///
import msgspec
from cozy_runtime.author import App

app = App()


class Request(msgspec.Struct, forbid_unknown_fields=True):
    text: str


class Answer(msgspec.Struct):
    text: str


@app.entrypoint(accelerator=False)
def fail(payload: Request) -> Answer:
    raise RuntimeError(f"cl-1012 fixture fails naming {payload.text}")
PY
printf '{"text": "%s"}\n' "$marker" > "$view/input.json"
# It fails, so the pipeline does (pipefail): its outcome is read below.
in_project "$P" --json run "$view/fail.py/fail" --input "$view/input.json" --on "$name" 2>&1 | sed "s/$marker/<marker>/g" > "$OUT/ended-run.ndjson" || true
grep -q '"status":"failed"\|"status": "failed"' "$OUT/ended-run.ndjson" || check "ended logs: the fixture's run on $name didn't fail: $(tail -c 400 "$OUT/ended-run.ndjson")"
hits=$(cozy machine logs "$name" -n 0 2>&1 | grep -c "$marker" || true)
[ "${hits:-0}" -gt 0 ] || check "ended logs: $name's live TensorD log doesn't show the run's failure (the cell proves nothing then)"
cozy machine logs "$name" -n 0 2>&1 | sed "s/$marker/<marker>/g" > "$OUT/ended-live.txt" || true

step "ended: cozy machine logs answers the machine's own lines Calcifer kept"
cozy --json machine end "$name" --force -y > "$OUT/ended-end.json" 2>&1 || check "ended logs: machine end $name: $(tail -c 300 "$OUT/ended-end.json")"
code=0
cozy --json machine logs "$name" -n 0 > "$OUT/ended-kept.raw" 2> "$OUT/ended-kept.err" || code=$?
hits=$(grep -c "$marker" "$OUT/ended-kept.raw" || true)
sed -i "s/$marker/<marker>/g" "$OUT/ended-kept.raw"
mv "$OUT/ended-kept.raw" "$OUT/ended-kept.ndjson"
[ "$code" = 0 ] && J -s 'length > 0 and length <= 200 and all(.text | test(" machine: "))' "$OUT/ended-kept.ndjson" \
  || check "ended logs: machine logs $name, ended: exit $code, not its own lines: $(head -c 400 "$OUT/ended-kept.ndjson") $(tail -c 300 "$OUT/ended-kept.err")"
[ "${hits:-0}" = 0 ] || check "ended logs: Calcifer kept $hits line(s) of the project's run"
grep -qE 'readiness|executor cores|kernel boot' "$OUT/ended-kept.ndjson" || check "ended logs: the kept lines don't include its boot: $(head -c 400 "$OUT/ended-kept.ndjson")"
echo "ended logs: $(wc -l < "$OUT/ended-kept.ndjson") kept line(s) of $name, $(wc -l < "$OUT/ended-live.txt") live"
code=0
cozy --json machine logs "$name" tensorfs > "$OUT/ended-tensorfs.json" 2>&1 || code=$?
[ "$code" != 0 ] && J --arg n "$name" '.error.code == "machine_ended" and (.error.next | index("cozy machine logs " + $n))' "$OUT/ended-tensorfs.json" \
  || check "ended logs: machine logs $name tensorfs, ended: exit $code, not a refusal naming its TensorD log: $(head -c 300 "$OUT/ended-tensorfs.json")"

step "locked: the project's run is in no file outside it"
in_project "$P" --json vault lock > "$OUT/ended-lock.json" 2>&1 || check "ended logs: lock: $(head -c 300 "$OUT/ended-lock.json")"
MARKER=$marker python3 - "$E2E/cli" "$since" > "$OUT/ended-scan.json" <<'PY_ENDED_SCAN' || check "ended logs: the locked project's run is outside it: $(head -c 600 "$OUT/ended-scan.json")"
import json, os, sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from private_execution import contains
since, marker = int(sys.argv[2]), os.environ["MARKER"].encode()
roots = sorted({Path(p).resolve() for p in (Path.home(), "/tmp", os.environ.get("TMPDIR", "/tmp"), os.environ.get("RUNNER_TEMP", "/tmp"))})
leaked, seen, checked = [], set(), 0
for root in roots:
    for directory, dirs, files in os.walk(root, followlinks=False):
        for name in files:
            path = Path(directory) / name
            if path in seen or path.is_symlink():
                continue
            seen.add(path)
            try:
                if path.stat().st_mtime_ns < since or not path.is_file():
                    continue
                checked += 1
                if contains(path, [marker]):
                    leaked.append(str(path))
            except OSError:
                continue  # gone meanwhile, or another user's (the stand-in's own machine): nothing this computer's cozy wrote
print(json.dumps({"roots": [str(root) for root in roots], "checked_new_or_modified_files": checked, "leaked_paths": leaked}))
assert checked > 0 and not leaked
PY_ENDED_SCAN
)
