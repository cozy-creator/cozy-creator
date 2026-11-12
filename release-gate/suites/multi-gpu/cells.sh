# Suite `multi-gpu`, every release: a 5 s Turbo H3 over two GPUs, a new prompt and seed, on one card of each kernel family
# whose group takes its own attention path (`skus`). A group opens every kernel of a site's walk, where one GPU stops at its
# first: sm120's Sage3 kernels faulted every group from 0.35.5 to 0.39.5 while one GPU and the 3090 passed (corteo).
# qwen-replicas needs four GPUs and stays a --cells run when sharding changes, as does h3-ulysses.sh's 15 s and 30-step.
source "$HERE/suites/execution/multi-gpu/h3-group.sh"
h3_group h3-u2-short 100 prompt="<$NAME> $MOTION" seed:=$((R + 353)) duration_s:=5
# The same group again, warm (cr-268): Sol's per-length object built by the first call serves every sparse step, none
# dense (`*warm*` in h3_group). Runtime 0.45.0 ran a group's first call at a length all dense over two H100s (47001:
# sparse 0 of 2304, so h3-u2-short-sparse fails) and exchanged over NCCL on every call (-exchange).
h3_group h3-u2-warm 100 prompt="<$NAME> $SCENE" seed:=$((R + 354)) duration_s:=5
