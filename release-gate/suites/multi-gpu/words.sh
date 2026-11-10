# A --cells hook beside `multi-gpu`: H3 over two GPUs from words alone, held as h3-u2-short is. That cell's prompt mentions
# its inline reference (`<Name>`), which Calcifer refuses as no box of the project since e1e0f0ad6 (11-04): release-gate
# 42749406243 ran nothing. The group's path (join, form, attention across the ranks) is the same without a reference.
#   gh workflow run release-gate.yml -f suites=multi-gpu -f cells=release-gate/suites/multi-gpu/words.sh -f only=h3-u2-words ...
source "$HERE/suites/execution/multi-gpu/h3-group.sh"
H3REF='[]'
h3_group h3-u2-words 100 prompt="$CHAR. $MOTION" seed:=$((R + 354)) duration_s:=5
