# e2e/cli/egress.sh: what a request can make a computer read (xs-196 D). Linux, the local machine.
#   egress          No URL a run or a Hub answer names reaches a link-local, loopback or private address, at its first hop
#                   or after a redirect from an origin this computer declared local (E2E_REDIRECT, lib.sh): Calcifer's link
#                   import and TensorD's model sources alike. A run names no path on the machine as a model
#                   (local_model_path_refused). A failed fetch of a presigned URL says, and logs, none of its query.
#   hostile_inputs  A video input that names other files or the network (a concat list, a DASH manifest, an HLS
#                   playlist) is refused by the Runtime's decoder, which opens none of them.

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
        with open(log, "a") as out:
            out.write(self.path + "\n")
        self.send_response(404 if self.path.startswith("/missing") else 302)
        if not self.path.startswith("/missing"):
            self.send_header("Location", "http://169.254.169.254/latest/meta-data/")
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

egress() {
step "no fetch reaches the metadata address or the machine's files (xs-196 D)"
[ -n "$LOCAL" ] || return 0
[ -n "${E2E_REDIRECT:-}" ] || { check "egress: lib.sh declared no redirector (E2E_REDIRECT)"; return; }
local dir=$OUT/egress secret=e2e-presigned-$RANDOM$RANDOM pem key='' code=0 name source leaked
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

# TensorD: model sources, as a client holding the machine's authorization sends them.
pem=$dir/signer.pem
for _ in $(seq 100); do
  openssl genpkey -algorithm ed25519 -out "$pem" 2> /dev/null || { check "egress: openssl could not make an Ed25519 key"; break; }
  key=$(openssl pkey -in "$pem" -pubout -outform DER | tail -c 32 | base64 | tr '+/' '-_' | tr -d '=\n')
  [[ $key == -* ]] || break
done
cozy --json machine authorize "$key" > "$dir/authorize.json" || { check "egress: machine authorize failed"; redirector_stop; return; }
cozy --json machine show local > "$dir/local.json" || check "egress: machine show local failed"
uv run -q --no-project --with cryptography python -I - "$pem" "$(jq -r .address "$dir/local.json")" \
  "$(jq -r .fingerprint "$dir/local.json")" "$E2E_REDIRECT" "$secret" "$dir/machine.json" <<'PY' || code=$?
import base64, hashlib, http.client, json, ssl, sys, time
from cryptography.hazmat.primitives.serialization import load_pem_private_key

pem, address, fingerprint, redirect, secret, out = sys.argv[1:]
key = load_pem_private_key(open(pem, "rb").read(), None)
b64 = lambda raw: base64.urlsafe_b64encode(raw).rstrip(b"=").decode()
host, port = address.split("://")[-1].rsplit(":", 1)
tls = ssl.create_default_context()
tls.check_hostname, tls.verify_mode = False, ssl.CERT_NONE

def connect(timeout=60):
    connection = http.client.HTTPSConnection(host.strip("[]"), int(port), context=tls, timeout=timeout)
    connection.connect()
    leaf = hashlib.sha256(connection.sock.getpeercert(binary_form=True)).hexdigest()
    assert leaf == fingerprint.removeprefix("sha256:"), f"{address} presents the leaf {leaf}, not {fingerprint}"
    return connection

def call(method, path, body=b""):
    connection = connect()
    connection.request(method, path, body=body, headers={"content-length": str(len(body)), "authorization": f"Cozy-Cap {token}"})
    answer = connection.getresponse()
    return answer.status, answer.read()

connection = connect()
connection.request("GET", "/v1/status")
worker = json.loads(connection.getresponse().read())["worker_id"]
grant = json.dumps({"m": worker, "a": "machine", "e": int(time.time()) + 900, "k": b64(hashlib.sha256(key.public_key().public_bytes_raw()).digest()[:16])}).encode()
domain = b"cozy-capability/1\0"  # api::machine::capability's DOMAIN, then the grant
token = f"{b64(grant)}.{b64(key.sign(domain + grant))}"

def outcome(run):
    """The run's outcome event, read from its event stream."""
    connection = connect(timeout=600)
    connection.request("GET", f"/v1/runs/{run}/events", headers={"authorization": f"Cozy-Cap {token}", "accept": "text/event-stream"})
    answer, event = connection.getresponse(), ""
    for raw in answer:
        line = raw.decode().rstrip("\n")
        if line.startswith("event:"):
            event = line[6:].strip()
        elif line.startswith("data:") and event == "outcome":
            return json.loads(line[5:])["event"]["outcome"]
    return {"status": "no outcome"}

cases = {
    "path": [{"parameter": "model", "source": "/etc/hostname"}],
    "added": [{"parameter": "model", "source": "hf://cozy-e2e/none"}, {"parameter": "add:transformer/notes.txt", "source": "/etc/hostname"}],
    "metadata": [{"parameter": "model", "source": "https://169.254.169.254/latest/meta-data/model.safetensors"}],
    "redirect": [{"parameter": "model", "source": f"{redirect}/model.safetensors"}],
    "presigned": [{"parameter": "model", "source": f"{redirect}/missing.safetensors?X-Amz-Signature={secret}"}],
}
stamp, said = time.time_ns(), {}
runs = [{"id": f"e2e-egress-{name}-{stamp}", "spec": {"kind": "warm", "models": models}} for name, models in cases.items()]
status, body = call("POST", "/v1/runs/batch", json.dumps({"runs": runs}).encode())
assert status == 200, f"POST /v1/runs/batch: {status} {body[:300]!r}"
for name, admitted in zip(cases, json.loads(body)["runs"]):
    said[name] = {"refusal": admitted["refusal"]} if admitted.get("refusal") else {"outcome": outcome(f"e2e-egress-{name}-{stamp}")}
    print(f"{name}: {json.dumps(said[name])[:400]}")
open(out, "w").write(json.dumps(said))
PY
cozy --json machine authorize "$key" --remove > /dev/null || check "egress: machine authorize --remove failed"
redirector_stop
[ "$code" = 0 ] || check "egress: the machine's requests failed (exit $code)"
for name in path added; do
  J --arg n "$name" '.[$n].refusal.code == "local_model_path_refused"' "$dir/machine.json" \
    || check "egress: a model source naming /etc/hostname ($name) was not refused local_model_path_refused: $(jq -c --arg n "$name" '.[$n]' "$dir/machine.json" 2> /dev/null | head -c 300)"
done
for name in metadata redirect; do
  J --arg n "$name" '.[$n].outcome.status == "failed" and (.[$n] | tostring | contains("link-local"))' "$dir/machine.json" \
    || check "egress: TensorD's $name model source did not fail as link-local: $(jq -c --arg n "$name" '.[$n]' "$dir/machine.json" 2> /dev/null | head -c 400)"
done
grep -qx /model.safetensors "$dir/asked.log" || check "egress: TensorD never asked the declared origin, so its redirect proves nothing"
J '.presigned.outcome.status == "failed"' "$dir/machine.json" || check "egress: the presigned source did not fail: $(jq -c .presigned "$dir/machine.json" 2> /dev/null | head -c 300)"
# The presigned query appears in no answer and no log of this computer's cozy (the run's own spec keeps the source it was sent).
leaked=$({ grep -lsI "$secret" "$dir"/import-*.json; jq -c '.presigned' "$dir/machine.json" | grep -l "$secret"; grep -rlsI --include='*.log*' "$secret" "$HOME/.cozy"; } 2> /dev/null | head -3 || true)
[ -z "$leaked" ] || check "egress: the presigned query is in $leaked"
echo "egress: $(jq -c 'map_values(.refusal.code // .outcome.status)' "$dir/machine.json" 2> /dev/null)"
}

