# e2e/cli/egress.sh: what a request can make a computer read (xs-196 D). Linux, the local machine.
#   egress          No URL a run or a Hub answer names reaches a link-local, loopback or private address, at its first hop
#                   or after a redirect from an origin this computer declared local (E2E_REDIRECT, lib.sh): Calcifer's link
#                   import and TensorD's model sources alike. A run names no path on the machine as a model
#                   (local_model_path_refused). A failed fetch of a presigned URL says, and logs, none of its query.
#   hostile_inputs  A video input that names other files or the network (a concat list, a DASH manifest, an HLS
#                   playlist), sent as video/mp4 past Calcifer's sniff, is refused by the Runtime's decoder, which opens
#                   none of them; a real clip beside them is read.
#   private_weights A model the local machine just uploaded, private, read by a run (cl-1009, the release gate's qwen-lora flow):
#                   its run capability puts its executor on TensorD's own Hub gateway (cl-675), an origin TensorD declares
#                   local for its executors and nothing a run or Hub answer names. 0.26.0 refused it at the model's load.
#   token_scope     The token `tensorfs.configure` was given reaches the default endpoint's origin and no other (xs-202, the
#                   11-15 design audit: TensorFS 0.25.1 sent it to whatever endpoint a call named). The candidate TensorFS, no machine.
# TensorD is asked as any client holding the machine's authorization asks it (egress.py, a key `machine authorize` admits).

# redirector_start <log>: E2E_REDIRECT, which lib.sh declared local, answering /missing… 404 and anything else with a
# redirect to the metadata address; each path asked goes in <log>.
REDIRECTOR=''
redirector_start() {
  : > "$1"
  python3 -I - "${E2E_REDIRECT##*:}" "$1" <<'PY' &
import http.server, sys
port, log = int(sys.argv[1]), sys.argv[2]
class Redirect(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/missing-ready":
            with open(log, "a") as out:
                out.write(self.path + "\n")
        self.send_response(404 if self.path.startswith("/missing") else 302)
        if not self.path.startswith("/missing"):
            self.send_header("Location", "https://169.254.169.254/latest/meta-data/")
        self.send_header("Content-Length", "0")
        self.end_headers()
    do_HEAD = do_GET
    def log_message(self, *_):
        pass
http.server.ThreadingHTTPServer(("127.0.0.1", port), Redirect).serve_forever()
PY
  REDIRECTOR=$!
  for _ in $(seq 50); do curl -s -o /dev/null "$E2E_REDIRECT/missing-ready" && break; sleep 0.2; done
}
redirector_stop() { [ -z "$REDIRECTOR" ] || kill "$REDIRECTOR" 2> /dev/null || true; REDIRECTOR=''; }

# machine_client <dir> <egress.py command and arguments…>: a fresh key authorized on local, egress.py run with it, the key
# removed. Its answers go in <dir>/machine.json.
machine_client() {
  local dir=$1 pem=$1/signer.pem key='' code=0
  shift
  for _ in $(seq 100); do
    openssl genpkey -algorithm ed25519 -out "$pem" 2> /dev/null || { check "egress: openssl could not make an Ed25519 key"; return 1; }
    key=$(openssl pkey -in "$pem" -pubout -outform DER | tail -c 32 | base64 | tr '+/' '-_' | tr -d '=\n')
    [[ $key == -* ]] || break
  done
  cozy --json machine authorize "$key" > "$dir/authorize.json" || { check "egress: machine authorize failed"; return 1; }
  cozy --json machine show local > "$dir/local.json" || check "egress: machine show local failed"
  uv run -q --no-project --with cryptography python -I "$here/egress.py" "$1" "$pem" "$(jq -r .address "$dir/local.json")" \
    "$(jq -r .fingerprint "$dir/local.json")" "$dir/machine.json" "${@:2}" || code=$?
  cozy --json machine authorize "$key" --remove > /dev/null || check "egress: machine authorize --remove failed"
  [ "$code" = 0 ] || check "egress: the machine's requests failed (exit $code)"
  return "$code"
}

egress() {
step "no fetch reaches the metadata address or the machine's files (xs-196 D)"
[ -n "$LOCAL" ] || return 0
[ -n "${E2E_REDIRECT:-}" ] || { check "egress: lib.sh declared no redirector (E2E_REDIRECT)"; return; }
local dir=$OUT/egress secret=e2e-presigned-$RANDOM$RANDOM name source leaked
rm -rf "$dir"; mkdir -p "$dir"
redirector_start "$dir/asked.log"

# Calcifer: a link a person imports.
for name in metadata redirect presigned; do
  case $name in
    metadata) source=http://169.254.169.254/latest/meta-data/ ;;
    redirect) source=$E2E_REDIRECT/picture.png ;;
    presigned) source="$E2E_REDIRECT/missing.png?X-Amz-Signature=$secret" ;;
  esac
  project_api "$RUN_PROJECT" /imports -X POST -H 'Content-Type: application/json' -d "$(jq -n --arg source "$source" '{source: $source}')" \
    > "$dir/import-$name.json" 2>&1 && check "egress: Calcifer imported $name ($source)"
