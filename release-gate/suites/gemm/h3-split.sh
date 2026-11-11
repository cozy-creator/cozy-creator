# xs-177 step 1, a `cells=` hook on one H100: H3 Turbo 15 s on one GPU pinned to 8 bits (the int8 lane and Kitchen's int8
# attention: bench-results' 78.0 s row) and at the card's defaults (51.3 s); the int8 lane alone when the defaults took
# another lane, so the gap splits into lane and attention; then the attention bench at H3's shapes, one GPU and one of four.
H3=$(pkg minimax-h3 generate)
H3_ARGS=(beats:='[{"description":"[Shot 1]\nA slow bank of fog crosses the bay. The camera tracks along the cliff edge while gulls ride the wind. Spray climbs the rocks below the gallery.","duration":15}]'
  soundscape="Wind and ocean surf, with waves splashing against the rocks." turbo:=true seed:=20261301 --gpus=1)
cell h3s-8bit "$H3" "${H3_ARGS[@]}" --lane int8 --attention-kernel transformer=kitchen-sol
cell h3s-default "$H3" "${H3_ARGS[@]}"
h3s_lanes() { run_get "$OUT" "$1" '[.calls[]?.measurements.models[]? | select(.slot == "base_model") | .lane] | unique | join(",")'; }
h3s_default=$(h3s_lanes h3s-default)
log "h3s lanes: default ${h3s_default:-?}, 8-bit $(h3s_lanes h3s-8bit)"
[ -n "$h3s_default" ] && [ "$h3s_default" = "$(h3s_lanes h3s-8bit)" ] || cell h3s-int8 "$H3" "${H3_ARGS[@]}" --lane int8
cell h3s-attention "$HERE/suites/attention/drive/attention_speed" \
  kernels=kitchen-sol,kitchen-int8,sageattention,sol-attn-sage2-fixed-kmean \
  points=dense-d128-111598-h56,sparse-d128-111598-h56,dense-d128-111598-h14,sparse-d128-111598-h14 seed:=$((R + 772)) --gpus=1
