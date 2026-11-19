# cr-348 C (multi-gpu cells.sh `h3_again`), over h3-u2-short's record, h3-u2-warm's when it ran, then h3-u2-again's. Each
# sharded stage row of h3-u2-again's group call (degree 2) with a degree key: h3-u2-short's key for that stage, and the degree
# the machine's record gives: one GPU where an earlier run of the key recorded its time at two (a steady step, not cold) and
# one GPU fits (no `refused_degree_1`), else two.
def rows: [.calls[]?.measurements.execution // {} | select(.degree == 2) | .transport.stages // {}] | first // {}
  | with_entries(select(.value.key != null));
(.[-1] | rows) as $again
| (.[:-1] | map(rows)) as $before
| [$again | to_entries[] | .key as $stage | .value as $row
    | ($before | map(.[$stage] // empty | select(.key == $row.key))) as $same
    | any($same[]; .degree == 2 and .steady_ms != null and .cold == null) as $recorded
    | {stage: $stage, degree: $row.degree, want: (if $recorded and $row.refused_degree_1 == null then 1 else 2 end),
       short_key: ($before[0][$stage].key == $row.key)}] as $judged
| if ($judged | length) == 0 then "FAIL: h3-u2-again's record has no sharded stage with a degree key"
  elif all($judged[]; .short_key and .degree == .want) then "PASS \($judged | tojson)"
  else "FAIL \($judged | tojson)" end