done
grep -q link-local "$dir/import-metadata.json" || check "egress: the metadata address was not refused as link-local: $(head -c 300 "$dir/import-metadata.json")"
grep -q link-local "$dir/import-redirect.json" || check "egress: a redirect to the metadata address was not refused as link-local: $(head -c 300 "$dir/import-redirect.json")"
grep -qx /picture.png "$dir/asked.log" || check "egress: the declared origin was never asked, so its redirect proves nothing: $(head -c 200 "$dir/asked.log")"

# TensorD: model sources.
if machine_client "$dir" models "$E2E_REDIRECT" "$secret"; then
  for name in path added; do
    J --arg n "$name" '.[$n].refusal.code == "local_model_path_refused"' "$dir/machine.json" \
      || check "egress: a model source naming /etc/hostname ($name) was not refused local_model_path_refused: $(jq -c --arg n "$name" '.[$n]' "$dir/machine.json" | head -c 300)"
  done
  for name in metadata redirect; do
    J --arg n "$name" '.[$n].outcome.status == "failed" and (.[$n] | tostring | contains("link-local"))' "$dir/machine.json" \
      || check "egress: TensorD's $name model source did not fail as link-local: $(jq -c --arg n "$name" '.[$n]' "$dir/machine.json" | head -c 400)"
  done
  grep -qx /model.safetensors "$dir/asked.log" || check "egress: TensorD never asked the declared origin, so its redirect proves nothing"
  J '.presigned.outcome.status == "failed"' "$dir/machine.json" || check "egress: the presigned source did not fail: $(jq -c .presigned "$dir/machine.json" | head -c 300)"
fi
redirector_stop
# The presigned query appears in no answer and no log of this computer's cozy (the run's own spec keeps the source it was sent).
leaked=$({ grep -lsI "$secret" "$dir"/import-*.json; jq -c '.presigned' "$dir/machine.json" | grep -l "$secret"; grep -rlsI --include='*.log*' "$secret" "$HOME/.cozy"; } 2> /dev/null | head -3 || true)
[ -z "$leaked" ] || check "egress: the presigned query is in $leaked"
echo "egress: $(jq -c 'map_values(.refusal.code // .outcome.status)' "$dir/machine.json" 2> /dev/null || true)"
}

