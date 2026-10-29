# pinprobe (proof copy): what pinning host memory costs on one rental (zeroing, cudaHostRegister on touched and
# untouched pages, Torch's pinned allocator, a register from a background thread, the H2D copy), at 64 MiB-1 GiB.
# params: traffic_gb (4)
. "$(dirname "$0")/lib.sh"
L=$(cd "$(dirname "$0")" && pwd)/lowvram
quote "$(param traffic_gb 4)" 1200
rent
cd "$L" || fail "no tool"
# a TensorD environment with torch: SDXL's package (no weights needed to import it)
step env package install sdxl --on "$NAME" || say "no sdxl env; probing the image's Python"
step probe run ./tool.py op=pinprobe --on "$NAME" --out "$OUT/probe" || fail "probe"
grep -E '^\s*value' "$OUT/probe.txt" | cut -c1-3000 | tee -a "$R"
say "PASS: probe done"
