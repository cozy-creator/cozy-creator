# One RTX 4090 (release lane, proof-only): Runtime 0.34.6 candidate (#1857's allocator) against 0.34.5 on the same pod, then
# the qwen-image-2 0.1.3 promotion check. Arms in order, each after a restart (a Runtime update or a GPU-limit write):
#   a: 0.34.5 (the booted prod image)  H3 Turbo 15 s -> SDXL -> Qwen 0.1.1 4MP
#   b: 0.34.6+cand1                   H3 Turbo 15 s -> SDXL -> Qwen 0.1.1 4MP
#   c: 0.34.5                         SDXL -> Qwen 0.1.3 4MP (packages #675 + #676)
#   d: 0.34.5                         SDXL -> Qwen 0.1.1 4MP (control)
CAND='https://github.com/cozy-creator/cozy-runtime/releases/download/cand1-0346-3146ae84d/cozy_runtime-0.34.6%2Bcand1-cp312-abi3-manylinux_2_28_x86_64.whl#sha256=847ff38e82e9cb8970cba276cdb645c155cb28818c6009b2e0ed60a9c2c5f861'
PROD='https://github.com/cozy-creator/cozy-runtime/releases/download/v0.34.5/cozy_runtime-0.34.5-cp312-abi3-manylinux_2_28_x86_64.whl#sha256=40806b6505128f944976eb9c05f14767f95bfd8c2cad63f771c43e5411d98dbb'
H3REF="[{\"name\": \"$NAME\", \"kind\": \"character\", \"image\": \"$REF\"}]"
QP="This is an RGBA image with transparency. $OBJECT, semi-transparent with soft reflections. The image has alpha channel and the background is transparent."
runtime() { local f; f=$(fetch "$1") || { log "cannot fetch $1"; return 1; }
  "$UP" --json machine update "$M" --runtime "$f" -y > "ab-update-$2.json" 2>&1; log "arm $2: $(jq -c 'select(.machine and .runtime)' "ab-update-$2.json" 2>/dev/null)"; }
arm_h3() {  # arm_h3 <arm> <seed offset> <sky>
  cell "$1-h3" "minimax-h3/ref2va_turbo" references:="$H3REF" seed:=$((R + $2)) duration_s:=15 resolution=768p prompt="<$NAME> $MOTION $3"
  cell "$1-sdxl" "sdxl/generate" prompt="$SCENE, photo, $3" seed:=$((R + $2 + 1))
  cell "$1-qwen011" "qwen-image-2/generate@0.1.1" prompt="$QP $3" seed:=$((R + $2 + 2))
}
arm_h3 a 401 "Dawn."
runtime "$CAND" b && arm_h3 b 411 "Dusk."
runtime "$PROD" c && {
  cell c-sdxl "sdxl/generate" prompt="$SCENE, photo, noon" seed:=$((R + 421))
  cell c-qwen013 "qwen-image-2/generate@0.1.3" prompt="$QP Noon." seed:=$((R + 422))
}
"$CLI" machine update "$M" --gpu-memory-limit off -y > ab-restart-d.log 2>&1; log "arm d: restarted $(grep -E '^status' ab-restart-d.log)"
cell d-sdxl "sdxl/generate" prompt="$SCENE, photo, midnight" seed:=$((R + 431))
cell d-qwen011 "qwen-image-2/generate@0.1.1" prompt="$QP Midnight." seed:=$((R + 432))
python3 -I - . <<'PY' > ab-summary.tsv
import json, os, sys
for name in ("a-h3", "b-h3", "a-qwen011", "b-qwen011", "c-qwen013", "d-qwen011"):
    p = os.path.join(sys.argv[1], name + ".json")
    if not os.path.exists(p):
        print(f"{name}\tmissing"); continue
    d = json.load(open(p))
    st, stg = [], {}
    for c in [d] + (d.get("calls") or []):
        att = (c.get("measurements") or {}).get("attribution") or {}
        for v in (att.get("steps") or {}).values():
            st += [x[1] for x in v.get("series") or []]
        for k, v in (att.get("stages") or {}).items():
            stg[k] = stg.get(k, 0) + v.get("total_ms", 0)
    peak = max([r["peak_bytes"] for c in [d] + (d.get("calls") or []) for r in ((c.get("measurements") or {}).get("memory") or {}).get("driver") or []] or [0])
    big = [s for s in st if s > 5000]
    print(f"{name}\t{d.get('status')}\texec {d.get('execution_ms')}\tpeak {peak // 2**20} MiB\tdecode {round((stg.get('decoding image', 0) + stg.get('decode_video', 0)) / 1000, 1)} s"
          f"\tsteps {[round(s / 1000, 1) for s in big[-8:]]}\t{((d.get('reason') or {}).get('message') or '')[:160]}")
PY
while IFS=$'\t' read -r n rest; do log "ab $n: $rest"; done < ab-summary.tsv
