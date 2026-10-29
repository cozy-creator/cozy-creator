# sdxl 0.1.3 vs 0.1.4 and moss-image 0.1.0 vs 0.1.1 on Runtime 0.35.2, one pod (release lane, proof-only). Sourced by
# gate.sh --cells. SDXL at its default 1 MP cold + two warm per release, then 4096^2 on 0.1.4 (0.1.3 runs out of
# memory there); MOSS-Image at 4 MP, 50 steps, cold + one warm per release.
ex() { jq -r '.execution_ms // 0 | . / 1000 | . * 10 | round / 10' "$1.json" 2>/dev/null; }
for i in 1 2 3; do cell "sdxl-old-$i" "sdxl/generate@0.1.3" prompt="$OBJECT, call $i, photo" seed:=$((R + 440 + i)); done
for i in 1 2 3; do cell "sdxl-new-$i" "sdxl/generate@0.1.4" prompt="$OBJECT, call $i on the candidate, photo" seed:=$((R + 450 + i)); done
cell sdxl-4k "sdxl/generate@0.1.4" prompt="$SCENE, wide photo, fine detail" seed:=$((R + 459)) megapixels:=16.0 aspect_ratio=1:1
for i in 1 2; do cell "moss-old-$i" "moss-image/generate@0.1.0" prompt="$CHAR, call $i, studio shot" seed:=$((R + 460 + i)) megapixels:=4.0 steps:=50; done
for i in 1 2; do cell "moss-new-$i" "moss-image/generate@0.1.1" prompt="$CHAR, call $i on the candidate, studio shot" seed:=$((R + 470 + i)) megapixels:=4.0 steps:=50; done
ok=PASS; for c in sdxl-old-1 sdxl-old-2 sdxl-old-3 sdxl-new-1 sdxl-new-2 sdxl-new-3 sdxl-4k moss-old-1 moss-old-2 moss-new-1 moss-new-2; do
  [ "$(run_get "$OUT" $c .status)" = completed ] || ok=FAIL; done
expect packages-ab $ok "sdxl 0.1.3 $(ex sdxl-old-1)/$(ex sdxl-old-2)/$(ex sdxl-old-3) s, 0.1.4 $(ex sdxl-new-1)/$(ex sdxl-new-2)/$(ex sdxl-new-3) s, 4096^2 $(ex sdxl-4k) s ($(run_get "$OUT" sdxl-4k .status)); moss 0.1.0 $(ex moss-old-1)/$(ex moss-old-2) s, 0.1.1 $(ex moss-new-1)/$(ex moss-new-2) s"
