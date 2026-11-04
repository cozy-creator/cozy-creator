# A 15 s Turbo call and a standard 30-step call over two GPUs: a --cells run when the Ulysses exchange changes.
source "$HERE/suites/execution/multi-gpu/h3-group.sh"
h3_group h3-u2-turbo 300 prompt="<$NAME> $MOTION" seed:=$((R + 351)) duration_s:=15
h3_group h3-u2-std 300 prompt="<$NAME> $MOTION The camera slowly pushes in." seed:=$((R + 352)) duration_s:=15 turbo:=false steps:=30
