# pool-scramble: a scratch pool promises nothing crosses its uses. Each model runs one request twice,
# the second call replaying, on its package and on a copy with pool_scramble.py loaded, which unmaps
# every idle pool and fills what comes back with noise. Their outputs must be byte-identical: a
# difference is something crossing a pool's use boundary, which is silent wrong output.
# Sourced in gate.sh's shell, with $PKGS a packages checkout.
scramble() {  # scramble <name> <package> <function> <cell args...>: one model's four runs
  local name=$1 package=$2 function=$3 copy=$OUT/scramble/$2 call a b; shift 3
  rm -rf "$copy"; mkdir -p "$copy"; cp -r "$PKGS/$package/." "$copy"
  local module; module=$(sed -n 's/^object = "\([a-z0-9_]*\):.*/\1/p' "$copy/package.toml")
  cp "$HERE/suites/varena/pool-scramble/pool_scramble.py" "$copy/$module/"
  printf '\nfrom . import pool_scramble  # noqa: E402, F401\n' >> "$copy/$module/__init__.py"
  for call in 1 2; do
    cell "scramble-$name-base-$call" "$(pkg "$package" "$function")" "$@"
    cell "scramble-$name-on-$call" "$copy/$function" "$@"
    a=$(run_output "$OUT" "scramble-$name-base-$call" "${EXT:-webp|png|mp4}")
    b=$(run_output "$OUT" "scramble-$name-on-$call" "${EXT:-webp|png|mp4}")
    if [ -n "$a" ] && cmp -s "$a" "$b"; then
      expect "scramble-$name-$call" PASS "byte-identical with noise in every idle pool"
    else
      expect "scramble-$name-$call" FAIL "output differs with noise in every idle pool: $a $b"
    fi
  done
}
if [ -z "$PKGS" ]; then
  log "pool-scramble needs a packages checkout"
else
  scramble sdxl sdxl generate prompt="$OBJECT on a wooden table, photo" seed:=$((R + 71))
  scramble qwen qwen-image-2 generate prompt="$SCENE, photo" seed:=$((R + 72)) megapixels:=4
  scramble h3 minimax-h3 generate prompt="$MOTION Wide, steady." seed:=$((R + 73)) duration_s:=5 turbo:=true
fi
