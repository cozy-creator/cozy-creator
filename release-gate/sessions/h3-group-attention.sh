# h3-group-attention: H3 Turbo (fl2va_turbo, 15 s / 362 frames / 8 steps / audio, int8) on one multi-GPU rental, first
# on the Runtime `control`, then on `candidate`: the attention route every rank served, the V-clip probe's per-call
# clip share (a tree whose producer reports `vclip=` in its served route), step times, and videos against a reference
# cell (PSNR, SSIM). The rental ends before the comparisons. Proof-only (rc/h3-group-attention-proof-20261027). params:
#   gpus       GPUs in the rental and per call (4)
#   control    {"runtime": "<version | URL#sha256=…>", "tensorfs": "<version | URL#sha256=…>"}: put on first unless running
#   candidate  {"runtime": "<URL#sha256=…>"}
#   extra      {"runtime": "<URL#sha256=…>"} (optional): a third arm's Runtime, put on for cells of phase "extra"
#   cells      [{"label", "phase": "control"|"candidate", "tree" (a packages_refs name), "prompt", "seed",
#              "attention" ("" or a --attention-kernel pin)}], in order
#   max_fails  failed cells that stop the session (2); a failed cell's machine logs are kept either way
#   compare    [["reference", "cell"], …]
#   traffic_gb GB the provider bills (120); need_s seconds the cap must buy (3000)
. "$(dirname "$0")/lib.sh"
command -v ffmpeg >/dev/null || sudo apt-get install -y -qq ffmpeg >/dev/null 2>&1 || fail "no ffmpeg"
fetch() {
  [[ $1 == https://*#sha256=* ]] || { echo "$1"; return 0; }  # a version: the pod resolves it
  local f=$OUT/wheels/$(basename "${1%%#*}" | sed 's/%2B/+/g')
  mkdir -p "$OUT/wheels"
  [ -f "$f" ] || curl -fsSL --retry 5 --retry-all-errors "${1%%#*}" -o "$f" || { echo "cannot fetch ${1%%#*}" >&2; return 1; }
  [ "$(sha256sum "$f" | cut -d' ' -f1)" = "${1##*#sha256=}" ] || { echo "$f does not hash to ${1##*#sha256=}" >&2; return 1; }
  echo "$f"
}
version() {
  [ -f "$1" ] || { echo "$1"; return; }
  python3 -I -c 'import sys, zipfile; z = zipfile.ZipFile(sys.argv[1]); m = next(n for n in z.namelist() if n.endswith(".dist-info/METADATA")); print(next(l.split(": ", 1)[1] for l in z.read(m).decode().splitlines() if l.startswith("Version: ")))' "$1"
}
CR=$(fetch "$(jq -r .control.runtime <<<"$PARAMS")") || fail "control runtime"
CT=$(fetch "$(jq -r .control.tensorfs <<<"$PARAMS")") || fail "control tensorfs"
DR=$(fetch "$(jq -r .candidate.runtime <<<"$PARAMS")") || fail "candidate runtime"
CRV=$(version "$CR") CTV=$(version "$CT") DRV=$(version "$DR") ER='' ERV=''
if jq -e '.extra.runtime' <<<"$PARAMS" >/dev/null; then
  ER=$(fetch "$(jq -r .extra.runtime <<<"$PARAMS")") || fail "extra runtime"; ERV=$(version "$ER"); say "extra runtime $ERV"
fi
say "control runtime $CRV, tensorfs $CTV; candidate runtime $DRV; $GPUS GPUs; cozy $(cozy --version)"
[ "$CRV" != "$DRV" ] || fail "control and candidate are the same Runtime"
CELLS=$(param cells '[]') COMPARE=$(param compare '[]')
mapfile -t labels < <(jq -r '.[].label' <<<"$CELLS")
[ "${#labels[@]}" -gt 0 ] || fail "no cells"
mkdir -p "$OUT/inputs"
for i in "${!labels[@]}"; do
  k=${labels[i]} tree=$(jq -r ".[$i].tree" <<<"$CELLS")
  [[ $k =~ ^[A-Za-z0-9_-]+$ ]] && [ "$(jq --arg k "$k" 'map(select(.label==$k)) | length' <<<"$CELLS")" = 1 ] || fail "labels must be unique words: $k"
  [ -f "$PACKAGES/$tree/minimax-h3/package.toml" ] || fail "cell $k: no packages checkout $tree"
  jq -c ".[$i] | {prompt, seed, duration_s: 15}" <<<"$CELLS" > "$OUT/inputs/$k.json"
  say "cell $k ($(jq -r ".[$i].phase" <<<"$CELLS"), $tree $(git -C "$PACKAGES/$tree" rev-parse --short HEAD), pin '$(jq -r ".[$i].attention // \"\"" <<<"$CELLS")'): seed $(jq -r ".[$i].seed" <<<"$CELLS")"
done
jq -e --argjson n "$(jq -c 'map(.label)' <<<"$CELLS")" 'all(.[]; length == 2 and all(.[]; . as $x | $n | index($x)))' <<<"$COMPARE" >/dev/null ||
  fail "compare names cells that are not there"
for m in minimax-h3:1.0.0-rc.5:int8 minimax-h3-turbo-lora:1.0.1:pdd8; do
  IFS=: read -r repo rel lane <<<"$m"
  cozy --json model info "$repo" > "$OUT/model-$repo.json" 2>&1 || fail "model info $repo"
  jq -e --arg r "$rel" --arg l "$lane" '.releases[] | select(.version==$r) | .lanes[] | select(.lane==$l)' "$OUT/model-$repo.json" >/dev/null ||
    fail "$repo@$rel has no $lane lane"
done
printf 'run 5 · sync local/x (source changed)\nrun 6 · local/x/fl2va_turbo\nrun      6\n' > "$OUT/probe-cell.txt"
[ "$(runno probe-cell)" = 6 ] || fail "parsers"
# summary <label>: execution, and per GPU call its degree, step times, peak VRAM and every rank's attention route.
summary() {
  python3 -I - "$OUT/$1.json" <<'PY'
import json, re, sys
run = json.load(open(sys.argv[1]))
out = {"run": run.get("number"), "status": run.get("status"), "execution_s": round((run.get("execution_ms") or 0) / 1000, 2), "calls": []}
for c in [run] + (run.get("calls") or []):
    m = c.get("measurements") or {}
    ex = m.get("execution") or {}
    steps = ((m.get("attribution") or {}).get("steps") or {}).get("denoise") or {}
    if m.get("software"):
        out["software_runtime"] = m["software"].get("runtime")
    if not ex.get("gpus") or not steps.get("series"):
        continue
    ranks = []
    for g in ex["gpus"]:
        a = g.get("attention") or {}
        impl = a.get("impl") or ""
        ranks.append({"gpu": g.get("gpu"), "impl": impl, "vclip": re.findall(r"vclip=([^),]+)\)(?::(\d+))?", impl),
                      "failed": {k["kernel"]: (k.get("detail") or "")[:160] for k in a.get("kernels", []) if k.get("state") == "failed"}})
    out["calls"].append({"function": c.get("function"), "degree": ex.get("degree"),
        "step_s": [round(v / 1000, 1) for _, v in steps["series"]],
        "stages_s": {k: round(v["total_ms"] / 1000, 1) for k, v in ((m.get("attribution") or {}).get("stages") or {}).items()},
        "peak_GB": [round(d.get("peak_bytes", 0) / 1e9, 1) for d in (m.get("memory") or {}).get("driver") or []],
        "selected": sorted({f'{o["fields"].get("component")}={o["value"]}' for o in m.get("observations", []) if o["name"] == "attention.selected"}),
        "gemm": sorted({f'{o["fields"].get("component")}={o["value"]}' for o in m.get("observations", []) if o["name"] == "gemm.selected"}),
        "ranks": ranks})
print(json.dumps(out))
PY
}
quote "$(param traffic_gb 120)" "$(param need_s 3000)"
rent

on_runtime() { cozy machine show "$NAME" > "$OUT/machine-$2.txt" 2>&1; grep -E '^(version|runtime|tensorfs) ' "$OUT/machine-$2.txt" | tee -a "$R"
  grep -E '^runtime ' "$OUT/machine-$2.txt" | grep -qF "$1"; }
if ! on_runtime "$CRV" boot || ! grep -E '^tensorfs ' "$OUT/machine-boot.txt" | grep -qF "$CTV"; then
  step control-update --json machine update "$NAME" --runtime "$CR" --tensorfs "$CT" -y || fail "control did not go on"
  on_runtime "$CRV" control || fail "$NAME does not run Runtime $CRV"
fi
step install package install "$PACKAGES/$(jq -r '.[0].tree' <<<"$CELLS")/minimax-h3" --on "$NAME" || fail "minimax-h3 did not install"
step download-base model download _/minimax-h3@1.0.0-rc.5 --lane int8 --on "$NAME" || fail "base download"
step download-turbo model download _/minimax-h3-turbo-lora@1.0.1 --on "$NAME" || fail "turbo download"
phase=control fails=0
printf 'label\tphase\trun\tstatus\texecution_s\treason\n' > "$OUT/outcomes.tsv"
for i in "${!labels[@]}"; do
  k=${labels[i]} p=$(jq -r ".[$i].phase" <<<"$CELLS") tree=$(jq -r ".[$i].tree" <<<"$CELLS") pin=$(jq -r ".[$i].attention // \"\"" <<<"$CELLS")
  if [ "$p" != "$phase" ]; then
    case $p in candidate) W=$DR V=$DRV ;; extra) W=$ER V=$ERV ;; *) fail "$k: phase $p after $phase" ;; esac
    [ -n "$W" ] || fail "$k: no $p runtime"
    step "$p-update" --json machine update "$NAME" --runtime "$W" -y || fail "$p did not go on"
    on_runtime "${V%%+*}" "$p" || fail "$NAME does not run the $p base version"
    phase=$p
  fi
  args=(run "$PACKAGES/$tree/minimax-h3/fl2va_turbo" --input "$OUT/inputs/$k.json" --lane int8 --gpus="$GPUS" --on "$NAME" --out "$OUT/media/$k")
  [ -z "$pin" ] || args+=(--attention-kernel "$pin")
  step "$k" "${args[@]}"
  n=$(runno "$k")
  [ -n "$n" ] || fail "$k: no run number"
  cozy run show "$n" --once --json > "$OUT/$k.json" 2>/dev/null; cozy run show "$n" --once > "$OUT/$k.show.txt" 2>&1
  st=$(jq -r '.status // "none"' "$OUT/$k.json")
  summary "$k" > "$OUT/$k.summary.json"
  say "$k ($p) run $n $st: $(cut -c1-1800 "$OUT/$k.summary.json")"
  rt=$(jq -r '.software_runtime // ""' "$OUT/$k.summary.json")
  if [ -z "$rt" ]; then :  # a failed call may carry no measurements
  elif [ "$p" = extra ]; then [[ $rt == "$ERV"* ]] || fail "$k ran Runtime '$rt', not the extra $ERV"
  elif [ "$p" = candidate ]; then [[ $rt == "$DRV"* ]] || fail "$k ran Runtime '$rt', not the candidate $DRV"
  else [[ $rt == "$CRV"* && $rt != "$DRV"* ]] || fail "$k ran Runtime '$rt', not the control $CRV"; fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$k" "$p" "$n" "$st" "$(jq -r '(.execution_ms // 0) / 1000' "$OUT/$k.json")" \
    "$(jq -r '.reason.message // "" | gsub("[\t\n]"; " ") | .[0:300]' "$OUT/$k.json")" >> "$OUT/outcomes.tsv"
  say "$k gemm $(jq -c '[.calls[].gemm] | add // []' "$OUT/$k.summary.json"); 'bits differ' in its record: $(grep -o 'bits differ[^"]*' "$OUT/$k.json" | sort | uniq -c | head -5 | tr '\n' ';')"
  if [ "$p" = extra ]; then
    cozy machine logs "$NAME" -n 3000 > "$OUT/$k.machine.log" 2>&1
    say "$k machine log: $(grep -ciE 'bits differ' "$OUT/$k.machine.log") 'bits differ' lines; $(grep -iE 'verdict|probe' "$OUT/$k.machine.log" | tail -3 | cut -c1-300 | tr '\n' ';')"
  fi
  if [ "$st" != completed ]; then
    cozy machine logs "$NAME" -n 300 > "$OUT/$k.tensord.log" 2>&1
    say "$k failed: $(jq -r '.reason.message // ""' "$OUT/$k.json" | cut -c1-600)"
    fails=$((fails + 1)); [ "$fails" -lt "$(param max_fails 2)" ] || fail "$fails cells failed"
  fi
