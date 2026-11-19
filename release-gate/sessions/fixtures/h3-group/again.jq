# cr-348 C (multi-gpu cells.sh `h3_again`), over h3-u2-short's record, h3-u2-warm's when it ran, then h3-u2-again's. Each
# sharded stage row of h3-u2-again's group call (degree 2) with a degree key: h3-u2-short's key for that stage (a prompt a
# word longer is in its text bucket), and the degree the machine's record gives from the earlier runs of the key: two until
# a warm time at two is recorded (a steady step, not cold), then one GPU unless it was refused (`refused_degree_1`) until a
# warm time at one is, then the faster. A degree keeps its first record.
def rows: [.calls[]?.measurements.execution // {} | select(.degree == 2) | .transport.stages // {}] | first // {}
  | with_entries(select(.value.key != null));
def kept($d): map(select(.degree == $d and .steady_ms != null and .cold == null)) | first | .steady_ms;
(.[-1] | rows) as $again
| (.[:-1] | map(rows)) as $before
| [$again | to_entries[] | .key as $stage | .value as $row
    | ($before | map(.[$stage] // empty | select(.key == $row.key))) as $same
    | ($same | kept(2)) as $two | ($same | kept(1)) as $one
    | {stage: $stage, degree: $row.degree, two: $two, one: $one,
       want: (if $two == null then 2 elif $one == null then (if $row.refused_degree_1 then 2 else 1 end)
              elif $one < $two then 1 else 2 end),
       short_key: ($before[0][$stage].key == $row.key)}] as $judged
| if ($judged | length) == 0 then "FAIL: h3-u2-again's record has no sharded stage with a degree key"
  elif all($judged[]; .short_key and .degree == .want) then "PASS \($judged | tojson)"
  else "FAIL \($judged | tojson)" end
