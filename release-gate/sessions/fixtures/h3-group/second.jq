# second (cr-326): read with `jq -rs --rawfile smi <nvidia-smi's index, uuid, name, power.limit, mig.mode.current> -f second.jq <records>`,
# the last two records the two 1-GPU runs made together. h3-group.sh's and suite multi-gpu's (h3-second).
def na: if . == "[Not Supported]" then "[N/A]" else . end;
($smi | split("\n") | map(select(length > 0) | split(", ") | map(sub("\\s+$"; ""))
  | {key: .[1], value: {gpu: (.[0] | tonumber), name: .[2], settings: "\(.[3] | na)W/\(.[4] | na)"}}) | from_entries) as $card
| ($card | to_entries | map(select(.value.gpu == 1)) | .[0].key // "") as $second
| [.[] | [.calls[]?.measurements.execution // {} | .gpus[]? | select(.uuid != "")]] as $runs
| ($runs[-2:] | map(select(length > 0 and all(.[]; .uuid == $second))) | length) as $alone
| [$runs[][] | select($card[.uuid] == null or $card[.uuid] != {gpu, name, settings})
  | "GPU \(.gpu) \(.uuid) recorded \(.name) \(.settings), nvidia-smi says \($card[.uuid] // {} | "GPU \(.gpu) \(.name) \(.settings)")"] as $bad
| if $second == "" then "FAIL second: nvidia-smi names no GPU 1"
  elif $alone == 0 then "FAIL second: inconclusive: neither 1-GPU run ran on GPU 1 alone (\($second))"
  elif ($bad | length) > 0 then "FAIL second: \($bad | join("; "))"
  else "PASS second: a run on GPU 1 alone (\($second)) recorded \($card[$second].name) \($card[$second].settings), as nvidia-smi -i says; \([$runs[][]] | length) GPU rows of \($runs | length) runs match"
  end
