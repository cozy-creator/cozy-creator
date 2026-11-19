# bench_fields (cl-1279). Paul, on a field a cozy doesn't read: "warning, not error". 11-19: packages on rc/0.28.0 gained benchmark
# row fields (`lane`, `model`, `cold_ms`, memory: packages #1040, #1014), and a released 0.27 cozy refused to publish any of them
# (`benchmarks_invalid: benchmarks.json: 0.lane is no field of a benchmark`). A cozy must not do that to the next one's fields: one line
# names each field it doesn't read with its rows, the field is left out, and the rows publish. isolated.sh published e2e-fields 0.1.0,
# `e2e_unread` in its rows 0, 1 and 3 and in row 1's `ran_on`: red before cl-1279's fix, which refused it. Its row 2's note is 300 bytes:
# the Hub keeps 200 (`validateBenchmark`) and refused the whole release (packages #1063, a 401-byte note); cozy cuts it at a character to
# fit and says so once (cl-1286, "fix, don't refuse"): 199 bytes, its 200th inside a 2-byte character.
bench_fields() {
step "a benchmarks.json field this cozy doesn't read, or a note over the Hub's 200 bytes, is warned of once with its rows, and the release publishes (cl-1279)"
local said=${E2E_FIELDS_PUBLISHED:-/dev/null} release=$OUT/bench-fields-release.json
[ -n "${E2E_FIELDS:-}" ] || { check "bench fields: isolated.sh published no e2e-fields"; return; }
{ grep -F 'benchmarks.json' "$said" || true; } | sed 's/^ */bench fields: /'
grep -q '"status": *"published"' "$said" \
  || { check "bench fields: e2e-fields, its benchmarks.json carrying a field this cozy doesn't read, was refused: $(tail -c 400 "$said" | tr '\n' ' ')"; return; }
[ "$(grep -c '`e2e_unread`' "$said")" = 1 ] && grep -qF '`e2e_unread` in rows 0-1, 3 is no field this cozy reads' "$said" \
  || check "bench fields: e2e_unread wasn't warned of once, in rows 0-1, 3: $(grep -F 'e2e_unread' "$said" | tr '\n' ' ')"
[ "$(grep -c '`ran_on.e2e_unread`' "$said")" = 1 ] && grep -qF '`ran_on.e2e_unread` in row 1 is no field this cozy reads' "$said" \
  || check "bench fields: ran_on.e2e_unread wasn't warned of once, in row 1: $(grep -F 'ran_on.e2e_unread' "$said" | tr '\n' ' ')"
curl -fsS --retry 3 --retry-all-errors "${COZY_HUB_URL:?}/v1/packages/$E2E_FIELDS/releases/0.1.0" > "$release" 2> "$release.err" \
  || { check "bench fields: the Hub's e2e-fields 0.1.0 didn't read: $(tail -c 300 "$release.err")"; return; }
jq -e '(.benchmark_rows // .benchmarks // []) | length == 4 and ([.. | objects | has("e2e_unread")] | any | not)' "$release" > /dev/null \
  || check "bench fields: the Hub's e2e-fields 0.1.0 doesn't hold its 4 rows without e2e_unread: $(jq -c '.benchmark_rows // .benchmarks' "$release" | head -c 400)"
[ "$(grep -c '`note`' "$said")" = 1 ] && grep -qF '`note` in row 2 is longer than the 200 bytes the Hub keeps: cut to fit' "$said" \
  || check "bench fields: row 2's 300-byte note wasn't warned of once: $(grep -F 'note' "$said" | tr '\n' ' ')"
local kept
kept=$(jq -r '[(.benchmark_rows // .benchmarks // [])[] | select(.gpu == "NVIDIA L4") | .note | utf8bytelength] | first // "none"' "$release")
[ "$kept" = 199 ] || check "bench fields: the Hub's row 2 note is $kept bytes, not 199 (cut at a character to the Hub's 200)"
}