done

# The rental ends here: the comparisons run on the runner, off the pod's clock.
until gone "$RID"; do cozy machine end "$RID" -y >> "$OUT/end.txt" 2>&1 || cozy machine end "$RID" -y --force >> "$OUT/end.txt" 2>&1; sleep 10; done
say "GONE $RID ($NAME) before the comparisons"; RID=''
video() { find "$OUT/media/$1" -name '*.mp4' | head -1; }
while read -r a b; do
  va=$(video "$a") vb=$(video "$b")
  { [ -s "$va" ] && [ -s "$vb" ]; } || { say "$a vs $b: missing video"; continue; }
  psnr=$(ffmpeg -nostdin -threads 2 -i "$va" -i "$vb" -lavfi '[0:v][1:v]psnr' -f null - 2>&1 | grep -oE 'average:[0-9.inf]+' | tail -1)
  ssim=$(ffmpeg -nostdin -threads 2 -i "$va" -i "$vb" -lavfi '[0:v][1:v]ssim' -f null - 2>&1 | grep -oE 'All:[0-9.]+ \([0-9.inf]+\)' | tail -1)
  say "$a vs $b: PSNR $psnr, SSIM $ssim"
done < <(jq -r '.[] | "\(.[0]) \(.[1])"' <<<"$COMPARE")
for k in "${labels[@]}"; do
  v=$(video "$k"); [ -s "$v" ] || continue
  say "$k video $(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames,width,height -of csv=p=0 "$v") audio $(ffprobe -v error -select_streams a:0 -show_entries stream=codec_name,channels -of csv=p=0 "$v")"
done
