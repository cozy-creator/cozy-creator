# e2e/cli/previews.sh: the `previews` part (cr-282): a denoise loop on the CPU (project/latents.py) shows its latents to a
# client watching its run. `cozy run --previews` and `cozy run watch --previews` save each new one as it runs; a run no
# one asks about makes none. The box's side is web/previews.js (web.sh).

previews() {
[ -n "$LOCAL" ] || { echo "previews: this computer runs no models itself"; return 0; }
local tool=$here/project/latents.py dir=$OUT/previews n count f follower
step "previews: a denoise loop's previews saved while it runs (cozy run --previews)"
if (cd "$here" && cell previews "$tool/latents" steps:=16 seconds:=0.5 --previews "$dir"); then
  n=$(cat "$OUT/previews.number")
  count=$(find "$dir" -name "$n-preview-*.webp" | wc -l)
  [ "$count" -ge 2 ] || check "previews: $count of run $n's previews saved in $dir, not 2 or more"
  for f in "$dir"/*.webp; do
    [ "$(head -c 4 "$f")" = RIFF ] && [ "$(tail -c +9 "$f" | head -c 4)" = WEBP ] || check "previews: $f is no WebP"
  done
  grep -q '"preview": *[0-9]' "$OUT/previews.ndjson" || check "previews: no progress of run $n names a preview"
fi
step "previews: none made for a run no one asks about"
if (cd "$here" && cell unwatched "$tool/latents" steps:=8 seconds:=0.5); then
  ! grep -q '"preview"' "$OUT/unwatched.ndjson" || check "previews: run $(cat "$OUT/unwatched.number") made previews with no one asking"
fi
step "previews: a running run followed again, its previews saved (cozy run watch --previews)"
(cd "$here" && run_submit "$OUT" watched run "$tool/latents" steps:=16 seconds:=0.5 --on local > /dev/null) || true
n=$(cat "$OUT/watched.number" 2> /dev/null) || n=
if [ -n "$n" ] && [ "$n" != none ]; then
  # Its number names a run of the project it was made in (run_start's).
  (cd "${RUN_PROJECT:-$here}" && cozy run watch "$n" --previews "$OUT/watched-previews" > "$OUT/watched-watch.txt" 2>&1) \
    || check "previews: cozy run watch $n --previews failed: $(tail -c 300 "$OUT/watched-watch.txt")"
  count=$(find "$OUT/watched-previews" -name "$n-preview-*.webp" 2> /dev/null | wc -l)
  [ "$count" -ge 1 ] || check "previews: cozy run watch $n --previews saved none"
  run_wait "$OUT" watched || check "previews: run $n did not complete"
else
  check "previews: the watched run never began: $(tail -c 300 "$OUT/watched.err" 2> /dev/null)"
fi
}
