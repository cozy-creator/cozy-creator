# group-disk (cl-1076): an idle 2-GPU group's model goes for room a render needs. Each rank opens what it loads under its own
# pid (Runtime's `_follower_load` -> `_prepare` -> `tensorfs.open`), and TensorD counted only a group's leader as the idle
# executor (`Pool::idle_executors`), so an idle group's model was never evictable. One 2-GPU rental with SSH, red then green,
# each the same: H3 int8 over both GPUs leaves its group idle (and its Turbo LoRA's bake on disk), a file leaves 1 GiB free past
# the reserve, and LTX 2.5 (71.5 GB, more than the bake and every unheld model can free) is installed.
#   red    the TensorD of dev-build `red` (the RC without the fix): H3's group is never ended, the install is refused
#   green  the session's cozy: the group ends, H3 is evicted, LTX 2.5 lands
# params:
#   red      a cozy-creator dev-build run id: red's TensorD
#   need_s   seconds the guard must buy (default 3600)
# Dispatch: sku=a40 disk=150 cap=4 runtime_ref=rc/0.26.0 cozy=<the fix's dev-build run id> params='{"red": "<run id>"}'
. "$(dirname "$0")/lib.sh"
gh run download "$(param red)" -R cozy-creator/cozy-creator -p 'cozy-*' -D "$OUT/red" > /dev/null 2>&1 \
  && install -m 0755 "$OUT"/red/cozy-*/cozy-linux-x86_64 "$OUT/red-cozy" || fail "no cozy from dev-build $(param red)"
say "red $("$OUT/red-cozy" --version), green $(cozy --version)"
# ssh for the ballast file (`cozy machine ssh` needs a key the rental carries)
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS="--ssh --gpus 2"
quote "$(param need_s 3600)"
rent

show() { cozy machine show "$NAME" --json 2> /dev/null; }
installed() { show | jq -r '[.status.models[] | select(any(.checkpoints[]; .complete)) | .repository] | sort | join(" ")'; }
for i in $(seq 1 20); do cozy machine ssh "$NAME" true < /dev/null > /dev/null 2>&1 && break; [ "$i" = 20 ] && fail "no SSH into $NAME"; sleep 15; done

# phase <name> <tensord>: H3 over both GPUs, then the install that needs its room. Sets LANDED (0: landed) and ENDED (log lines
# naming H3's idle executors ended).
phase() {
  local p=$1 free total reserve ballast
  local rt=(); [ -z "${RUNTIME_WHEEL:-}" ] || rt=(--runtime "$RUNTIME_WHEEL")
  step "$p-update" machine update "$NAME" --tensord "$2" "${rt[@]}" -y || fail "$p: the update"
  jq -n --argjson s "$RANDOM$RANDOM" '{summary: {references: [], text: "A steady tracking shot of a courier cycling a cargo bike through a narrow cobbled street after rain, past cafe tables and a flower stall, into a sunlit square."},
    beats: [{duration: 15, description: {references: [], text: "The courier rings the bell at the flower stall and rolls down the slope into the square."}}],
    soundscape: {references: [], text: "Tires hissing on wet cobbles, a bicycle bell, cafe chatter."}, seed: $s}' > "$OUT/$p-h3.req.json"
  step "$p-h3" run minimax-h3/generate --input "$OUT/$p-h3.req.json" --gpus 2 --lane int8 --on "$NAME" || fail "$p: H3 over both GPUs"
  read -r free total < <(show | jq -r '.status | "\(.disk_free_bytes) \(.disk_total_bytes)"')
  reserve=$((total / 50)); [ "$reserve" -ge $((1 << 30)) ] || reserve=$((1 << 30)); [ "$reserve" -le $((10 << 30)) ] || reserve=$((10 << 30))
  # 1 GiB free past the reserve: LTX's 71.5 GB needs H3's room, beyond the bake's and the LoRAs'.
  ballast=$((free - reserve - (1 << 30)))
  [ "$ballast" -gt 0 ] && { cozy machine ssh "$NAME" "fallocate -l $ballast /var/lib/cozy/cl1076-ballast" > "$OUT/$p-ballast.txt" 2>&1 \
    || fail "$p: the ballast: $(tail -c 300 "$OUT/$p-ballast.txt")"; }
  say "$p: installed $(installed); ballast $((ballast >> 20)) MiB, free $(( $(show | jq -r .status.disk_free_bytes) >> 20 )) MiB"
  step "$p-ltx" model install lightricks/ltx-2.5 --on "$NAME" -y; LANDED=$?
  cozy machine ssh "$NAME" "rm -f /var/lib/cozy/cl1076-ballast" > /dev/null 2>&1 || fail "$p: removing the ballast"
  cozy machine logs "$NAME" tensord -n 0 --since 30m > "$OUT/$p-tensord.log" 2>&1
  ENDED=$(grep -c 'ended the idle executors holding .*minimax-h3' "$OUT/$p-tensord.log")
  grep -E 'ended the idle|tensorfs: evicted|doesn.t fit|machine_disk_full' "$OUT/$p-tensord.log" | cut -c1-300 >> "$R"
  say "$p: install exit $LANDED, H3's idle executors ended $ENDED times; installed $(installed)"
}

phase red "$OUT/red-cozy"
[ "$LANDED" != 0 ] && [ "$ENDED" = 0 ] || fail "red: the install landed or H3's group ended without the fix: the pair can't tell"
phase green "$(command -v cozy)"
[ "$LANDED" = 0 ] && [ "$ENDED" -gt 0 ] && [[ " $(installed) " != *" minimaxai/minimax-h3 "* ]] \
  || fail "green: the install $LANDED, H3's group ended $ENDED times, installed $(installed)"
say "PASS: red refused with H3's idle group kept; green ended the group, evicted H3 and landed LTX 2.5"
