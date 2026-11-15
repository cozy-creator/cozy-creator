# performance_data (cl-1006, Paul 11-15: "it has to be anonymous and there has to be an opt-out"): a run's report reaches the Hub with
# no credential and only its workload, GPU, package and versions; with the setting off, none does. Calcifer is started again with
# config.toml's calcifer.report_seconds (tests only) at 10, so a report goes 10 s after the first run it waits with, not 10 min. Off
# (`cozy config set`), an echo runs on local; on again, another runs at once: a report of the first, had it been kept, would wait with the
# second and go in its request. The Hub, through hubcheck's --keep, hears one request of one run, the second, and keeps it.
# kept_reports: every run report hubcheck kept, the one hub_away started again after it included.
kept_reports() { cat "$OUT/hub-kept.jsonl" "$OUT"/*/hub-kept.jsonl 2> /dev/null || true; }

performance_data() {
step "performance data (cl-1006): one anonymous report of a run with it on, none with it off"
local P=$OUT/reports config=$HOME/.cozy/config.toml marker=reports-$RANDOM$RANDOM before seen
[ -n "$HUB_CHECK" ] || { echo "performance data: no hubcheck between cozy and the Hub"; return; }
awk '/^report_seconds *=/ { next } { print } /^\[calcifer\]/ { print "report_seconds = 10"; put = 1 } END { if (!put) printf "\n[calcifer]\nreport_seconds = 10\n" }' "$config" > "$config.next" && mv "$config.next" "$config"
cozy --json restart > "$OUT/reports-restart.json" 2>&1 || { check "performance data: restart: $(tail -c 300 "$OUT/reports-restart.json")"; return; }
cozy --json config > "$OUT/reports-config.json" 2>&1 && J '.send_performance_data == true' "$OUT/reports-config.json" \
  || check "performance data: not on by default: $(tail -c 300 "$OUT/reports-config.json")"
cozy --json new --unencrypted "$P" > "$OUT/reports-new.json" 2>&1 || { check "performance data: new: $(tail -c 300 "$OUT/reports-new.json")"; return; }

cozy --json config set send_performance_data false > "$OUT/reports-off.json" 2>&1 && J '.send_performance_data == false' "$OUT/reports-off.json" \
  || check "performance data: config set false: $(tail -c 300 "$OUT/reports-off.json")"
grep -q '^send_performance_data = false' "$config" || check "performance data: off isn't in config.toml: $(cat "$config")"
cozy --json config get send_performance_data > "$OUT/reports-get.json" 2>&1 && J '.send_performance_data == false' "$OUT/reports-get.json" \
  || check "performance data: config get: $(tail -c 300 "$OUT/reports-get.json")"
before=$(kept_reports | wc -l)
in_project "$P" --json run minimax-h3-tools/echo "text=$marker-off" --on local > "$OUT/reports-run-off.ndjson" 2> "$OUT/reports-run-off.err" \
  || check "performance data: the run with it off: $(tail -c 300 "$OUT/reports-run-off.err")"
cozy --json config set send_performance_data on > "$OUT/reports-on.json" 2>&1 && J '.send_performance_data == true' "$OUT/reports-on.json" \
  || check "performance data: config set on: $(tail -c 300 "$OUT/reports-on.json")"
in_project "$P" --json run minimax-h3-tools/echo "text=$marker-on" --on local > "$OUT/reports-run-on.ndjson" 2> "$OUT/reports-run-on.err" \
  || check "performance data: the run with it on: $(tail -c 300 "$OUT/reports-run-on.err")"
seen=$(kept_reports | wc -l)
[ "$seen" -eq "$before" ] || check "performance data: a report came before the run with it on settled: the run with it off was reported"
for _ in $(seq 60); do
  seen=$(kept_reports | wc -l)
  [ "$seen" -le "$before" ] || break
  sleep 1
done
kept_reports | tail -n +$((before + 1)) > "$OUT/reports-kept.jsonl"
cozy --json config list > "$OUT/reports-list.json" 2>&1 || check "performance data: config list: $(tail -c 300 "$OUT/reports-list.json")"
python3 - "$OUT/reports-kept.jsonl" "$marker" <<'PY' || check "performance data: $(tail -c 600 "$OUT/reports-judged.txt")"
import json, sys
lines = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
allowed = {
    None: {"function", "release", "models", "software", "os", "gpu", "width", "size", "timing", "kernels", "outcome", "error_code"},
    "software": {"cozy", "tensord", "runtime", "tensorfs"},
    "gpu": {"name", "count", "memory_gib", "driver", "family"},
    "size": {"megapixels", "width", "height", "frames", "steps", "tokens"},
    "timing": {"total_ms", "load_ms", "steps", "step1_ms", "steady_ms", "warm", "peak_vram_gib"},
}
wrong = []
if len(lines) != 1:
    wrong.append(f"{len(lines)} requests, not one: {lines}")
for line in lines:
    runs = line["body"].get("runs", []) if isinstance(line["body"], dict) else []
    if line["credential"]:
        wrong.append("it carried a credential")
    if line["status"] != 202 or line["answer"] != {"kept": 1}:
        wrong.append(f"the Hub answered {line['status']} {line['answer']}, not 202 kept 1")
    if len(runs) != 1:
        wrong.append(f"{len(runs)} runs, not the one with it on")
    if sys.argv[2] in json.dumps(line["body"]):
        wrong.append("it holds the run's input")
    for run in runs:
        for part, keys in allowed.items():
            held = run if part is None else run.get(part) or {}
            extra = set(held) - keys
            if extra:
                wrong.append(f"{part or 'the run'} holds {sorted(extra)}")
        if not run.get("function", "").endswith("/minimax-h3-tools/echo") or run.get("outcome") != "succeeded" or run.get("timing", {}).get("total_ms", 0) < 1:
            wrong.append(f"not the echo's succeeded run: {run}")
        for model in run.get("models", []):
            extra = set(model) - {"slot", "model", "release", "lane", "precision", "adapters"}
            if extra:
                wrong.append(f"a model holds {sorted(extra)}")
with open(sys.argv[1].replace("reports-kept.jsonl", "reports-judged.txt"), "w") as out:
    out.write("; ".join(wrong) or f"one report: {json.dumps(lines[0]['body'])}")
sys.exit(1 if wrong else 0)
PY
}
