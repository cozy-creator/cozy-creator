#!/bin/bash
# quantize-skew (proof only, cl-1326, never merged): `cozy model quantize` with this session's cozy as the pod's TensorD, (1) on the
# image's released Runtime and TensorFS (0.48.0 / 0.27.0: a machine that hasn't updated), then (2) on the RC's (new×new).
# packages_refs: one checkout `tree` holding qwen3. params: model, lane (int8), runtime_url, tensorfs_url (the RC's wheels).
. "$(dirname "$0")/lib.sh"
MODEL=$(param model huihui-ai/huihui-qwen3.5-0.8b-abliterated@0.1.0) TREE=$(param tree q) LANE=$(param lane int8)
PKG=$PACKAGES/$TREE/qwen3
[ -f "$PKG/pyproject.toml" ] || fail "no qwen3 in packages_refs checkout '$TREE'"
RW=$(wheel_from "$OUT/wheels/runtime" "$(param runtime_url)") && [ -n "$RW" ] || fail "no Runtime wheel"
TW=$(wheel_from "$OUT/wheels/tensorfs" "$(param tensorfs_url)") && [ -n "$TW" ] || fail "no TensorFS wheel"
kernels "$PKG"
quote "$(param seconds 900)"
rent
on() { cozy machine show "$NAME" 2>&1 | grep -E '^(version|runtime|tensorfs) ' | paste -sd' ' | tr -s ' '; }
step tensord machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "the pod's TensorD"
say "old: $NAME $(on)"
step pull model install "$MODEL" --on "$NAME" || fail "pull: $(tail -c 400 "$OUT/pull.txt")"
step old model quantize "$MODEL" --lane "$LANE" --package "$PKG" --on "$NAME" || fail "old: $(tail -c 400 "$OUT/old.txt")"
say "SKEW old: $MODEL $LANE quantized by TensorD $(cozy --version) on $(on)"
step candidate machine update "$NAME" --runtime "$RW" --tensorfs "$TW" -y || fail "candidate: $(tail -c 400 "$OUT/candidate.txt")"
say "new: $NAME $(on)"
step new model quantize "$MODEL" --lane "$LANE" --package "$PKG" --on "$NAME" || fail "new: $(tail -c 400 "$OUT/new.txt")"
say "SKEW new: $MODEL $LANE quantized on $(on)"
