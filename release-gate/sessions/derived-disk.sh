# derived-disk (cl-1081): on a short disk, derived pages (LoRA bakes) go before any model. A lane a live hold names is never
# dropped, and bakes were cached in their model's lane, so a model in use kept the bake of every LoRA stack it ran with:
# kamitsure's in-use H3 held 61 GB of bakes on a full 200 GB disk, and its store freed nothing for 3 h.
# Here: Qwen3.5 0.8B lands and stays idle; SDXL runs bare and then with four LoRA stacks (cl-683's), each a bake of its own;
# a file then leaves the disk half of Qwen-Image 2.1 int8 short past its reserve, and that model is installed. It must land
# with every earlier model still installed, the store's log naming derived pages freed and no model evicted. Before the fix
# the store evicted a model to make the room (or refused). params:
#   tree       the packages_refs checkout SDXL runs from (default rc)
#   runtime    a cozy-runtime rc-dev or dev-build run id: the pod's Runtime (the open RC's needs this session's cozy)
#   tensorfs   artifact:<cozy-creator run id>/<artifact>: the pod's TensorFS wheel
#   need_s     seconds the guard must buy (default 2700)
. "$(dirname "$0")/lib.sh"
TREE=$PACKAGES/$(param tree rc)
[ -d "$TREE/sdxl" ] || fail "no $TREE/sdxl"
wheel() {  # wheel <kind> <source>: the manylinux x86_64 wheel, fetched before renting
  local d=$OUT/wheels/$1; mkdir -p "$d"
  case $2 in
    artifact:*/*) local a=${2#artifact:}; gh run download "${a%%/*}" -R cozy-creator/cozy-creator -n "${a#*/}" -D "$d" > /dev/null 2>&1 || return 1 ;;
    *) gh run download "$2" -R cozy-creator/cozy-runtime -p 'cozy-runtime-*' -D "$d" > /dev/null 2>&1 || return 1 ;;
  esac
  find "$d" -name '*manylinux*x86_64*.whl' | head -1
}
RT=$(wheel runtime "$(param runtime)") && [ -n "$RT" ] || fail "no Runtime wheel from $(param runtime)"
TF=$(wheel tensorfs "$(param tensorfs)") && [ -n "$TF" ] || fail "no TensorFS wheel from $(param tensorfs)"
say "Runtime ${RT##*/}, TensorFS ${TF##*/}, TensorD $(cozy --version)"
kernels "$TREE/sdxl"
# ssh for the ballast file (`cozy machine ssh` needs a key the rental carries)
[ -f ~/.ssh/id_ed25519.pub ] || ssh-keygen -q -t ed25519 -N '' -f ~/.ssh/id_ed25519 || fail "ssh-keygen"
RENT_ARGS=--ssh
quote "$(param need_s 2700)"
rent

step update machine update "$NAME" --tensord "$(command -v cozy)" --runtime "$RT" --tensorfs "$TF" -y || fail "the update"
show() { cozy machine show "$NAME" --json 2> /dev/null; }
installed() { show | jq -r '[.status.models[] | select(any(.checkpoints[]; .complete)) | .repository] | sort | join(" ")'; }
step idle model install qwen/qwen3.5-0.8b --on "$NAME" -y || fail "Qwen3.5 0.8B did not install"
cd "$TREE" || fail "no $TREE"
I=ostris/ikea-lora-sdxl B=proomptengineer/balloon-lora-sdxl
step sdxl-bare run ./sdxl/generate prompt="a brass compass on a walnut desk" seed:=1081 --lane int8 --on "$NAME" || fail "SDXL"
n=0
for stack in "--lora $I" "--lora $B" "--lora $I,0.6 --lora $B,0.8" "--lora $I,0.5"; do
  n=$((n + 1))
  # shellcheck disable=SC2086 # the stack's flags
  step "sdxl-lora-$n" run ./sdxl/generate prompt="PEBalloonStyle, a brass compass as a flat-pack kit" seed:=$((1081 + n)) --lane int8 $stack --on "$NAME" ||
    fail "SDXL with $stack"
done
cd "$OUT" || fail "no $OUT"
before=$(installed)
read -r free total store listed < <(show | jq -r '.status | "\(.disk_free_bytes) \(.disk_total_bytes) \(.disk_models_bytes) \(.models_bytes)"')
derived=$((store - listed))
say "installed: $before; store $((store >> 20)) MiB, $((listed >> 20)) MiB of it in listed models: $((derived >> 20)) MiB of bakes"
# Qwen-Image 2.1 int8 is about 12 GB: the disk is left half of it short past TensorFS's reserve (2% of the disk, 1 to 10 GiB).
reserve=$((total / 50)); [ "$reserve" -ge $((1 << 30)) ] || reserve=$((1 << 30)); [ "$reserve" -le $((10 << 30)) ] || reserve=$((10 << 30))
need=$((6 << 30))
[ "$derived" -gt $((need + (1 << 30))) ] || fail "setup: $((derived >> 20)) MiB of bakes, fewer than the $((need >> 20)) MiB the install must take from them"
ballast=$((free - reserve - need))
if [ "$ballast" -gt 0 ]; then
  cozy machine ssh "$NAME" "fallocate -l $ballast /var/lib/cozy/cl1081-ballast && df -B1 --output=avail /var/lib/cozy | tail -1" > "$OUT/ballast.txt" 2>&1 ||
    fail "the ballast: $(tail -c 300 "$OUT/ballast.txt")"
fi
say "ballast $((ballast >> 20)) MiB: free now $(( $(show | jq -r .status.disk_free_bytes) >> 20 )) MiB, reserve $((reserve >> 20)) MiB"
step big model install qwen/qwen-image-2.1 --lane int8 --on "$NAME" -y; landed=$?
after=$(installed)
cozy machine logs "$NAME" tensord -n 0 > "$OUT/tensord.log" 2>&1
freed=$(grep -c 'of derived pages, least recently used first' "$OUT/tensord.log")
evicted=$(grep -c 'tensorfs: evicted' "$OUT/tensord.log")
grep -E 'of derived pages|tensorfs: evicted|tensorfs: freed|ended the idle|disk is short|doesn.t fit' "$OUT/tensord.log" | cut -c1-300 >> "$R"
lost=$(comm -23 <(tr ' ' '\n' <<< "$before" | sort) <(tr ' ' '\n' <<< "$after" | sort) | paste -sd' ')
say "after: $after; derived pages freed $freed times, $evicted models evicted, lost: ${lost:-none}"
[ "$landed" = 0 ] && [[ " $after " == *" qwen/qwen-image-2.1 "* ]] || fail "Qwen-Image 2.1 did not land: $(tail -c 300 "$OUT/big.txt")"
[ -z "$lost" ] && [ "$evicted" = 0 ] || fail "a model went for room the bakes held: lost ${lost:-none}, $evicted evictions"
[ "$freed" -gt 0 ] || fail "no derived pages were freed"
say "PASS: Qwen-Image 2.1 landed on the bakes' room; every model kept"
