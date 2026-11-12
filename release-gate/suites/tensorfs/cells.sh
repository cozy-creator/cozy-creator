# Suite `tensorfs` (gate.sh --suite tensorfs): TensorFS moves weights as fast as before and dedups.
#   straggler        on this host: `tfs download` of BiRefNet clean and through straggler.py, which drips its first
#                    objects as a slow storage GET does (10-01); then with three cold objects, each ask of which sends
#                    2 MiB and drips the rest (tfs-138: one 64 MiB chunk dripped for 5-8.5 min, runs 20867/20878); then
#                    with three crawling objects, each ask of which sends 64 KiB and drips, as R2's coldest do (tfs-141:
#                    15-60 min per chunk read whole, gate 41623354412); then with sixteen, whose 16 MiB ranges come at
#                    full speed: many write at once, and no download may hang behind them (tfs-143: gate 41823522538's
#                    stopped for good at 1207 of 11847 MiB). Each lands within a fixed bar of
#                    the clean download, whatever the range size (check.py). Then through one 5 MB/s link shared by every
#                    request, as a slow pod's (straggler-link; run 31103: the old downloader judged every range slow, restarted
#                    72 objects and moved nothing for good with 78 sockets open): it must land; 3 min with no byte into
#                    its store fails it. Then with every object's partial held by another process, as a cancelled fetch's
#                    commit holds one while the next run claims it (straggler-held; gate 41991226171: the fetch fell back
#                    to a temp it could not read back): it must land. Then with one small ask's socket held silent
#                    after half its body (straggler-silent; run 32321 sat 16 min on its last two chunks): it must land
#                    within 60 s, asked again. Then with one small ask dripping 16 KiB every 25 s after half its body, under
#                    the 30 s stall (straggler-drip; run 47820's last ~1 MB crawled 15 min, tfs-149): a hedge on another
#                    connection must land it within 20 s of the clean download. Then new random weights from origin.py through a 40 MB/s link, every
#                    storage ask answered 503 for 8 s once the ramp has settled (straggler-burst; R2 sent 13 near run
#                    32321's end, which left one request held by nothing to start): it must land
#   lora-alpha       on this host: `tfs import --converter h3.lora` of an H3 LoRA in ComfyUI's spelling with no `.alpha`
#                    tensors whose kohya metadata says ss_network_alpha 420 at rank 4: its committed `lora_alpha` must be r,
#                    a scale of 1, as kohya's and ComfyUI's loaders apply it (10-29: the Mystic XXX LoRAs, this format,
#                    rendered colour noise at 420/16)
# On the rented machine $M, through `cozy`:
#   probe            the machine's disk, written and read cold; an executor never dials, so no network figure
#   download-cold    a large many-object model ($TFS_MODEL), its time held against the last pass
#   download-again   the same model again: nothing comes from the Hub
#   download-cut/-resume  new random weights ($TFS_RESUME_MB MiB), uploaded private from this computer's machine, cancelled
#                    part-way on $M, then downloaded again: only the rest comes, from the Hub. No datacenter volume
#                    holds them (TensorFS writes back public objects only), so a cache never serves the resume (cl-686)
#   kill-cut         a rental's only: the same with TensorD killed in place of the cancel, as a crash ends it (kill-at.sh).
#                    Its stable parent starts it again and the same run goes on, so that run's record is the resume:
#                    what landed is still there, unfinished chunks too
#   upload           new random weights (a CozyTensors folder) to a throwaway <account>/ci-tfs-<R>
#   upload-again     the same folder to a second tag: no chunk is sent again (dedup)
#   upload-cut/-resume    new weights cancelled part-way, then uploaded again: only the rest is sent
#   mc-source/-upload/-quantize/-inspect  multi-conversion/: a recipe that encodes each component in its own call into
#                    one output (H3's) writes every component; its lane's commit read back through `cozy run`
#   family-refusal   an SDXL inpainting single file (sdxl.single_file's keys, conv_in at 9 channels) is refused from
#                    its header: FAMILY_MISMATCH, with only the header and the reference's configs downloaded
#   layout-probe     Comfy-Org's ComfyUI H3 Ref2VA DiT (contiguous Q/K/V) read as h3.native: the layout probe refuses it
#                    (LAYOUT_MISMATCH) after the header and the probed rows. That layout read right passes the probe in
#                    the h3-comfy-layout cell's import.
# check.py holds each against the SKU's last passed suite and records a PASS. The throwaway models are deleted at the end.
TFS_MODEL=${TFS_MODEL:-qwen/qwen3.5-9b} TFS_RESUME_MB=${TFS_RESUME_MB:-2048} TFS_UPLOAD_MB=${TFS_UPLOAD_MB:-256}
T=$OUT/suite-tensorfs TFS_DESTS=()
mkdir -p "$T"; : > "$T/cases"
# tfs_step <name> <cozy args...>: one command that makes a run, followed to its end (e2e/run.sh): its events in
# <name>.ndjson, its record in <name>.json, its wall time in steps.tsv.
tfs_step() {
  local name=$1 t0; shift
  t0=$(date +%s%3N)
  run_submit "$T" "$name" "$@" > /dev/null; run_wait "$T" "$name"
  printf '%s\t%s\t%s\n' "$name" "$(cat "$T/$name.number")" "$(( $(date +%s%3N) - t0 ))" >> "$T/steps.tsv"
  log "tensorfs $name: run $(cat "$T/$name.number") $(run_get "$T" "$name" .status)"
}
# tfs_cut <name> <stage> <fraction> <cozy args...>: the command, cancelled once its <stage> progress passes the
# fraction; the bytes it had moved by then go in <name>.cut.
tfs_cut() {
  local name=$1 stage=$2 frac=$3 n moved=; shift 3
  n=$(run_submit "$T" "$name" "$@")
  while [ -n "$n" ] && kill -0 "$(cat "$T/$name.pid")" 2>/dev/null; do
    moved=$(python3 -I "$HERE/suites/tensorfs/check.py" --past "$T/$name.ndjson" "$stage" "$frac")
    if [ -n "$moved" ]; then "$CLI" run cancel "$n" -y >/dev/null 2>&1; echo "$moved" > "$T/$name.cut"; break; fi
    sleep 0.5
  done
  run_wait "$T" "$name"
  printf '%s\t%s\t\n' "$name" "${n:-none}" >> "$T/steps.tsv"
  log "tensorfs $name: run ${n:-none} cancelled at $(cat "$T/$name.cut" 2>/dev/null || echo '?') bytes"
}
# tfs_kill_cut <name> <bytes> <stage> <cozy args...>: the command, with kill-at.sh armed on $M to kill TensorD <bytes> into it. Its
# parent starts TensorD again and the same run goes on, followed here to its end; the bytes it had moved when the machine
# went go in <name>.cut. What the machine said across the kill stays in <name>.machine, should it not come back.
tfs_kill_cut() {
  local name=$1 bytes=$2 stage=$3 tail; shift 3
  "$CLI" machine ssh "$M" -- 'cat > /tmp/kill-at.sh' <"$HERE/suites/tensorfs/kill-at.sh" || return 1
  "$CLI" machine ssh "$M" -- "setsid sh /tmp/kill-at.sh $bytes >/tmp/kill-at.log 2>&1 </dev/null & echo \$! >/tmp/kill-at.pid" </dev/null || return 1
  "$CLI" machine ssh "$M" -- 'tail -f -s 0.1 ---disable-inotify -n 0 /var/lib/cozy/machine/logs/tensord.*.log' </dev/null >"$T/$name.machine" 2>&1 & tail=$!
  tfs_step "$name" "$@"
  kill "$tail" 2>/dev/null
  "$CLI" machine ssh "$M" -- 'kill $(cat /tmp/kill-at.pid) 2>/dev/null; cat /tmp/kill-at.log' </dev/null >>"$T/$name.machine" 2>&1
  python3 -I "$HERE/suites/tensorfs/check.py" --dropped "$T/$name.ndjson" "$stage" > "$T/$name.cut"
  log "tensorfs $name: TensorD killed at $(cat "$T/$name.cut") bytes"
}
# tfs_fixture <step> <dest>: new random weights uploaded private from this computer's machine; their commit in TFS_COMMIT.
tfs_fixture() {
  TFS_COMMIT=
  [ -n "$WHO" ] && tfs_random "$T/resume.safetensors" "$TFS_RESUME_MB" || return 1
  TFS_DESTS+=("$2")
  tfs_step "$1" model upload "$T/resume.safetensors" "$2" --lane f16 --source-format as-is --on local -y
  TFS_COMMIT=$(run_get "$T" "$1" "[.result.models[]?.published | select(.destination == \"$2\") | .commit][0]")
  rm -f "$T/resume.safetensors"
}
# tfs_random <file> <MiB>: new random F16 weights as one .safetensors file.
tfs_random() {
  rm -f "$1"
  uv run -q --no-project --with numpy --with safetensors python -I - "$1" "$2" <<'PY'
import sys
import numpy as np
from safetensors.numpy import save_file
rng = np.random.default_rng()
save_file({f"blocks.{i}.weight": rng.standard_normal((1024, 1024)).astype(np.float16) for i in range(int(sys.argv[2]) // 2)}, sys.argv[1])
PY
}
# tfs_weights <dir> <MiB>: new random weights as a CozyTensors folder (tfs import + export, the candidate TensorFS).
tfs_weights() {
  rm -rf "$1"
  tfs_random "$1.safetensors" "$2" || return 1
  TENSORFS_HOME=$T/tfs-store uv tool run -q --from "${TWF:-tensorfs}" tfs import "$1.safetensors" --converter as-is --name "local/$(basename "$1")" >/dev/null &&
    TENSORFS_HOME=$T/tfs-store uv tool run -q --from "${TWF:-tensorfs}" tfs export "local/$(basename "$1")" "$1" >/dev/null
}
# tfs_straggler <name> <stragglers> [<burst> [<min> [fast]]] [--link <bytes/s>]: `tfs download` of BiRefNet into a store of its
# own, through straggler.py.
tfs_straggler() {
  local port pid
  port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
  python3 -I "$HERE/suites/tensorfs/straggler.py" "$port" "${HUB_URL:-https://tensorhub.com}" "$2" 65536 "$T/$1.proxy" "${@:3}" &
  pid=$!
  tfs_fetch "$1" "$port" zhengpeng7/birefnet
  kill "$pid"; wait "$pid" 2>/dev/null || true  # the proxy ends on TERM
}
# tfs_burst <name>: `tfs download` of new random weights, 24 objects of 40 MiB (three ranges each), from origin.py through
# one 40 MB/s link, every storage ask answered 503 from 8 s to 16 s, after the ramp has settled: enough halvings to reach
# the floor.
tfs_burst() {
  local port pid tree
  rm -rf "$T/$1-origin"
  uv run -q --no-project --with numpy --with safetensors python -I - "$T/$1.safetensors" <<'PY' || return 1
import sys
import numpy as np
from safetensors.numpy import save_file
rng = np.random.default_rng()
save_file({f"blocks.{i}.weight": rng.standard_normal((20480, 1024)).astype(np.float16) for i in range(24)}, sys.argv[1])
PY
  tree=$(uv tool run -q --from "${TWF:-tensorfs}" tfs --store "$T/$1-origin" --json import "$T/$1.safetensors" --converter as-is \
    --name local/burst | python3 -c 'import json, sys; print(json.load(sys.stdin)["tree"]["sha256"])')
  rm -f "$T/$1.safetensors"
  port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
  python3 -I "$HERE/suites/tensorfs/origin.py" "$port" "$T/$1-origin" "$T/$1.proxy" 40000000 8 16 & pid=$!
  tfs_fetch "$1" "$port" "sha256:$tree"
  kill "$pid"; wait "$pid" 2>/dev/null || true
  rm -rf "$T/$1-origin"
}
# tfs_fetch <name> <port> <ref>: `tfs download` of <ref> from the server at <port> into a store of its own; its exit and time
# in straggler.tsv.
tfs_fetch() {
  local t0 tfs seen since now
  for _ in $(seq 50); do (exec 3<>"/dev/tcp/127.0.0.1/$2") 2>/dev/null && break; sleep 0.1; done
  t0=$(date +%s%3N)
  TENSORFS_HOME=$T/$1-store setsid uv tool run -q --from "${TWF:-tensorfs}" tfs --endpoint "http://127.0.0.1:$2" --json \
    download "$3" --dir "$T/$1-out" >"$T/$1.out" 2>"$T/$1.err" & tfs=$!
  # A hang is this case's failure, not the gate's: no byte into its store for 3 min ends it.
  seen= since=$SECONDS
  while kill -0 "$tfs" 2>/dev/null; do
    now=$(du -s --block-size=1 "$T/$1-store" 2>/dev/null | cut -f1)  # allocated bytes: a range written in place starts as a sparse file at full length
    [ "$now" = "$seen" ] || { seen=$now since=$SECONDS; }
    if (( SECONDS - since >= 180 )); then echo "no byte into its store for 3 min" >> "$T/$1.err"; kill -9 -- "-$tfs"; break; fi
    sleep 2
  done
  wait "$tfs"
  printf '%s\t%s\t%s\n' "$1" "$?" "$(( $(date +%s%3N) - t0 ))" >> "$T/straggler.tsv"
  rm -rf "$T/$1-store" "$T/$1-out"
  log "tensorfs $1: $(tail -n 1 "$T/straggler.tsv")"
}
# tfs_lora_alpha: the LoRA above, imported into a store of its own; its inspected record in lora-alpha.json.
tfs_lora_alpha() {
  local s=$T/lora-alpha
  rm -rf "$s"; mkdir -p "$s"
  uv run -q --no-project --with numpy --with safetensors python -I - "$s/lora.safetensors" <<'PY' || return 1
import sys
import numpy as np
from safetensors.numpy import save_file
rng = np.random.default_rng()
w = {}
for i in range(2):
    w[f"diffusion_model.blocks.{i}.attn.out_proj.lora_A.weight"] = rng.standard_normal((4, 7168)).astype(np.float32)
    w[f"diffusion_model.blocks.{i}.attn.out_proj.lora_B.weight"] = rng.standard_normal((5376, 4)).astype(np.float32)
save_file(w, sys.argv[1], metadata={"ss_network_alpha": "420", "ss_network_dim": "69"})
PY
  TENSORFS_HOME=$s/store uv tool run -q --from "${TWF:-tensorfs}" tfs import "$s/lora.safetensors" --name local/lora-alpha \
    --converter h3.lora >"$s/import.txt" 2>&1 &&
    TENSORFS_HOME=$s/store uv tool run -q --from "${TWF:-tensorfs}" tfs --json inspect local/lora-alpha >"$T/lora-alpha.json" \
      2>>"$s/import.txt"
  log "tensorfs lora-alpha: exit $? $(grep -o '"lora_alpha":[^,}]*' "$T/lora-alpha.json" 2>/dev/null | head -1)"
  rm -rf "$s/store"
}
tfs_cleanup() {
  local d
  for d in "${TFS_DESTS[@]}"; do
    "$CLI" model delete "$d" -y >"$T/delete.log" 2>&1 && log "tensorfs: deleted $d" ||
      grep -qE "model[._]not_found" "$T/delete.log" ||  # no upload landed: nothing to delete
      expect tensorfs-cleanup FAIL "cozy model delete $d: $(tail -c 300 "$T/delete.log")"
  done
  TFS_DESTS=()
}
trap 'tfs_cleanup; declare -F hub_cleanup >/dev/null && hub_cleanup; end_it' EXIT

# tfs_case <case>: whether --only selects it (none selects every case: probe, download, upload, family, multi-conversion,
# straggler, lora-alpha); a selected case is listed in cases, the ones check.py judges.
tfs_case() { [ -z "$ONLY" ] || [[ $ONLY == *",$1,"* ]] || return 1; echo "$1" >> "$T/cases"; }

if tfs_case probe; then
  ln -sfn "$HERE/suites/tensorfs/probe" "$OUT/ci-tfs-probe"
  echo '{}' > "$T/probe.yaml"
  tfs_step probe run ./ci-tfs-probe/measure --on "$M" --input "$T/probe.yaml"
fi
WHO=; for _ in 1 2 3; do  # a dropped connection is asked again
  WHO=$("$CLI" --json whoami 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["account"])' 2>/dev/null) && [ -n "$WHO" ] && break
  sleep 5
done
if tfs_case download; then
  tfs_step download-cold model install "$TFS_MODEL" --on "$M" -y
  tfs_step download-again model install "$TFS_MODEL" --on "$M" -y
  d=$WHO/ci-tfs-resume-$R
  tfs_fixture download-fixture "$d"
  if [ -n "${TFS_COMMIT:-}" ]; then
    tfs_cut download-cut download 0.3 model install "$d@$TFS_COMMIT" --on "$M" -y
    tfs_step download-resume model install "$d@$TFS_COMMIT" --on "$M" -y
  else
    expect tensorfs-download-resume FAIL "no private model to resume (${WHO:-not signed in}): $(run_get "$T" download-fixture .reason.message)"
  fi
  tfs_cleanup
fi
# A rental's only (gate.sh rents it with SSH): a machine that is someone's own never has its service killed.
if [ -n "${SSH:-}" ] && tfs_case kill; then
  d=$WHO/ci-tfs-kill-$R
  tfs_fixture kill-fixture "$d"
  if [ -n "${TFS_COMMIT:-}" ]; then
    tfs_kill_cut kill-cut $((TFS_RESUME_MB * 3 / 10 << 20)) download model install "$d@$TFS_COMMIT" --on "$M" -y
  else
    expect tensorfs-kill-cut FAIL "no private model to resume (${WHO:-not signed in}): $(run_get "$T" kill-fixture .reason.message)"
  fi
  tfs_cleanup
fi
# Local paths go absolute: `cozy model upload ./folder` resolves them against Calcifer, not here (2026-10-24).
# Refused before the bulk downloads (tfs-137): nothing reaches the Hub, so no model to delete.
if tfs_case family && [ -n "$WHO" ]; then
  tfs_step family-refusal model upload "hf://wangqyqq/sd_xl_base_1.0_inpainting_0.1.safetensors@26a6b2ff3ea6d7762d33eb57b0ad424f96ac3e09/sd_xl_base_1.0_inpainting_0.1.safetensors" \
    "$WHO/ci-tfs-family-$R" --lane fp16 --on "$M" -y
  tfs_step layout-probe model upload "hf://Comfy-Org/MiniMax-H3@77aac5182eb85bb20930536bac73e498bbe2bfd7/diffusion_models/minimax_h3_ref2va_bf16.safetensors" \
    "$WHO/ci-tfs-layout-$R" --lane bf16 --source-format h3.native --on "$M" -y
fi
if ! tfs_case upload; then :
elif [ -n "$WHO" ] && tfs_weights "$T/up-a" "$TFS_UPLOAD_MB" && tfs_weights "$T/up-c" "$TFS_UPLOAD_MB"; then
  d=$WHO/ci-tfs-$R; TFS_DESTS+=("$d")
  tfs_step upload model upload "$T/up-a" "$d" --lane bf16 --on "$M" -y
  tfs_step upload-again model upload "$T/up-a" "$d" --lane bf16-b --on "$M" -y
  tfs_cut upload-cut "uploading to" 0.3 model upload "$T/up-c" "$d" --lane bf16-c --on "$M" -y
  tfs_step upload-resume model upload "$T/up-c" "$d" --lane bf16-c --on "$M" -y
  tfs_cleanup
else
  expect tensorfs-upload FAIL "no account (${WHO:-not signed in}) or no weights to upload"
fi
# One output, each component encoded in its own call with its own plan (minimax-h3-tools' recipes). Escaped from
# TensorFS 0.17.0 (#2118) to #2238: a writer bound one conversion and refused the second component (run 20032).
if tfs_case multi-conversion && [ -n "$WHO" ]; then
  ln -sfn "$HERE/suites/tensorfs/multi-conversion" "$OUT/ci-multi-conversion"
  d=$WHO/ci-tfs-mc-$R; TFS_DESTS+=("$d")
  tfs_step mc-source run ./ci-multi-conversion/source seed:=$R --on "$M"
  [ "$(run_get "$T" mc-source .status)" != completed ] ||
    tfs_step mc-upload model upload "run:$(cat "$T/mc-source.number")#model" "$d" --lane bf16 --on "$M" -y
  c=$(run_get "$T" mc-upload "[.result.models[]?.published | select(.destination == \"$d\") | .commit][0]")
  [ -z "$c" ] || tfs_step mc-quantize model quantize "$d@$c" --lane int8 --package ./ci-multi-conversion --on "$M" -y
  c=$(run_get "$T" mc-quantize '[.output[]? | select(.name == "int8") | .checkpoint.revision][0]')
  [ -z "$c" ] || tfs_step mc-inspect run ./ci-multi-conversion/inspect "model.model=$d@$c" seed:=$R --on "$M"
  tfs_cleanup
fi
# On this host, after the machine's cases: the machine is not left idle while it runs.
if tfs_case straggler; then
  tfs_straggler straggler-clean 0
  tfs_straggler straggler-slow 3
  tfs_straggler straggler-cold 3 2097152
  tfs_straggler straggler-crawl 3 65536 8388608
  tfs_straggler straggler-parts 16 65536 2097152 fast
  tfs_straggler straggler-link 0 --link 5000000
  mkdir -p "$T/straggler-held-store/tmp"
  rm -f "$T/straggler-held.ready"
  python3 -I - "$T/straggler-clean.proxy" "$T/straggler-held-store/tmp" "$T/straggler-held.ready" <<'PY' & held=$!
import fcntl, json, os, sys, time
locks = []
for sha in {r['path'].rsplit('/', 1)[1] for r in map(json.loads, open(sys.argv[1])) if r.get('kind') == 'object'}:
    locks.append(open(os.path.join(sys.argv[2], 'partial-' + sha), 'a'))
    fcntl.flock(locks[-1], fcntl.LOCK_EX)
open(sys.argv[3], 'w').close()
time.sleep(3600)
PY
  for _ in $(seq 50); do [ -e "$T/straggler-held.ready" ] && break; sleep 0.1; done
  tfs_straggler straggler-held 0
  kill "$held"
  tfs_straggler straggler-silent 0 --silent
  tfs_straggler straggler-drip 0 --drip
  tfs_burst straggler-burst
fi
if tfs_case lora-alpha; then tfs_lora_alpha; fi
