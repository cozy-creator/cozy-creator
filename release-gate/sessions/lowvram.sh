# lowvram: Cozy against ComfyUI under real VRAM limits on one consumer card (owner: 8, 6 and 3 GB; varena must match or
# beat ComfyUI on load time and bytes moved). One rental. Per VRAM level a separate non-Cozy process holds the rest of
# the card, as a user's game would, so both engines see the same driver-level limit. Per level and model: a fresh
# engine with the page cache dropped (cold), then warm requests, each with its own prompt and seed. ComfyUI runs
# first, then each Cozy arm: the machine as rented, then any candidate after `cozy machine update`. One NVML sampler
# on the pod measures both engines: H2D bytes (PCIe RX) and peak memory used per request.
# params:
#   levels   MiB left free; "full" holds nothing (["full", 6144, 3072])
#   models   from sdxl (16-bit), anima (16-bit), qwen (Qwen-Image 2.1 1 MP, int8 ConvRot), qwen4 (the same at
#            4 MP; Cozy only) (["sdxl", "anima", "qwen"])
#   warm     warm requests after each cold one (2)
#   arms     [{"name": "comfy"}, {"name": "prod"}, {"name": <cand>, "update": [machine update flags]},
#            {"name": <capped>, "update": [...], "limit": "6GiB", "levels": [6144]}]; an arm with "limit" sets
#            --gpu-memory-limit, holds nothing and comes last; an arm's "levels" replace the session's; default
#            [comfy, prod]. An update value "artifact:<run id>:<name>" is that cozy-creator run's artifact (a dev
#            TensorFS wheel); "url:<https URL>#sha256=<hex>" a proof wheel. Both are fetched here, before renting,
#            and sent from this runner.
#   an arm's "local": {"<model>": "<packages_refs name>"} runs that model's package from that packages checkout
#            (installed by path, removed as local/<package>) instead of its published release
#   an arm's "ram_free_gib": N holds host RAM so only N GiB stay available while it runs (a smaller machine)
#   skip     ["<level>/<model>", …] cells no arm runs (a known failure measured before); an arm's "skip" replaces it
#   keep_going  true: a failed request does not end its cell (to measure the executor after a failure)
#   stall_s  a request with no end after this many seconds is diagnosed (machine logs) and abandoned (1200)
#   seed_base the first request's seed (2026102700); a later session takes a new base, and every prompt is new
#   traffic_gb, need_s
. "$(dirname "$0")/lib.sh"
L=$(cd "$(dirname "$0")" && pwd)/lowvram
LEVELS=$(param levels '["full", 6144, 3072]') MODELS=$(param models '["sdxl", "anima", "qwen"]') WARM=$(param warm 2)
STALL_S=$(param stall_s 1200)
ARMS=$(param arms '[{"name": "comfy"}, {"name": "prod"}]') SKIP=$(param skip '[]') KEEP=$(param keep_going false)
G=$OUT/gen; mkdir -p "$G" "$OUT/out" "$OUT/rec"
mapfile -t levels < <(jq -r '.[] | tostring' <<<"$LEVELS")
mapfile -t models < <(jq -r '.[]' <<<"$MODELS")
mapfile -t arms < <(jq -r '.[].name' <<<"$ARMS")
for m in "${models[@]}"; do case $m in sdxl|anima|qwen|qwen4|krea) ;; *) fail "no model $m" ;; esac; done
for a in "${arms[@]}"; do [[ $a =~ ^[a-z0-9]+$ ]] || fail "arm names are letters and digits: $a"; done
COMFY=$(jq -r 'map(.name) | index("comfy") // -1' <<<"$ARMS")
[ "$COMFY" -le 0 ] || fail "comfy runs first, on a card no Cozy executor has touched"
COZY1=${arms[0]}; [ "$COZY1" = comfy ] && COZY1=${arms[1]:-}
jq -e 'map(has("limit")) | . == (sort)' <<<"$ARMS" >/dev/null || fail "arms with a memory limit come last"
arm_levels() { jq -r --arg a "$1" --argjson d "$LEVELS" '.[] | select(.name==$a) | (.levels // $d)[] | tostring' <<<"$ARMS"; }
# Cozy side: package, model and lane per model.
pkg() { case $1 in sdxl) echo sdxl ;; anima) echo anima ;; qwen|qwen4) echo qwen-image-2 ;; krea) echo krea-2 ;; esac; }
# Cozy runs every model's 8-bit lane, what users run (Paul 10-28); ComfyUI comparisons are the benchmarks runner's (bench).
model_ref() { case $1 in sdxl) echo "sdxl@1.0.0 int8" ;; anima) echo "anima@1.0.0 int8" ;; qwen|qwen4) echo "qwen-image-2.1@0.1.0 int8" ;; krea) echo "krea-2@0.1.0 int8" ;; esac; }
tiers() { echo cold; for i in $(seq 1 "$WARM"); do echo "w$i"; done; }
tree() { jq -r --arg a "$1" --arg m "$2" '.[] | select(.name==$a) | (.local // {})[$m] // empty' <<<"$ARMS"; }
for a in "${arms[@]}"; do for m in "${models[@]}"; do
  t=$(tree "$a" "$m"); [ -z "$t" ] || [ -d "$PACKAGES/$t/$(pkg "$m")" ] || fail "arm $a: no packages checkout $t/$(pkg "$m")"
