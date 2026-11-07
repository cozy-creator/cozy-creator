# upload-proofs: a pod's uploads under its run capability, each request with a fresh DPoP proof in the Hub's time
# (cli-release 42224725083: a host 665 s fast had every redemption refused, invalid_dpop_proof). One rental, no GPU work:
#   1. the pod's TensorD becomes the session's cozy; a private commit of download_mb MB is uploaded from the runner;
#   2. two uploads --on the pod in a row;
#   3. a download of that commit --on the pod, a second download of it waiting on the first, and an upload --on the pod
#      while they run: the upload's capability grants more, so the pod redeems again mid-download;
#   4. every run completes; the pod's clock against the Hub's, and its redemptions, from its log.
# The throwaway models are deleted after. params:
#   download_mb  the downloaded commit's size (2048): long enough for the upload to land while it downloads
. "$(dirname "$0")/lib.sh"
MB=$(param download_mb 2048)
TS=$(date -u +%m%d%H%M) A=$(cozy --json whoami | jq -r .account)
[ -n "$A" ] && [ "$A" != null ] || fail "not signed in"
MD=$A/proofs-d-$TS MU=$A/proofs-u-$TS
# The weights stay out of $OUT, which is published.
W=$(mktemp -d)
tensors() { python3 -I -c 'import json,struct,sys,os
n=int(sys.argv[2])*2**20//4; h=json.dumps({"w":{"dtype":"F32","shape":[n],"data_offsets":[0,n*4]}}).encode(); h+=b" "*(-len(h)%8)
with open(sys.argv[1],"wb") as f:
    f.write(struct.pack("<Q",len(h))+h)
    for _ in range(n*4//2**20): f.write(os.urandom(2**20))
    f.write(os.urandom(n*4%2**20))' "$1" "$2"; }
# A step's output is its NDJSON with the CLI's own lines among them.
records() { grep '^{' "$OUT/$1"; }
commit() { records "$1.txt" | jq -r 'select(.type? == "run.completed") | .run.result.models[0].published.commit // empty' | tail -1; }
settled() { records "$1" | jq -r 'select(.run.settled_at_ms?) | .run.settled_at_ms' | tail -1; }
cleanup() { rm -rf "$W"; for m in $MD $MU; do cozy model delete "$m" -y >> "$OUT/delete.txt" 2>&1; done; }
quote 1800
rent
trap 'cleanup; finish' EXIT

# 1. The session's TensorD on the pod; the commit its downloads read, from the runner.
step update machine update "$NAME" --tensord "$(command -v cozy)" -y || fail "update"
v=$(cozy machine show "$NAME" --json | jq -r .status.version)
[ "$v" = "$(cozy --version | awk '{print $2}')" ] || fail "the pod runs TensorD $v, not the session's"
tensors "$W/d.safetensors" "$MB"
step upload-d --json model upload "$W/d.safetensors" "$MD" --lane f32 --source-format as-is || fail "upload from the runner"
CD=$(commit upload-d); [ -n "$CD" ] || fail "upload-d named no commit"
say "$MD@$CD ($MB MB, private)"

# 2. Two uploads on the pod, in a row.
for i in 1 2; do
  tensors "$W/u$i.safetensors" 1
  step "upload-$i" --json model upload "$W/u$i.safetensors" "$MU" --lane f32 --source-format as-is --on "$NAME" || fail "upload $i --on the pod"
  [ -n "$(commit "upload-$i")" ] || fail "upload $i named no commit"
  say "upload $i: $MU@$(commit "upload-$i")"
done

# 3. An upload while one download runs and another waits on it.
cozy --json model download "$MD@$CD" --on "$NAME" > "$OUT/download-1.ndjson" 2> "$OUT/download-1.err" & D1=$!
until records download-1.ndjson | jq -e 'select(.progress.stage? | strings | startswith("downloading"))' > /dev/null 2>&1; do
  kill -0 $D1 2> /dev/null || fail "download 1 ended before it downloaded: $(tail -c 300 "$OUT/download-1.ndjson")"; sleep 0.5
done
cozy --json model download "$MD@$CD" --on "$NAME" > "$OUT/download-2.ndjson" 2> "$OUT/download-2.err" & D2=$!
tensors "$W/u3.safetensors" 1
step upload-3 --json model upload "$W/u3.safetensors" "$MU" --lane f32 --source-format as-is --on "$NAME" || fail "upload 3 --on the pod"
wait $D1 || fail "download 1: $(tail -c 300 "$OUT/download-1.ndjson")"
wait $D2 || fail "download 2: $(tail -c 300 "$OUT/download-2.ndjson")"
U=$(settled upload-3.txt)
say "upload 3 settled at $U; downloads at $(settled download-1.ndjson), $(settled download-2.ndjson)"
[ "$U" -lt "$(settled download-1.ndjson)" ] || fail "download 1 ended before upload 3: raise download_mb"
for d in 1 2; do say "download $d: $(records "download-$d.ndjson" | jq -c 'select(.type? == "run.completed") | .run.result.models[0].bytes')"; done

# 4. The pod's clock against the Hub's: its first progress stamp against the runner's Calcifer, the runner's against the Hub's Date.
pod=$(records upload-1.txt | jq -rs '(map(select(.run.accepted_at_ms?)) | first | .run.accepted_at_ms) as $a | (map(select(.progress.at_ms?)) | first | .progress.at_ms) - $a')
hub=$(curl -sI "${COZY_HUB_URL:-https://tensorhub.com}/.well-known/oauth-protected-resource" | tr -d '\r' | sed -n 's/^[Dd]ate: //p')
runner=$(( $(date +%s) - $(date -d "$hub" +%s) ))
say "CLOCK the pod's first stamp is $pod ms past the runner's acceptance; the runner reads ${runner}s past the Hub's Date"
cozy machine logs "$NAME" -n 0 > "$OUT/tensord.log" 2>&1
grep -E "redeemed the run capability|refused the run capability|clock is" "$OUT/tensord.log" | tail -10 | sed 's/^/    /' >> "$R"
! grep -q "refused the run capability" "$OUT/tensord.log" || fail "the Hub refused the pod's capability"
say "PASS: two uploads in a row and one beside a download and a run waiting on it, each under the pod's own proofs"
