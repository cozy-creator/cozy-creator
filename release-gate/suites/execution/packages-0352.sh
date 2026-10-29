# krea-2 0.1.1 / qwen-image-2 0.1.9 / minimax-h3 0.1.9 on Runtime 0.35.2 (release lane, proof-only). Sourced by gate.sh
# --cells after the runtime suite's selected cells (h3 and qwen on their pinned candidates): Krea 2 Turbo at 2048^2,
# cold and two warm, on krea-2 0.1.0 then on 0.1.1, on this pod; the decode stage is what #701 moves.
dec() { jq -r '[.. | objects | select(has("stages")) | .stages | objects | ."decoding image".total_ms? // empty] | add // 0 | . / 1000 | . * 100 | round / 100' "$1.json" 2>/dev/null; }
ex() { jq -r '.execution_ms // 0 | . / 1000 | . * 10 | round / 10' "$1.json" 2>/dev/null; }
for i in 1 2 3; do cell "krea-old-$i" "krea-2/generate_turbo@0.1.0" prompt="$OBJECT, Krea call $i, studio photo" seed:=$((R + 420 + i)); done
for i in 1 2 3; do cell "krea-pkg-$i" "krea-2/generate_turbo@0.1.1" prompt="$OBJECT, Krea call $i on the candidate, studio photo" seed:=$((R + 430 + i)); done
if [ -f krea-old-3.json ] && [ -f krea-pkg-3.json ]; then
  ok=PASS; for c in krea-old-1 krea-old-2 krea-old-3 krea-pkg-1 krea-pkg-2 krea-pkg-3; do [ "$(run_get "$OUT" $c .status)" = completed ] || ok=FAIL; done
  expect krea-pkg-ab $ok "0.1.0 exec $(ex krea-old-1)/$(ex krea-old-2)/$(ex krea-old-3) s decode $(dec krea-old-2)/$(dec krea-old-3) s; 0.1.1 exec $(ex krea-pkg-1)/$(ex krea-pkg-2)/$(ex krea-pkg-3) s decode $(dec krea-pkg-2)/$(dec krea-pkg-3) s"
fi
