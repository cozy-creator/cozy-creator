# h3-baked-room: H3 Turbo (fl2va_turbo, 15 s / 362 frames / 8 steps / audio, int8) on one rental under named Runtimes in
# call order (an update between Runtimes, so A → B → A matches B's executor with a second A one), with host-memory
# probes between calls: run times, resident bytes, DiT fills by origin (store or kept baked bytes), Turbo LoRA factor
# reads, the cache's pages and room, the container's cgroups and RSS, and frames from every video.
# Proof-only (rc/baked-room-proof-20261027). params:
#   runtimes   {"<name>": "<URL#sha256=…>", …}; tensorfs "<URL#sha256=…>" (goes on with the first; tensord: this cozy)
#   calls      [{"label", "runtime", "prompt", "seed", "lora_mode"?, "gpu_memory_limit"? ("24GiB"; default off)} |
#               {"label", "probe": true}], in order; a new runtime or limit is a machine update
#   traffic_gb GB the provider bills (90); need_s seconds the cap must buy (3600)
. "$(dirname "$0")/lib.sh"
H3=$PACKAGES/pk/minimax-h3
[ -f "$H3/package.toml" ] || [ -f "$H3/pyproject.toml" ] || fail "no packages checkout at pk (packages_refs pk=<commit>)"
command -v ffmpeg >/dev/null || sudo apt-get install -y -qq ffmpeg >/dev/null 2>&1 || fail "no ffmpeg"
fetch() {
  local f=$OUT/wheels/$(basename "${1%%#*}" | sed 's/%2B/+/g')
  [[ $1 == https://*#sha256=* ]] || { echo "$1 is not https://…#sha256=" >&2; return 1; }
  mkdir -p "$OUT/wheels"
  [ -f "$f" ] || curl -fsSL --retry 5 --retry-all-errors "${1%%#*}" -o "$f" || { echo "cannot fetch ${1%%#*}" >&2; return 1; }
  [ "$(sha256sum "$f" | cut -d' ' -f1)" = "${1##*#sha256=}" ] || { echo "$f does not hash to ${1##*#sha256=}" >&2; return 1; }
  echo "$f"
}
version() { python3 -I -c 'import sys, zipfile; z = zipfile.ZipFile(sys.argv[1]); m = next(n for n in z.namelist() if n.endswith(".dist-info/METADATA")); print(next(l.split(": ", 1)[1] for l in z.read(m).decode().splitlines() if l.startswith("Version: ")))' "$1"; }
declare -A WHL VER
for rt in $(jq -r '.runtimes | keys[]' <<<"$PARAMS"); do
  WHL[$rt]=$(fetch "$(jq -r --arg r "$rt" '.runtimes[$r]' <<<"$PARAMS")") || fail "runtime $rt"
  VER[$rt]=$(version "${WHL[$rt]}"); say "runtime $rt: ${VER[$rt]}"
done
TFS=$(fetch "$(param tensorfs)") || fail "tensorfs"
say "tensorfs $(version "$TFS"); tensord $(cozy --version)"
CALLS=$(param calls '[]')
mapfile -t labels < <(jq -r '.[].label' <<<"$CALLS")
[ "${#labels[@]}" -gt 0 ] || fail "no calls"
mkdir -p "$OUT/inputs" "$OUT/probe" "$OUT/frames"
cp "$(dirname "$0")/baked-host.py" "$OUT/probe/baked-host.py"
for i in "${!labels[@]}"; do
  c=$(jq -c ".[$i]" <<<"$CALLS")
  if jq -e .probe <<<"$c" >/dev/null; then say "call ${labels[i]}: host probe"; continue; fi
  rt=$(jq -r .runtime <<<"$c"); [ -n "${WHL[$rt]:-}" ] || fail "${labels[i]}: no runtime $rt"
  jq -c '{prompt, seed, duration_s: 15} + (if .lora_mode then {lora_mode} else {} end)' <<<"$c" > "$OUT/inputs/${labels[i]}.json"
  say "call ${labels[i]} (runtime $rt$(jq -r '(if .lora_mode then ", lora_mode " + .lora_mode else "" end) + (if .gpu_memory_limit then ", GPU memory " + .gpu_memory_limit else "" end)' <<<"$c")): seed $(jq -r .seed <<<"$c")"
done
for m in minimax-h3:1.0.0-rc.5:int8 minimax-h3-turbo-lora:1.0.1:pdd8; do
  IFS=: read -r repo rel lane <<<"$m"
  cozy --json model info "$repo" > "$OUT/model-$repo.json" 2>&1 || fail "model info $repo"
  jq -e --arg r "$rel" --arg l "$lane" '.releases[] | select(.version==$r) | .lanes[] | select(.lane==$l)' "$OUT/model-$repo.json" >/dev/null ||
    fail "$repo@$rel has no $lane lane"
done
printf 'run 5 · sync local/x (source changed)\nrun 6 · local/x/fl2va_turbo\nrun      6\n' > "$OUT/probe-cell.txt"
[ "$(runno probe-cell)" = 6 ] || fail "parsers"
# summary <label>: execution, steps, residency, DiT fills by origin, factor reads and the cache, from the run record.
summary() {
  python3 -I - "$OUT/$1.json" <<'PY'
import collections, json, sys
run = json.load(open(sys.argv[1]))
out = {"run": run.get("number"), "status": run.get("status"), "execution_s": round((run.get("execution_ms") or 0) / 1000, 2)}
for c in [run] + (run.get("calls") or []):
    m = c.get("measurements") or {}
    steps = ((m.get("attribution") or {}).get("steps") or {}).get("denoise") or {}
    if steps.get("series"):
        out["step_s"] = [round(v / 1000, 1) for _, v in steps["series"]]
    for d in (m.get("memory") or {}).get("driver") or []:
        out["driver_gb"] = [round(d.get("before_bytes", 0) / 1e9, 1), round(d.get("peak_bytes", 0) / 1e9, 1)]
    for g in ((m.get("memory") or {}).get("stats") or {}).get("varena") or []:
        st = g.get("stats") or {}
        dit, lora = collections.Counter(), collections.Counter()
        for t in st.get("trace", []):
            comp = t["set"].rsplit("/", 1)[-1].split("#")[0]
            if comp in ("transformer", "transformer_ref") and "minimax-h3@" in t["set"]:
                dit[t["origin"]] += 1
            elif comp in ("fl2va_turbo", "ref2va_turbo"):
                lora[t["origin"]] += 1
        if dit or lora:
            wr = st.get("weight_residency") or {}
            out.setdefault("gpus", []).append({"dit_fills": dict(dit), "factor_reads": dict(lora),
                "resident_gb": round((wr.get("resident_bytes") or 0) / 1e9, 1),
                "baked_pages": st.get("baked_pages"), "moved": st.get("moved")})
print(json.dumps(out))
PY
}
# host <label>: the probe's tightest-limited cgroup (or the root), MemAvailable and the top RSS.
host() {
  jq -r '[.. | strings | select(contains("host_probe"))][0] | fromjson |
    ([.cgroups[] | select((.["memory.max"] // .["memory.limit_in_bytes"] // "max") | type == "number")] | min_by(.["memory.max"] // .["memory.limit_in_bytes"])) as $c |
    "cgroup " + (if $c then ($c.path + " limit " + ((($c["memory.max"] // $c["memory.limit_in_bytes"]) / 1e9 * 10 | round) / 10 | tostring) + " GB, current " + ((($c["memory.current"] // $c["memory.usage_in_bytes"]) / 1e9 * 10 | round) / 10 | tostring) + " GB, peak " + ((($c["memory.peak"] // $c["memory.max_usage_in_bytes"] // 0) / 1e9 * 10 | round) / 10 | tostring) + " GB") else "none limited" end) +
    "; MemTotal " + ((.meminfo.MemTotal / 1e9 * 10 | round) / 10 | tostring) + " GB, MemAvailable " + ((.meminfo.MemAvailable / 1e9 * 10 | round) / 10 | tostring) +
    " GB; RSS total " + ((.rss_total / 1e9 * 10 | round) / 10 | tostring) + " GB, top " + ([.top[0:3][] | ((.rss / 1e9 * 10 | round) / 10 | tostring) + " GB " + (.cmd | .[0:60])] | join(" | "))' "$OUT/$1.json" 2>&1 | head -c 1500
}
quote "$(param traffic_gb 90)" "$(param need_s 3600)"
rent

cur=''
for i in "${!labels[@]}"; do
  k=${labels[i]} c=$(jq -c ".[$i]" <<<"$CALLS")
  if jq -e .probe <<<"$c" >/dev/null; then
    step "$k" run "$OUT/probe/baked-host.py" --on "$NAME" || fail "$k: host probe"
    n=$(runno "$k"); [ -n "$n" ] || fail "$k: no run number"
    cozy run show "$n" --once --json > "$OUT/$k.json" 2>/dev/null
    say "$k host: $(host "$k")"
    continue
  fi
  rt=$(jq -r .runtime <<<"$c") lim=$(jq -r '.gpu_memory_limit // "off"' <<<"$c")
  if [ "$rt/$lim" != "$cur" ]; then
    if [ -z "$cur" ]; then
      step "update-$k" --json machine update "$NAME" --runtime "${WHL[$rt]}" --tensorfs "$TFS" --tensord "$(command -v cozy)" --gpu-memory-limit "$lim" -y || fail "$rt did not go on"
    else
      step "update-$k" --json machine update "$NAME" --runtime "${WHL[$rt]}" --gpu-memory-limit "$lim" -y || fail "$rt ($lim) did not go on"
    fi
    cozy machine show "$NAME" > "$OUT/machine-$k.txt" 2>&1; grep -E '^(version|runtime|tensorfs) ' "$OUT/machine-$k.txt" | tee -a "$R"
    grep -E '^runtime ' "$OUT/machine-$k.txt" | grep -qF "${VER[$rt]%%+*}" || fail "$NAME does not run ${VER[$rt]}"
    say "$k: runtime ${VER[$rt]}, GPU memory limit $lim"
    if [ -z "$cur" ]; then
      step install package install "$H3" --on "$NAME" || fail "minimax-h3 did not install"
      step download-base model download _/minimax-h3@1.0.0-rc.5 --lane int8 --on "$NAME" || fail "base download"
      step download-turbo model download _/minimax-h3-turbo-lora@1.0.1 --on "$NAME" || fail "turbo download"
    fi
    cur=$rt/$lim
  fi
  step "$k" run "$H3/fl2va_turbo" --input "$OUT/inputs/$k.json" --lane int8 --gpus=1 --on "$NAME" --out "$OUT/media/$k"
  n=$(runno "$k")
  [ -n "$n" ] || fail "$k: no run number"
  cozy run show "$n" --once --json > "$OUT/$k.json" 2>/dev/null; cozy run show "$n" --once > "$OUT/$k.show.txt" 2>&1
  st=$(jq -r '.status // "none"' "$OUT/$k.json")
  say "$k ($rt ${VER[$rt]}) run $n $st: $(summary "$k" | cut -c1-1800)"
  [ "$st" = completed ] || { cozy machine logs "$NAME" -n 300 > "$OUT/$k.tensord.log" 2>&1; fail "$k: $(jq -r '.reason.message // ""' "$OUT/$k.json" | cut -c1-600)"; }
  v=$(find "$OUT/media/$k" -name '*.mp4' | head -1)
  [ -s "$v" ] || fail "$k: no video"
  for s in 1 7 14; do ffmpeg -nostdin -v error -ss "$s" -i "$v" -frames:v 1 "$OUT/frames/$k-${s}s.png" || true; done
  say "$k video $(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames,width,height -of csv=p=0 "$v") $(ffprobe -v error -select_streams a:0 -show_entries stream=codec_name -of csv=p=0 "$v")"
done
cozy machine logs "$NAME" -n 400 > "$OUT/tensord.log" 2>&1