done; done

# Render: the ComfyUI lock into its setup script; one prompt and seed per request; parse checks.
python3 - "$L/comfy_setup.py" "$L/comfy-requirements.lock" "$G/comfy_setup.py" <<'PY' || fail "render comfy_setup"
import sys
src, lock, dst = sys.argv[1:]
s = open(src).read()
assert s.count("REQUIREMENTS = None  # @REQUIREMENTS@") == 1
open(dst, "w").write(s.replace("REQUIREMENTS = None  # @REQUIREMENTS@", "REQUIREMENTS = " + repr(open(lock).read())))
PY
cp "$L/tool.py" "$L/comfy_cell.py" "$G/"
for f in "$G"/*.py "$L"/*.py; do python3 -I -m py_compile "$f" || fail "py_compile $f"; done
mapfile -t cozy_arms < <(printf '%s\n' "${arms[@]}" | grep -vx comfy)
python3 -I "$L/requests.py" "$OUT/requests.json" "$(param seed_base 2026102700)" "${cozy_arms[@]}" || fail "requests"
# key <arm> <level> <model> <tier>: ComfyUI and the first Cozy arm share a key; later arms have their own.
key() { local a=$1; [ "$a" = comfy ] || [ "$a" = "$COZY1" ] && a=shared; echo "$a/$2/$3/$4"; }
# Dev artifacts named by arms' updates: downloaded before renting, so a missing one costs nothing.
mapfile -t artifacts < <(jq -r '.[].update // [] | .[] | select(startswith("artifact:") or startswith("url:"))' <<<"$ARMS" | sort -u)
artifact_dir() { echo "$OUT/artifacts/$(sha256sum <<<"$1" | cut -c1-12)"; }
# A Linux x86_64 wheel, else a dev cozy binary (dev-build's cozy-linux-x86_64), made executable.
artifact_path() {
  local d f; d=$(artifact_dir "$1")
  f=$(ls "$d"/*.whl 2>/dev/null | grep -m1 'manylinux.*x86_64') && { echo "$f"; return; }
  f=$(find "$d" -type f -name 'cozy-linux-x86_64' | head -1); [ -n "$f" ] || return 1
  chmod +x "$f"; echo "$f"
}
for a in "${artifacts[@]}"; do
  d=$(artifact_dir "$a"); mkdir -p "$d"
  if [[ $a == artifact:* ]]; then
    IFS=: read -r _ rid name <<<"$a"
    gh run download "$rid" -R cozy-creator/cozy-creator -n "$name" -D "$d" >/dev/null 2>&1 || fail "no artifact $a"
  else
    u=${a#url:} want=${a##*#sha256=}; u=${u%%#*}; f=${u##*/}; f=${f//%2B/+}  # a local version's +
    curl -fsSL --retry 5 -o "$d/$f" "$u" || fail "cannot fetch $u"
    [ "$(sha256sum "$d/$f" | cut -d' ' -f1)" = "$want" ] || fail "$u is not sha256 $want"
  fi
  w=$(artifact_path "$a") || fail "$a has no Linux x86_64 wheel or cozy binary"
  say "fetched $a: ${w##*/} sha256 $(sha256sum "$w" | cut -c1-16)"
done
req() { jq -c --arg k "$(key "$@")" '.requests[$k] // error("no request \($k)")' "$OUT/requests.json"; }
for a in "${arms[@]}"; do for l in $(arm_levels "$a"); do for m in "${models[@]}"; do for t in $(tiers); do
  req "$a" "$l" "$m" "$t" >/dev/null || fail "no request for $a/$l/$m/$t"
done; done; done; done
cozy_input() {  # cozy_input <model> <request json> <file>: Cozy's inputs at ComfyUI's graph settings
  local m=$1 r=$2
  case $m in
    sdxl) jq -n --argjson r "$r" '{prompt: $r.prompt, seed: $r.seed, steps: 20, guidance: 7.0, negative_prompt: "",
            megapixels: 1.0, aspect_ratio: "1:1", prompt_upsampling: false, hidiffusion: false}' ;;
    anima) jq -n --argjson r "$r" --arg neg "$ANIMA_NEG" '{prompt: $r.prompt, seed: $r.seed, steps: 30, guidance: 4.0,
            negative_prompt: $neg, megapixels: 1.0, aspect_ratio: "1:1", prompt_upsampling: false,
            cfg_interval_start: 0.0, cfg_interval_stop: 1.0}' ;;
    qwen) jq -n --argjson r "$r" '{prompt: $r.prompt, seed: $r.seed, steps: 40, megapixels: 1.0, aspect_ratio: "1:1"}' ;;
    qwen4) jq -n --argjson r "$r" '{prompt: $r.prompt, seed: $r.seed, steps: 40, megapixels: 4.0, aspect_ratio: "1:1"}' ;;
    krea) jq -n --argjson r "$r" '{prompt: $r.prompt, seed: $r.seed}' ;;  # Krea 2 Raw's own settings (52 steps, 1 MP)
  esac > "$3"
}
ANIMA_NEG="worst quality, low quality, score_1, score_2, score_3, blurry, jpeg artifacts, sepia"
comfy_jobs() {  # comfy_jobs <level> <model> <file>
  local l=$1 m=$2 t r out='[]'
  for t in $(tiers); do
    r=$(req comfy "$l" "$m" "$t")
    out=$(jq -c --argjson r "$r" --arg label "comfy-$l-$m-$t" --arg m "$m" --arg neg "$ANIMA_NEG" '. + [{label: $label, cell: $m,
      prompt: $r.prompt, seed: $r.seed, steps: ({sdxl: 20, anima: 30, qwen: 40}[$m]), guidance: ({sdxl: 7.0, anima: 4.0, qwen: 1.0}[$m]),
      negative: $neg}]' <<<"$out")
  done
  jq -n --arg jobs "$out" '{jobs: $jobs}' > "$3"
}
for l in "${levels[@]}"; do for m in "${models[@]}"; do comfy_jobs "$l" "$m" "$OUT/chk.json" && cozy_input "$m" "$(req "$COZY1" "$l" "$m" cold)" "$OUT/chk2.json" ||
  fail "inputs for $l/$m"; done; done
