# ltx-gpus-no-slower (xs-199, cr-348): [one GPU's first run, one GPU's warm run, two GPUs' runs...], $n the ledger's NOISE.
# Every two-GPU run is within $n of one GPU. The first against one GPU's first, each without its start-up (the executor's
# start, construction and every weight load: a new group's start is TensorD's, cr-348); every later one against the warm run.
def phases: [.report.measured[]?.stages[]?];
def startup: [phases[] | select(.name == "executor start" or .name == "construction" or (.name | test(" / loading "))) | .ms] | add // 0;
def work: .execution_ms - startup;
def pct(a; b): ((b / a - 1) * 1000 | round) / 10;
.[0] as $first | .[1] as $one | .[2:] as $twos
| [{run: 1, a: ($first | work), b: ($twos[0] | work), vs: "one GPU's first, both without start-up"}]
  + [range(1; $twos | length) as $i | {run: ($i + 1), a: $one.execution_ms, b: $twos[$i].execution_ms, vs: "one GPU warm"}]
| map(. + {ok: (.a > 0 and .b > 0 and .b <= .a * (1 + $n))})
| (if all(.ok) then "PASS" else "FAIL" end) + ": "
  + (map("2-GPU run \(.run) \(.b | round) ms against \(.vs) \(.a | round) ms (\(pct(.a; .b))%)") | join("; "))
