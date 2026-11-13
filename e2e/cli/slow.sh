# e2e/cli/slow.sh: a run is never silently slow (cl-913), on this computer's machine with no GPU. The machine reads recorded GPU
# readings in place of NVML's (`machine.gpu_samples`: slow/hot-host.jsonl, rabou's two H100s as run 46678's driver record measured
# them, GPU 0 held back for heat 98% of its busy time at 89 °C), through the real path: a run that loads it 20 s warns
# `gpu_throttled` as it goes, `cozy machine show` and the page mark the machine red, and a run far past what its function took
# before warns `slow_run` naming the heat, on its record, live in `cozy run` and on its box (web/slow.js). Then the machine goes back
# to its own readings and is no longer marked.

# slow_config <config.toml> <samples, or empty>: config.toml with `machine.gpu_samples` set, or taken out.
slow_config() {
  python3 - "$1" "$2" <<'PY'
import re, sys
path, samples = sys.argv[1], sys.argv[2]
try:
    text = open(path).read()
except FileNotFoundError:
    text = ""
text = re.sub(r"(?m)^gpu_samples = .*\n", "", text)
if samples:
    line = f'gpu_samples = "{samples}"\n'
    text = re.sub(r"(?m)^\[machine\]\n", lambda m: m.group(0) + line, text, count=1) if re.search(r"(?m)^\[machine\]$", text) else text.rstrip("\n") + f"\n\n[machine]\n{line}"
open(path, "w").write(text)
PY
}

# slow_machine: the local machine restarted on what config.toml says now.
slow_machine() { machine_stop || check "slow: the local machine did not stop"; machine_ready; }

slow_runs() {
[ -n "$LOCAL" ] || return 0
local tool=$here/project/tool.py config=$HOME/.cozy/config.toml i n said
step "slow: three short runs of a slow still, its bar"
for i in 1 2 3; do cell "slow-bar-$i" "$tool/slow" 'seconds:=1' "instructions=bar $i" || return 0; done
for i in 1 2 3; do
  J '[.warnings[]?.code] | index("slow_run") | not' "$OUT/slow-bar-$i.json" || check "slow: bar run $i warned slow: $(jq -c .warnings "$OUT/slow-bar-$i.json")"
done

step "slow: the machine on a hot host's recorded readings"
slow_config "$config" "$(web_path "$here/slow/hot-host.jsonl")"
slow_machine

step "slow: a 45 s run there warns as it goes"
cell slow-hot "$tool/slow" 'seconds:=45' 'instructions=hot' || return 0
n=$(cat "$OUT/slow-hot.number")
J '[.warnings[]?.code] | index("gpu_throttled") and index("slow_run")' "$OUT/slow-hot.json" \
  || check "slow: run $n's record has no gpu_throttled and slow_run warnings: $(jq -c .warnings "$OUT/slow-hot.json")"
said=$(jq -r '.warnings[]? | select(.code == "slow_run") | .message' "$OUT/slow-hot.json")
grep -q 'throttled for heat' <<< "$said" || check "slow: run $n's slow_run names no heat: $said"
grep -qE "run [A-Z][A-Z0-9]*-$n has run" <<< "$said" || check "slow: run $n's slow_run was not said while it ran: $said"
# Said as it went: an in-progress event of its own already carries both.
grep '^{' "$OUT/slow-hot.ndjson" | jq -en 'any(inputs; .run.status? == "in_progress" and ([.run.warnings[]?.code] | index("slow_run") and index("gpu_throttled")))' > /dev/null \
  || check "slow: run $n's events never carried its warnings while it ran"
grep -q 'GPU 0 ran throttled for heat 98% of this run' "$OUT/slow-hot.ndjson" || check "slow: run $n's log never said GPU 0's heat"
cozy run watch "$n" --once > "$OUT/slow-hot.txt" 2>&1 || true
grep -qE "warning: run [A-Z][A-Z0-9]*-$n has run" "$OUT/slow-hot.txt" || check "slow: cozy run watch $n does not say it was slow: $(tail -c 400 "$OUT/slow-hot.txt")"

step "slow: the machine marked, in cozy machine show"
cozy --json machine show local > "$OUT/slow-machine.json" 2>&1 || check "slow: machine show local failed"
J '.status.heat[]? | select(.gpu == 0 and .share > 0.9 and .replayed == true and .temperature_max_c == 89)' "$OUT/slow-machine.json" \
  || check "slow: machine show local has no hot GPU 0: $(jq -c '.status.heat' "$OUT/slow-machine.json")"
J '[.status.heat[]?.gpu] | index(1) | not' "$OUT/slow-machine.json" || check "slow: GPU 1, capped for power only, is marked hot"
cozy machine show local > "$OUT/slow-machine.txt" 2>&1 || true
grep -qE '^slow +GPU 0 ran throttled for heat 98% of its last run \(89 °C, [0-9]+ of 1755 MHz\)' "$OUT/slow-machine.txt" \
  || check "slow: machine show local does not say it is slow: $(head -c 600 "$OUT/slow-machine.txt")"
grep -qE '^state +ready · slow' "$OUT/slow-machine.txt" || check "slow: machine show local's state is not marked slow"

slow_web

step "slow: back on its own readings, the machine is no longer marked"
slow_config "$config" ''
slow_machine
cozy --json machine show local > "$OUT/slow-machine-after.json" 2>&1 || true
J '(.status.heat // []) | length == 0' "$OUT/slow-machine-after.json" || check "slow: the machine is still marked hot: $(jq -c .status.heat "$OUT/slow-machine-after.json")"
}

# The page while the machine is hot: its chip red top right, its flyout and drawer saying why, and a box's run there saying it on the box.
slow_web() {
local pw=$OUT/playwright chrome
chrome=$(web_chrome)
[ -n "$chrome" ] && [ -x "$chrome" ] || { check "slow-web: no Google Chrome (${chrome:-none})"; return; }
npm install -s --prefix "$(web_path "$pw")" playwright-core@1.55.0 > "$OUT/web-npm.log" 2>&1 || { check "slow-web: npm install playwright-core failed: $(tail -c 300 "$OUT/web-npm.log")"; return; }
step "web-slow: the hot machine red on the page, and a box's slow run said on the box"
# Its box runs in the project whose history holds the bar: run history is the project's own.
SLOW_PROJECT=$(web_path "$RUN_PROJECT"); export SLOW_PROJECT
web_proof slow "$chrome" "$pw" ''
}