rm -f "$OUT/chk.json" "$OUT/chk2.json"
say "plan: levels ${levels[*]}; models ${models[*]}; arms ${arms[*]}; $WARM warm after each cold"
quote "$(param traffic_gb 90)" "$(param need_s 5400)"
rent
cd "$G" || fail "no $G"

# qstep <name> <cozy args…>: lib.sh's step, polled every second (its 30 s poll rounds every call up to 30 s); output
# that stops growing for 15 min still cancels it.
qstep() {
  local n=$1; shift; say "step $n: cozy $*"
  ( cd "${QCWD:-.}" && exec cozy "$@" ) > "$OUT/$n.txt" 2>&1 & local p=$! last=-1 still=0 size
  while kill -0 "$p" 2>/dev/null; do
    sleep 1; size=$(stat -c %s "$OUT/$n.txt")
    if [ "$size" = "$last" ]; then still=$((still + 1)); else still=0; last=$size; fi
    [ "$still" -lt 900 ] || { kill "$p"; say "$n: no output for 15 min"; return 124; }
  done
  wait "$p"; local c=$?
  tail -6 "$OUT/$n.txt" | cut -c1-300 | sed 's/^/    /' >> "$R"
  return $c
}
tool() {  # tool <step> <op> [inputs]: tool.py's result value, or fail
  local n=$1; shift
  qstep "$n" run ./tool.py "op=$1" "${@:2}" --on "$NAME" --out "$OUT/out/$n" || return 1
  local N; N=$(runno "$n"); cozy run show "$N" --once --json > "$OUT/rec/$n.json" 2>/dev/null
  [ "$(jq -r .status "$OUT/rec/$n.json")" = completed ] || return 1
  jq -r '.result.value' "$OUT/rec/$n.json" | tee -a "$OUT/tool.log" | cut -c1-400 >> "$R"
}
cozy_run() {  # cozy_run <label> <model> <input file> <arm>: one request, its record and a mark after it
  local n=$1 m=$2 lane t target cwd=.; lane=$(model_ref "$m" | cut -d' ' -f2) t=$(tree "$4" "$2")
  target="$(pkg "$m")/generate"; [ -z "$t" ] || { target="./$target" cwd=$PACKAGES/$t; }
  local t0; t0=$(date +%s%3N)
  # A request with no end in STALL_S seconds is a stall to diagnose: keep the machine's logs, then stop following it.
  ( QCWD=$cwd qstep "$n" run "$target" --input "$3" --lane "$lane" --on "$NAME" --out "$OUT/out/$n" ) & local qp=$! waited=0
  while kill -0 "$qp" 2>/dev/null && [ "$waited" -lt "${STALL_S:-1200}" ]; do sleep 5; waited=$((waited + 5)); done
  if kill -0 "$qp" 2>/dev/null; then
    say "STALL: $n has no end after ${waited}s; keeping the machine's logs"
    for log in tensord memory tensorfs; do cozy machine logs "$NAME" "$log" -n 0 > "$OUT/stall-$n-$log.log" 2>&1; done
    cozy machine show "$NAME" --json > "$OUT/stall-$n-show.json" 2>&1
    tool "stacks-$n" stacks > /dev/null || say "no stacks for $n"
    kill "$qp" 2>/dev/null
  fi
  wait "$qp"; local code=$?
  echo "{\"label\": \"$n\", \"wall_ms\": $(( $(date +%s%3N) - t0 )), \"exit\": $code}" >> "$OUT/walls.jsonl"
  local N; N=$(runno "$n")
  # The JSON record has no logs (a package's own, a native refusal's reason); the table does.
  [ -z "$N" ] || { cozy run show "$N" --once --json > "$OUT/rec/$n.json" 2>/dev/null
    cozy run show "$N" --once > "$OUT/rec/$n.log" 2>&1; }
  say "$n run ${N:-none} $(jq -r '.status // "none"' "$OUT/rec/$n.json" 2>/dev/null) exec $(jq -r '(.execution_ms // 0) / 1000' "$OUT/rec/$n.json" 2>/dev/null)s wall $(( $(date +%s%3N) - t0 ))ms"
  tool "mark-$n" mark "label=$n" > /dev/null || fail "mark after $n"
}