hostile_inputs() {
step "a video input naming other files or the network: the Runtime opens none of them (xs-196 D)"
[ -n "$LOCAL" ] || return 0
[ -n "${E2E_REDIRECT:-}" ] || { check "hostile inputs: lib.sh declared no redirector (E2E_REDIRECT)"; return; }
local dir=$OUT/hostile-inputs name
rm -rf "$dir"; mkdir -p "$dir/folder"; mkfifo "$dir/marker"
echo one > "$dir/folder/a.txt"
ffmpeg -v error -y -f lavfi -i "color=c=teal:size=64x64" -frames:v 1 "$dir/still.png"
printf "ffconcat version 1.0\nfile '%s'\n" "$dir/marker" > "$dir/concat.mp4"
printf '<?xml version="1.0"?>\n<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT2S" minBufferTime="PT1S" profiles="urn:mpeg:dash:profile:isoff-on-demand:2011">\n<Period><AdaptationSet mimeType="video/mp4"><Representation id="1" bandwidth="1000" codecs="avc1.42c00d" width="64" height="64"><BaseURL>%s/dash-segment.mp4</BaseURL><SegmentBase indexRange="0-100"/></Representation></AdaptationSet></Period></MPD>\n' "$E2E_REDIRECT" > "$dir/dash.mp4"
printf '#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\n%s/hls-segment.ts\n#EXT-X-ENDLIST\n' "$E2E_REDIRECT" > "$dir/hls.mp4"
redirector_start "$dir/asked.log"
(timeout 300 sh -c 'echo read > "$1"' _ "$dir/marker" && touch "$dir/opened") & local reader=$!
for name in concat dash hls; do
  (cd "$dir" && ALLOW='media_format_refused\|media_malformed' cell "hostile-$name" "$here/inspect/inspect" --file "clip=$dir/$name.mp4" --file "still=$dir/still.png" "folder=$dir/folder" label="$name") \
    && { [ -f "$OUT/hostile-$name.refused" ] || check "hostile inputs: the $name input was decoded"; }
done
grep -q media_format_refused "$OUT/hostile-concat.ndjson" "$OUT/hostile-concat.err" 2> /dev/null \
  || check "hostile inputs: the concat list was not refused media_format_refused: $(tail -c 300 "$OUT/hostile-concat.ndjson" 2> /dev/null)"
sleep 2
redirector_stop
[ ! -s "$dir/asked.log" ] || check "hostile inputs: a decoded input reached the network: $(head -c 200 "$dir/asked.log")"
[ ! -e "$dir/opened" ] || check "hostile inputs: a decoded input opened the file its concat list names"
kill "$reader" 2> /dev/null || true
}
