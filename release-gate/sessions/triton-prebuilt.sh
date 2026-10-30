# triton-prebuilt (proof-only, never merge): the Runtime's Triton sets prebuilt into the wheel (cozy-runtime
# opus/triton-prebuilt) against the candidate that compiles them, on one cheap rental, with SDXL and a LoRA (the bake
# set). Arm C, the candidate's wheel, with the pod's C compiler: its kernel boot compiles the sets; one LoRA request.
# Then the compilers go off PATH, every Triton entry of the kernel store and Triton's cache go. Arm P, the wheel with
# the prebuilt sets: its boot copies them (no compiler to compile with); the same LoRA request, which must match C's
# byte for byte. fixtures/triton_prebuilt_eval.py holds the bars. params:
#   runtimes    {"c": ..., "p": ...}: each arm's Runtime, a release the machine fetches (`0.35.6`) or a cozy-runtime
#               dev-build's manylinux x86_64 wheel (`run:<run id>/<commit>`); p must be the dev-build
#   tensord, tensorfs  what the first update pairs them with (0.13.15, 0.17.13)
#   function    sdxl/generate; lora: an SDXL LoRA ref; request: {prompt, seed, steps}
#   traffic_gb (30), need_s (2400)
. "$(dirname "$0")/lib.sh"
RUNTIMES=$(param runtimes '{}') FN=$(param function sdxl/generate) LORA=$(param lora)
REQUEST=$(param request '{}') FIX=$(dirname "$0")/fixtures
[ -f "$FIX/triton_prebuilt_eval.py" ] && [ -n "$LORA" ] && jq -e '(.prompt | length > 0) and (.seed | type == "number")' <<<"$REQUEST" >/dev/null ||
  fail "fixtures, lora or request"