# Setup: the sampler, ComfyUI and its files (the holder runs on its torch), Cozy's packages and weights.
CMODELS=$MODELS; [ "$COMFY" = 0 ] || CMODELS='[]'  # without a ComfyUI arm only its venv: the holder's torch
qstep comfy-setup run ./comfy_setup.py "models:=$CMODELS" --on "$NAME" --out "$OUT/out/comfy-setup" || fail "ComfyUI setup"
N=$(runno comfy-setup); cozy run show "$N" --once --json > "$OUT/rec/comfy-setup.json" 2>/dev/null
jq -r '.result.setup' "$OUT/rec/comfy-setup.json" > "$OUT/comfy-setup.json" 2>/dev/null
jq -e '.ok' "$OUT/comfy-setup.json" >/dev/null 2>&1 || fail "ComfyUI setup not ok: $(head -c 600 "$OUT/comfy-setup.json")"
say "ComfyUI: $(jq -c '{torch, download_gb, download_wall_s, seconds, host}' "$OUT/comfy-setup.json" | cut -c1-600)"
tool sampler sampler || fail "sampler"
for m in "${models[@]}"; do
  read -r ref lane <<<"$(model_ref "$m")"
  qstep "download-$m" model download "$ref" --lane "$lane" --on "$NAME" || fail "download $ref"
done
tool gpu-idle mark label=idle || fail "mark"