token_scope() {
step "a configured TensorFS token reaches its own endpoint only (xs-202)"
local dir=$OUT/token-scope with mode
[ -n "${CAND_TFS:-}" ] || { echo "token scope: skipped, this run names no TensorFS"; return 0; }
if [ -f "$CAND_TFS" ]; then with=$CAND_TFS; else with=tensorfs==${CAND_TFS#v}; fi
rm -rf "$dir"; mkdir -p "$dir"
for mode in configured environment; do
  uv run -q --no-project --with "$with" python -I "$here/token_scope.py" "$mode" > "$dir/$mode.json" 2> "$dir/$mode.err" \
    || { check "token scope ($mode): $(tail -c 300 "$dir/$mode.err")"; continue; }
  J '(.default | length > 0 and all(. == "Bearer token-a")) and (.named | length > 0 and all(. == "Bearer token-a"))' "$dir/$mode.json" \
    || check "token scope ($mode): the default endpoint did not get the configured token: $(cat "$dir/$mode.json")"
  J '.other | length > 0 and all(. == null)' "$dir/$mode.json" \
    || check "token scope ($mode): another origin got the configured token, or was never asked: $(cat "$dir/$mode.json")"
  J '.other_own | length > 0 and all(. == "Bearer token-b")' "$dir/$mode.json" \
    || check "token scope ($mode): another origin did not get the call's own token: $(cat "$dir/$mode.json")"
  echo "token scope ($mode): $(cat "$dir/$mode.json")"
done
}

hostile_inputs() {
step "a video input naming other files or the network: the Runtime opens none of them (xs-196 D)"
[ -n "$LOCAL" ] || return 0
[ -n "${E2E_REDIRECT:-}" ] || { check "hostile inputs: lib.sh declared no redirector (E2E_REDIRECT)"; return; }
local dir=$OUT/hostile-inputs reader
rm -rf "$dir"; mkdir -p "$dir"; mkfifo "$dir/marker"
ffmpeg -v error -y -f lavfi -i testsrc2=size=160x120:rate=24:duration=2 -c:v libx264 -pix_fmt yuv420p "$dir/control.mp4"
printf "ffconcat version 1.0\nfile '%s'\n" "$dir/marker" > "$dir/concat.mp4"
printf '<?xml version="1.0"?>\n<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT2S" minBufferTime="PT1S" profiles="urn:mpeg:dash:profile:isoff-on-demand:2011">\n<Period><AdaptationSet mimeType="video/mp4"><Representation id="1" bandwidth="1000" codecs="avc1.42c00d" width="64" height="64"><BaseURL>%s/dash-segment.mp4</BaseURL><SegmentBase indexRange="0-100"/></Representation></AdaptationSet></Period></MPD>\n' "$E2E_REDIRECT" > "$dir/dash.mp4"
printf '#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\n%s/hls-segment.ts\n#EXT-X-ENDLIST\n' "$E2E_REDIRECT" > "$dir/hls.mp4"
redirector_start "$dir/asked.log"
# The writer waits for a reader of the FIFO: $reader is its `timeout`, so the kill below ends it and its sh. It wrote nothing to the suite's
# output: a subshell around it was killed alone, and the writer held the job step's pipe 900 s after the suite ended (cl-1128).
timeout 900 sh -c 'echo read > "$1" && touch "$2"' _ "$dir/marker" "$dir/opened" > /dev/null 2>&1 < /dev/null & reader=$!
if machine_client "$dir" media "$here/clip" control="$dir/control.mp4" concat="$dir/concat.mp4" dash="$dir/dash.mp4" hls="$dir/hls.mp4"; then
  J '.control.outcome.status == "succeeded" and (.control.outcome.result | tostring | test("\"seconds\": *[12](\\.[0-9]+)?[,}]"))' "$dir/machine.json" \
    || check "hostile inputs: the real clip was not read as 2 s: $(jq -c .control "$dir/machine.json" | head -c 400)"
  J '.concat.outcome.status == "failed" and (.concat | tostring | contains("media_format_refused"))' "$dir/machine.json" \
    || check "hostile inputs: the concat list was not refused media_format_refused: $(jq -c .concat "$dir/machine.json" | head -c 400)"
  for name in dash hls; do
    J --arg n "$name" '.[$n].outcome.status == "failed" and (.[$n] | tostring | test("media_format_refused|media_malformed"))' "$dir/machine.json" \
      || check "hostile inputs: the $name input was not refused by the decoder: $(jq -c --arg n "$name" '.[$n]' "$dir/machine.json" | head -c 400)"
  done
fi
sleep 2
redirector_stop
[ ! -s "$dir/asked.log" ] || check "hostile inputs: a decoded input reached the network: $(head -c 200 "$dir/asked.log")"
[ ! -e "$dir/opened" ] || check "hostile inputs: a decoded input opened the file its concat list names"
kill "$reader" 2> /dev/null || true
wait "$reader" 2> /dev/null || true
echo "hostile inputs: $(jq -c 'map_values(.refusal.code // .outcome.reason.code // .outcome.status)' "$dir/machine.json" 2> /dev/null || true)"
}

private_weights() {
step "a run reads a model this machine just uploaded, private, through TensorD's gateway (cl-1009)"
[ -n "$LOCAL" ] || return 0
[ -n "${E2E_HUB_LOG:-}" ] && [ -n "${E2E_HUB_EMAIL:-}" ] || { check "private weights: e2e/isolated.sh named no Hub log or email to sign in with"; return; }
local dir=$OUT/private-weights account model commit
rm -rf "$dir"; mkdir -p "$dir"
signin "$E2E_HUB_EMAIL" > "$dir/login.json" || { check "private weights: cozy login failed: $(tail -c 300 "$dir/login.json")"; return; }
account=$(cozy --json whoami | jq -r '.account // empty')
model=${account:-e2e}/cl1009-vae-$RANDOM
# A `vae` as parts.py reads one: its file's metadata (`part`, `size`) is its config.
python3 -I - "$dir/vae.safetensors" <<'PY'
import json, random, struct, sys
n = 65536
raw = json.dumps({"__metadata__": {"part": "vae-private", "size": str(n)}, "w": {"dtype": "F32", "shape": [n], "data_offsets": [0, 4 * n]}}).encode()
raw += b" " * (-len(raw) % 8)
values = random.Random(1009).random
open(sys.argv[1], "wb").write(struct.pack("<Q", len(raw)) + raw + struct.pack(f"<{n}f", *(values() for _ in range(n))))
PY
if run_submit "$dir" upload model upload "$dir/vae.safetensors:vae" "$model" --lane f32 --source-format as-is --on local > /dev/null \
  && run_wait "$dir" upload && [ "$(run_get "$dir" upload .status)" = completed ]; then
  commit=$(jq -r '.. | objects | .commit? // empty' "$dir/upload.ndjson" 2> /dev/null | tail -1) || true
  if [ -z "$commit" ]; then
    check "private weights: the upload named no commit: $(tail -c 300 "$dir/upload.ndjson")"
  elif cell private-weights "$here/script/parts.py" --model "vae=$model@$commit"; then
    J '.result.vae.part == "vae-private" and .result.transformer.part == "dit"' "$OUT/private-weights.json" \
      || check "private weights: not e2e/parts's transformer with $model's VAE: $(jq -c .result "$OUT/private-weights.json")"
  fi
else
  check "private weights: model upload $model --on local: $(run_get "$dir" upload '.reason.message' | head -c 300)"
fi
cozy --json logout > "$dir/logout.json" 2>&1 || check "private weights: logout: $(tail -c 300 "$dir/logout.json")"
}
