# performance_data (cl-1006, Paul 11-15: "it has to be anonymous and there has to be an opt-out"): a run's report reaches the Hub with
# no credential, under a random install ID, holding only its workload, GPU, package, versions and how it went; with the setting off,
# none does, and turning it off deletes the ID. Calcifer is started again with config.toml's calcifer.report_seconds (tests only) at
# 10, so a report goes 10 s after the first run it waits with, not 10 min. Off (`cozy config set`), an echo runs on local; on again,
# another runs at once: a report of the first, had it been kept, would wait with the second and go in its request. The Hub, through
# hubcheck's --keep, hears one request of one run, the second, and keeps it. Off and on again, a third run's report goes under a new ID.

# kept_reports: every run report hubcheck kept, the one hub_away started again after it included.
kept_reports() { cat "$OUT/hub-kept.jsonl" "$OUT"/*/hub-kept.jsonl 2> /dev/null | grep -F '"operation":"POST /v1/run-reports"' || true; }

# reported_after <count> <file>: waits up to a minute for a report past the first <count>, and keeps the new ones in <file>.
reported_after() {
  for _ in $(seq 60); do
    [ "$(kept_reports | wc -l)" -le "$1" ] || break
    sleep 1
  done
  kept_reports | tail -n +$(($1 + 1)) > "$2"
}

# set_reports <on|off> <name>: `cozy config set send_performance_data`, and it says so.
set_reports() {
  local want=true
  [ "$1" = on ] || want=false
  cozy --json config set send_performance_data "$1" > "$OUT/reports-$2.json" 2>&1 && J ".send_performance_data == $want" "$OUT/reports-$2.json" \
    || check "performance data: config set $1: $(tail -c 300 "$OUT/reports-$2.json")"
}

performance_data() {
step "performance data (cl-1006): one anonymous report of a run with it on, none with it off; off resets its install ID"
local P=$OUT/reports config=$HOME/.cozy/config.toml id=$HOME/.cozy/calcifer/report-id marker=reports-$RANDOM$RANDOM before first=''
[ -n "$HUB_CHECK" ] || { echo "performance data: no hubcheck between cozy and the Hub"; return; }
awk '/^report_seconds *=/ { next } { print } /^\[calcifer\]/ { print "report_seconds = 10"; put = 1 } END { if (!put) printf "\n[calcifer]\nreport_seconds = 10\n" }' "$config" > "$config.next" && mv "$config.next" "$config"
cozy --json restart > "$OUT/reports-restart.json" 2>&1 || { check "performance data: restart: $(tail -c 300 "$OUT/reports-restart.json")"; return; }
cozy --json config > "$OUT/reports-config.json" 2>&1 && J '.send_performance_data == true' "$OUT/reports-config.json" \
  || check "performance data: not on by default: $(tail -c 300 "$OUT/reports-config.json")"
cozy --json new --unencrypted "$P" > "$OUT/reports-new.json" 2>&1 || { check "performance data: new: $(tail -c 300 "$OUT/reports-new.json")"; return; }

set_reports off off
grep -q '^send_performance_data = false' "$config" || check "performance data: off isn't in config.toml: $(cat "$config")"
[ ! -e "$id" ] || check "performance data: off, yet $id is still there"
cozy --json config get send_performance_data > "$OUT/reports-get.json" 2>&1 && J '.send_performance_data == false' "$OUT/reports-get.json" \
  || check "performance data: config get: $(tail -c 300 "$OUT/reports-get.json")"
before=$(kept_reports | wc -l)
in_project "$P" --json run minimax-h3-tools/echo "text=$marker-off" --on local > "$OUT/reports-run-off.ndjson" 2> "$OUT/reports-run-off.err" \
  || check "performance data: the run with it off: $(tail -c 300 "$OUT/reports-run-off.err")"
set_reports on on
in_project "$P" --json run minimax-h3-tools/echo "text=$marker-on" --on local > "$OUT/reports-run-on.ndjson" 2> "$OUT/reports-run-on.err" \
  || check "performance data: the run with it on: $(tail -c 300 "$OUT/reports-run-on.err")"
[ "$(kept_reports | wc -l)" -eq "$before" ] || check "performance data: a report came before the run with it on settled: the run with it off was reported"
reported_after "$before" "$OUT/reports-kept.jsonl"
first=$(cat "$id" 2> /dev/null || true)

# Off and on again: a new install ID.
set_reports off reset-off
[ ! -e "$id" ] || check "performance data: off again, yet $id is still there"
set_reports on reset-on
before=$(kept_reports | wc -l)
in_project "$P" --json run minimax-h3-tools/echo "text=$marker-again" --on local > "$OUT/reports-run-again.ndjson" 2> "$OUT/reports-run-again.err" \
  || check "performance data: the run after on again: $(tail -c 300 "$OUT/reports-run-again.err")"
reported_after "$before" "$OUT/reports-kept-again.jsonl"
cozy --json config list > "$OUT/reports-list.json" 2>&1 || check "performance data: config list: $(tail -c 300 "$OUT/reports-list.json")"
python3 - "$OUT/reports-kept.jsonl" "$OUT/reports-kept-again.jsonl" "$marker" "$first" "$(cat "$id" 2> /dev/null || true)" "$OUT/reports-judged.txt" <<'PY' \
  || check "performance data: $(tail -c 800 "$OUT/reports-judged.txt")"
import json, re, sys
first_file, again_file, marker, first_id, again_id, judged = sys.argv[1:7]
allowed = {
    None: {"function", "release", "lane", "models", "software", "os", "os_release", "host_memory_gib", "gpu", "width", "size", "timing",
           "memory", "kernels", "outcome", "error_code", "failure", "warnings", "slow"},
    "software": {"cozy", "tensord", "runtime", "tensorfs", "torch"},
    "gpu": {"name", "count", "memory_gib", "limit_gib", "driver", "family"},
    "size": {"megapixels", "width", "height", "frames", "steps", "tokens", "seconds", "refs"},
    "timing": {"total_ms", "lead_ms", "load_ms", "steps", "step1_ms", "steady_ms", "warm", "peak_vram_gib"},
    "memory": {"retries", "ooms", "paged_gib"},
    "failure": {"oom", "stage"},
}
wrong, installs = [], []
for name, path in (("with it on", first_file), ("on again", again_file)):
    lines = [json.loads(line) for line in open(path) if line.strip()]
    if len(lines) != 1:
        wrong.append(f"{name}: {len(lines)} requests, not one: {lines}")
    for line in lines:
        body = line["body"] if isinstance(line["body"], dict) else {}
        runs = body.get("runs", [])
        installs.append(body.get("install", ""))
        if set(body) != {"install", "runs"}:
            wrong.append(f"{name}: the request holds {sorted(body)}, not install and runs")
        if not re.fullmatch(r"[0-9a-f]{32}", body.get("install", "")):
            wrong.append(f"{name}: install {body.get('install')!r} is not 32 hex digits")
        if line["credential"]:
            wrong.append(f"{name}: it carried a credential")
        if line["status"] != 202 or line["answer"] != {"kept": 1}:
            wrong.append(f"{name}: the Hub answered {line['status']} {line['answer']}, not 202 kept 1")
        if len(runs) != 1:
            wrong.append(f"{name}: {len(runs)} runs, not one")
        if marker in json.dumps(body):
            wrong.append(f"{name}: it holds a run's input")
        for run in runs:
            for part, keys in allowed.items():
                held = run if part is None else run.get(part) or {}
                extra = set(held) - keys
                if extra:
                    wrong.append(f"{name}: {part or 'the run'} holds {sorted(extra)}")
            if not run.get("function", "").endswith("/minimax-h3-tools/echo") or run.get("outcome") != "succeeded" or run.get("timing", {}).get("total_ms", 0) < 1:
                wrong.append(f"{name}: not the echo's succeeded run: {run}")
            for model in run.get("models", []):
                extra = set(model) - {"slot", "model", "release", "lane", "precision", "adapters"}
                if extra:
                    wrong.append(f"{name}: a model holds {sorted(extra)}")
if len(installs) == 2:
    if installs[0] != first_id:
        wrong.append(f"the first report's install {installs[0]} isn't calcifer/report-id's {first_id}")
    if installs[1] != again_id:
        wrong.append(f"the second report's install {installs[1]} isn't calcifer/report-id's {again_id}")
    if installs[0] == installs[1]:
        wrong.append("off and on again kept the same install ID")
with open(judged, "w") as out:
    out.write("; ".join(wrong) or f"two reports under {', '.join(installs)}: {open(first_file).read().strip()[:800]}")
sys.exit(1 if wrong else 0)
PY
}