jq . <<<"$REQUEST" > "$OUT/request.json"
command -v ffmpeg >/dev/null || sudo apt-get install -y -qq ffmpeg >/dev/null 2>&1 || fail "no ffmpeg"
declare -A WHEEL WANT
for arm in c p; do
  src=$(jq -r --arg a "$arm" '.[$a] // empty' <<<"$RUNTIMES")
  if [[ $src =~ ^run:([0-9]+)/([0-9a-f]{40})$ ]]; then
    mkdir -p "$OUT/wheel-$arm"
    gh run download "${BASH_REMATCH[1]}" -R cozy-creator/cozy-runtime -n "cozy-runtime-${BASH_REMATCH[2]}" -D "$OUT/wheel-$arm" >/dev/null 2>&1 ||
      fail "arm $arm: no artifact cozy-runtime-${BASH_REMATCH[2]} in run ${BASH_REMATCH[1]}"
    WHEEL[$arm]=$(ls "$OUT"/wheel-$arm/cozy_runtime-*manylinux*x86_64.whl 2>/dev/null | head -1)
    [ -f "${WHEEL[$arm]}" ] || fail "arm $arm: no manylinux x86_64 wheel"
    WANT[$arm]=$(basename "${WHEEL[$arm]}" | cut -d- -f2)
    say "arm $arm wheel $(basename "${WHEEL[$arm]}"), $(unzip -l "${WHEEL[$arm]}" | grep -c 'cozy_runtime/_triton/') prebuilt files"
  elif [[ $src =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    WHEEL[$arm]=$src WANT[$arm]=$src
    say "arm $arm Runtime $src, the machine fetches it"
  else
    fail "arm $arm: runtime '$src' is neither a release nor run:<id>/<commit>"
  fi
done
[ -d "$OUT/wheel-p" ] || fail "arm p is not a dev-build wheel"
# The pod side: the kernel store's Triton entries and Triton's cache cleared (`clear`), the C compilers off or back on
# PATH (`cc off|on`), and the newest environment of a package by its generation record's own interpreter (`python
# <name>`: the record's package `<org>/<name>`, or its application's module, `<module>.<path>:<App>`).
POD_PY=$(cat <<'EOF'
import glob, json, os, shutil, sys
mode = sys.argv[1]
if mode == "clear":
    removed = 0
    for store in glob.glob("/var/lib/cozy/tensord/gpu/kernels/*"):
        for entry in glob.glob(os.path.join(store, "*", "*", "entry.json")):
            try:
                key = json.load(open(entry)).get("key", {})
            except (OSError, ValueError):
                continue
            if "launcher" in key.get("inputs", {}):
                shutil.rmtree(os.path.dirname(entry), ignore_errors=True); removed += 1
        shutil.rmtree(os.path.join(store, "triton"), ignore_errors=True)
    print(json.dumps({"removed_entries": removed}))
elif mode == "cc":
    moved = []
    for name in ("gcc", "cc", "clang", "x86_64-linux-gnu-gcc"):
        for path in (f"/usr/bin/{name}", f"/usr/local/bin/{name}"):
            off = path + ".cozy-off"
            if sys.argv[2] == "off" and os.path.lexists(path):
                os.rename(path, off); moved.append(path)
            elif sys.argv[2] == "on" and os.path.lexists(off):
                os.rename(off, path); moved.append(path)
    print(json.dumps({"cc": sys.argv[2], "moved": moved, "which_gcc": shutil.which("gcc"), "which_cc": shutil.which("cc")}))
elif mode == "python":
    found = []
    for record in glob.glob("/var/lib/cozy/tensord/generations/*/generation.json"):
        try:
            d = json.load(open(record))
        except (OSError, ValueError):
            continue
        names = {str(d.get("package", "")).split("/")[-1], str(d.get("application", "")).split(":")[0].split(".")[0]}
        if sys.argv[2] in names and d.get("python"):
            found.append((os.path.getmtime(record), d["python"]))
    print(max(found)[1] if found else "")
EOF
)
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param traffic_gb 30)" "$(param need_s 2400)"
rent
pod() { timeout "${T:-120}" cozy machine ssh "$NAME" "$@"; }
for i in $(seq 1 20); do pod true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done
podpy() { printf '%s' "$POD_PY" | T=300 pod "cat > /tmp/tp.py && python3 /tmp/tp.py $*"; }
runtime() { cozy machine show "$NAME" --json 2>/dev/null | jq -r '.status.runtime // empty'; }
BOOT=/var/lib/cozy/tensord/gpu/kernel-boot.jsonl
boots() { T=30 pod "cat $BOOT 2>/dev/null" < /dev/null | grep -c '"generation"'; }
booted() {
  local t n
  for t in $(seq 1 $(($3 * 6))); do n=$(boots); [ "${n:-0}" -ge "$2" ] && { say "$1: $n kernel boots ended"; return 0; }; sleep 10; done
  say "WARNING $1: $n of $2 kernel boots after $3 min"
}
update() {
  local n; n=$(boots)
  step "update-$1" machine update "$NAME" --runtime "${WHEEL[$1]}" -y || fail "update to arm $1"
  [ "$(runtime)" = "${WANT[$1]}" ] || fail "$NAME runs Runtime $(runtime), not ${WANT[$1]}"
  say "ASSERT $NAME runs Runtime ${WANT[$1]}"
  booted "arm $1 boot" $((n + 1)) 20
}
request() {
  step "$1" run "$FN" --input "$OUT/request.json" --lora "$LORA" --gpus=1 --on "$NAME" --out "$OUT/out/$1"
  local n; n=$(runno "$1"); [ -n "$n" ] || n=$(grep -oE 'cozy run show [0-9]+' "$OUT/$1.txt" | tail -1 | awk '{print $4}')
  [ -n "$n" ] || fail "$1 named no run"
  cozy run show "$n" --once --json > "$OUT/$1.json" 2>&1
  say "$1 run $n $(jq -r .status "$OUT/$1.json"), exec $(jq -r '(.execution_ms // 0) / 1000' "$OUT/$1.json") s"
}
booted "image boot" 1 20
n=$(boots); step install package install "${FN%%/*}" --on "$NAME" || fail "install"
booted "install boot" $((n + 1)) 6
n=$(boots)
step pair machine update "$NAME" --tensord "$(param tensord 0.13.15)" --tensorfs "$(param tensorfs 0.17.13)" --runtime "${WHEEL[c]}" -y ||
  fail "the first update"
cozy machine show "$NAME" --json > "$OUT/show-pair.json" 2>&1
say "pod $(jq -c '{version: .status.version, runtime: .status.runtime, tensorfs: .status.tensorfs, gpu: .status.gpus[0].name}' "$OUT/show-pair.json")"
[ "$(runtime)" = "${WANT[c]}" ] || fail "$NAME runs Runtime $(runtime), not ${WANT[c]}"
booted "arm c boot" $((n + 1)) 20
request c1
T=30 pod "cat $BOOT" < /dev/null > "$OUT/kernel-boot-c.jsonl" 2>&1
podpy cc off > "$OUT/cc-off.json"; say "compilers: $(cat "$OUT/cc-off.json")"
podpy clear > "$OUT/clear-p.json"; say "cleared: $(cat "$OUT/clear-p.json")"
update p
request p1
PY=$(podpy python "${FN%%/*}" | tail -1)
say "package environment python: ${PY:-none}"
[ -z "$PY" ] || T=120 pod "$PY -I -c 'import json, cozy_runtime.internal.triton_kernels as t; print(json.dumps({\"launcher\": t.LAUNCHER, \"shipped\": t._launcher_shipped() is not None, \"kernels\": len(t._manifest().get(\"kernels\", {}))}))'" < /dev/null > "$OUT/package-env.json" 2>&1
T=30 pod "cat $BOOT" < /dev/null > "$OUT/kernel-boot-p.jsonl" 2>&1
podpy cc on > "$OUT/cc-on.json"
for r in "${RIDS[@]}"; do cozy machine end "$r" -y >> "$OUT/end.txt" 2>&1 || cozy machine end "$r" -y --force >> "$OUT/end.txt" 2>&1; done
say "END requested after the last request"
# As ab.sh compares: the result digests, else the decoded image.
digests() { jq -r '[.result | .. | .digest? // empty] | join(",")' "$OUT/$1.json" 2>/dev/null; }
frames() { local f; f=$(jq -r '.output[]?.path' "$OUT/$1.json" 2>/dev/null | grep -E '\.(png|webp|jpg)$' | head -1)
  [ -s "$f" ] || { echo none; return; }; ffmpeg -nostdin -v error -i "$f" -map 0 -f framemd5 - | grep -v '^#' | md5sum | cut -c1-16; }
da=$(digests c1) db=$(digests p1) fa=$(frames c1) fb=$(frames p1)
if [ -n "$da" ] && [ "$da" = "$db" ]; then echo "p1 identical bytes" >> "$OUT/ab.txt"
elif [ "$fa" != none ] && [ "$fa" = "$fb" ]; then echo "p1 identical decoded image" >> "$OUT/ab.txt"
else echo "p1 differs: digests $da / $db, decoded $fa / $fb" >> "$OUT/ab.txt"; fi
say "A/B against c1: $(paste -sd' ' "$OUT/ab.txt" 2>/dev/null)"
python3 "$FIX/triton_prebuilt_eval.py" "$OUT" | tee -a "$R"
[ "${PIPESTATUS[0]}" = 0 ] || fail "a bar failed"
say "PASS"