arm_update() {  # the arm's machine update (a candidate's versions, or a dev memory limit), then the sampler again
  local a=$1 flags
  mapfile -t flags < <(jq -r --arg a "$a" '.[] | select(.name==$a) | (.update // [])[]' <<<"$ARMS")
  for i in "${!flags[@]}"; do [[ ${flags[$i]} == artifact:* || ${flags[$i]} == url:* ]] && flags[$i]=$(artifact_path "${flags[$i]}"); done
  local limit; limit=$(jq -r --arg a "$a" '.[] | select(.name==$a) | .limit // empty' <<<"$ARMS")
  [ -n "$limit" ] && flags+=(--gpu-memory-limit "$limit")
  [ "${#flags[@]}" -gt 0 ] || return 0
  qstep "update-$a" machine update "$NAME" "${flags[@]}" -y || fail "update for $a"
  cozy machine show "$NAME" > "$OUT/show-$a.txt" 2>&1
  grep -E '^(version|runtime|tensorfs|GPU 0) ' "$OUT/show-$a.txt" | tee -a "$R"
  tool "sampler-$a" sampler || fail "sampler after update"
}
for a in "${arms[@]}"; do
  arm_update "$a"
  ram=$(jq -r --arg a "$a" '.[] | select(.name==$a) | .ram_free_gib // 0' <<<"$ARMS")
  tool "ramhold-$a" ramhold "ram_free_gib:=$ram" || fail "ram hold for $a"
  limited=$(jq -r --arg a "$a" '.[] | select(.name==$a) | .limit // empty' <<<"$ARMS")
  for l in $(arm_levels "$a"); do
    free=$l; [ "$l" = full ] && free=0
    [ -n "$limited" ] && free=0
    tool "hold-$a-$l" hold "free_mib:=$free" || fail "hold $l"
    tool "sampler-$a-$l" sampler || fail "sampler"  # a retired executor can take it down
    for m in "${models[@]}"; do
      jq -e --arg a "$a" --arg c "$l/$m" --argjson d "$SKIP" '.[] | select(.name==$a) | (.skip // $d) | index($c)' <<<"$ARMS" >/dev/null &&
        { say "skip $a-$l-$m"; continue; }
      p=$(pkg "$m") pi=$(pkg "$m") t=$(tree "$a" "$m")
      [ -z "$t" ] || { pi=$PACKAGES/$t/$p p=local/$p; }
      if [ "$a" != comfy ]; then  # a fresh executor: no other Cozy executor holds the card
        qstep "remove-$a-$l-$m" package remove "$p" --on "$NAME" -y
        qstep "install-$a-$l-$m" package install "$pi" --on "$NAME" || fail "install $pi"
      fi
      tool "evict-$a-$l-$m" evict || fail "evict"
      tool "mark-$a-$l-$m-start" mark "label=$a-$l-$m-start" > /dev/null || fail "mark"
      if [ "$a" = comfy ]; then
        comfy_jobs "$l" "$m" "$G/comfy-$l-$m.json"
        qstep "comfy-$l-$m" run ./comfy_cell.py --input "$G/comfy-$l-$m.json" --on "$NAME" --out "$OUT/out/comfy-$l-$m"
        N=$(runno "comfy-$l-$m"); [ -z "$N" ] || cozy run show "$N" --once --json > "$OUT/rec/comfy-$l-$m.json" 2>/dev/null
        jq -r '.result.session' "$OUT/rec/comfy-$l-$m.json" > "$OUT/comfy-$l-$m.session.json" 2>/dev/null
        say "comfy-$l-$m: $(jq -c '[.per_request[] | {label, status, total_s, h2d_gb, peak_used_mib}]' "$OUT/comfy-$l-$m.session.json" 2>/dev/null | cut -c1-700)"
        tool "mark-comfy-$l-$m-end" mark "label=comfy-$l-$m-end" > /dev/null || fail "mark"
      else
        for t in $(tiers); do
          cozy_input "$m" "$(req "$a" "$l" "$m" "$t")" "$G/$a-$l-$m-$t.json"
          cozy_run "$a-$l-$m-$t" "$m" "$G/$a-$l-$m-$t.json" "$a"
          # A failed request's executor is not the product's warm state (an OOM can leave it crawling).
          [ "$(jq -r '.status // "none"' "$OUT/rec/$a-$l-$m-$t.json" 2>/dev/null)" = completed ] || [ "$KEEP" = true ] ||
            { say "skip the rest of $a-$l-$m after a failed $t"; break; }
        done
        qstep "remove-$a-$l-$m-end" package remove "$p" --on "$NAME" -y
      fi
    done
  done
done
for log in tensord memory; do cozy machine logs "$NAME" "$log" -n 0 > "$OUT/machine-$log.log" 2>&1; done
tool hold-release hold "free_mib:=0"
tool ramhold-release ramhold "ram_free_gib:=0"
tool collect collect || fail "collect"
python3 -I "$L/analyze.py" "$OUT" > "$OUT/table.md" 2> "$OUT/analyze.err" && cat "$OUT/table.md" >> "$R" || say "analyze failed: $(tail -3 "$OUT/analyze.err")"
