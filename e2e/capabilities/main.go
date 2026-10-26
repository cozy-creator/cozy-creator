// e2e/capabilities: a run capability (cl-675, AuthKit's workload grant) reaches what its runs need and nothing else. It
// runs no run: it does what Calcifer and TensorD do, against the Hub's own answers, with this computer's signed-in cozy:
// signs a capability with the device key for a P-256 workload key, redeems it (RFC 7523 jwt-bearer + DPoP) and reads.
//
//	go run ./e2e/capabilities --out DIR --commit ORG/NAME@sha256:<hex> [--hub ORIGIN] [--rental ID --rental-state live|ended]
//
// ORIGIN is the Hub cozy is signed in to (default $COZY_HUB_URL, else https://tensorhub.com); the commit is a private one
// of the signed-in account. Checks:
//   - the capability's token reads its commit, which an anonymous read cannot, and is redeemed again (renewal);
//   - a copied token is refused without its DPoP key, and a proof is spent once;
//   - widened scope is refused: another repository, a release, a delete, rentals; a capability naming another account's
//     repository; a capability altered after signing; an expired one;
//   - with --rental, a capability naming that rental is granted while it is live and refused once it ended.
package main

import (
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"
)

var (
	b64    = base64.RawURLEncoding
	out    string
	failed bool
	client = &http.Client{Timeout: time.Minute, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
)

func main() {
	hub := flag.String("hub", envOr("COZY_HUB_URL", "https://tensorhub.com"), "the Hub cozy is signed in to")
	commit := flag.String("commit", "", "ORG/NAME@sha256:<hex>, a private commit of the signed-in account")
	rental := flag.String("rental", "", "a rental of the signed-in account")
	rentalState := flag.String("rental-state", "live", "live or ended")
	flag.StringVar(&out, "out", "", "where each answer is kept")
	flag.Parse()
	m := regexp.MustCompile(`^([a-z0-9_.-]+)/([a-z0-9_.-]+)@sha256:([0-9a-f]{64})$`).FindStringSubmatch(*commit)
	if out == "" || m == nil {
		fmt.Fprintln(os.Stderr, "usage: capabilities --out DIR --commit ORG/NAME@sha256:<hex> [--hub ORIGIN] [--rental ID --rental-state live|ended]")
		os.Exit(2)
	}
	org, name, sha := m[1], m[2], m[3]
	*hub = strings.TrimRight(*hub, "/")
	must(os.MkdirAll(out, 0o755))

	device, bearer := deviceKey(*hub)
	var account struct {
		Name   string `json:"name"`
		UserID string `json:"user_id"`
	}
	call("account", "GET", *hub+"/v1/accounts/current", "Bearer "+bearer, "", nil, &account)
	var resource struct {
		Resource string   `json:"resource"`
		Issuers  []string `json:"authorization_servers"`
	}
	call("protected-resource", "GET", *hub+"/.well-known/oauth-protected-resource", "", "", nil, &resource)
	var metadata struct {
		TokenEndpoint string `json:"token_endpoint"`
	}
	call("authorization-server", "GET", resource.Issuers[0]+"/.well-known/oauth-authorization-server", "", "", nil, &metadata)
	origin, token := resource.Resource, metadata.TokenEndpoint
	blob := fmt.Sprintf("%s/blobs/%s/%s/%s", origin, sha[:2], sha[2:4], sha)

	key := newWorkload()
	read := map[string]any{"type": "tensorhub_read", "commit": "sha256:" + sha}
	write := map[string]any{"type": "tensorhub_write", "model": org + "/" + name}
	capability := func(ops []map[string]any, life time.Duration, extra map[string]any) string {
		return device.sign(account.UserID, origin, key.thumbprint, ops, life, extra)
	}
	granted := capability([]map[string]any{read, write}, time.Hour, nil)

	got := redeem(key, token, granted)
	expect("redeem", got.status, 200, "")
	at := got.token
	expect("anonymous-read", status("anonymous-read", "GET", blob, "", ""), 404, "")
	expect("read", status("read", "GET", blob, "DPoP "+at, key.proof("GET", blob, at)), 307, "")
	expect("head", status("head", "HEAD", blob, "DPoP "+at, key.proof("HEAD", blob, at)), 200, "")

	renewed := redeem(key, token, granted)
	expect("renew", renewed.status, 200, "")
	if renewed.token == at {
		fail("renew: the same token came back")
	}
	expect("read-renewed", status("read-renewed", "GET", blob, "DPoP "+renewed.token, key.proof("GET", blob, renewed.token)), 307, "")

	expect("copied-as-bearer", status("copied-as-bearer", "GET", blob, "Bearer "+at, ""), 401, "")
	expect("copied-other-key", status("copied-other-key", "GET", blob, "DPoP "+at, newWorkload().proof("GET", blob, at)), 401, "")
	proof := key.proof("GET", blob, at)
	expect("proof-once", status("proof-once", "GET", blob, "DPoP "+at, proof), 307, "")
	expect("proof-replayed", status("proof-replayed", "GET", blob, "DPoP "+at, proof), 401, "")

	other := origin + "/v1/models/" + org + "/" + name + "-e2e-other/commits"
	expect("other-repository", status("other-repository", "POST", other, "DPoP "+at, key.proof("POST", other, at)), 403, "capability.operation_forbidden")
	release := origin + "/v1/models/" + org + "/" + name + "/releases/999.0.0"
	expect("release", status("release", "PUT", release, "DPoP "+at, key.proof("PUT", release, at)), 403, "capability.operation_forbidden")
	del := origin + "/v1/models/" + org + "/" + name + "/commits/sha256:" + sha
	expect("delete", status("delete", "DELETE", del, "DPoP "+at, key.proof("DELETE", del, at)), 403, "capability.operation_forbidden")
	rentals := origin + "/v1/rentals"
	expect("rentals", status("rentals", "GET", rentals, "DPoP "+at, key.proof("GET", rentals, at)), 403, "capability.operation_forbidden")
	expect("minted-route-gone", status("minted-route-gone", "POST", *hub+"/v1/capabilities", "Bearer "+bearer, ""), 404, "")

	foreign := capability([]map[string]any{read, {"type": "tensorhub_write", "model": account.Name + "-e2e-not-yours/x"}}, time.Hour, nil)
	expectRefused("foreign-repository", redeem(key, token, foreign), "refused")
	// A read-only capability given a write after it was signed.
	parts := strings.Split(capability([]map[string]any{read}, time.Hour, nil), ".")
	var claims map[string]any
	payload, _ := b64.DecodeString(parts[1])
	must(json.Unmarshal(payload, &claims))
	claims["authorization_details"] = []map[string]any{read, write}
	widened := parts[0] + "." + segment(claims) + "." + parts[2]
	expectRefused("altered-after-signing", redeem(key, token, widened), "capability_invalid")
	expectRefused("another-key", redeem(newWorkload(), token, granted), "key_mismatch")
	short := capability([]map[string]any{read}, 2*time.Second, nil)
	expect("short-lived", redeem(key, token, short).status, 200, "")
	time.Sleep(3 * time.Second)
	expectRefused("expired", redeem(key, token, short), "capability_expired")

	expectRefused("unknown-rental", redeem(key, token, capability([]map[string]any{read}, time.Hour, map[string]any{"rental": "rent-e2e-none"})), "refused")
	if *rental != "" {
		named := redeem(key, token, capability([]map[string]any{read}, time.Hour, map[string]any{"rental": *rental}))
		if *rentalState == "ended" {
			expectRefused("rental-ended", named, "refused")
		} else {
			expect("rental-live", named.status, 200, "")
		}
	}

	if failed {
		fmt.Println("capabilities: FAILED")
		os.Exit(1)
	}
	fmt.Println("capabilities: every check passed")
}

// device is this computer's device key at the Hub, as cozy keeps it, and its live Bearer.
type device struct {
	id  string
	key ed25519.PrivateKey
}

func deviceKey(hub string) (device, string) {
	home, err := os.UserHomeDir()
	must(err)
	sum := sha256.Sum256([]byte(hub))
	file := filepath.Join(home, ".cozy", "keys", "device-"+hex.EncodeToString(sum[:]))
	var kept struct {
		ID   string `json:"device_key_id"`
		Seed string `json:"private_key"`
	}
	raw, err := os.ReadFile(file + ".json")
	must(err)
	must(json.Unmarshal(raw, &kept))
	seed, err := b64.DecodeString(kept.Seed)
	must(err)
	var bearer struct{ Token string }
	raw, err = os.ReadFile(file + ".bearer.json")
	must(err)
	must(json.Unmarshal(raw, &bearer))
	return device{id: kept.ID, key: ed25519.NewKeyFromSeed(seed)}, bearer.Token
}

// sign is AuthKit's devicekey.SignCapability.
func (d device) sign(user, audience, jkt string, ops []map[string]any, life time.Duration, extra map[string]any) string {
	now := time.Now()
	claims := map[string]any{"sub": user, "aud": audience, "cnf": map[string]string{"jkt": jkt},
		"authorization_details": ops, "jti": nonce(), "iat": now.Unix(), "exp": now.Add(life).Unix()}
	for k, v := range extra {
		claims[k] = v
	}
	input := segment(map[string]string{"alg": "EdDSA", "typ": "authkit-capability+jwt", "kid": d.id}) + "." + segment(claims)
	return input + "." + b64.EncodeToString(ed25519.Sign(d.key, []byte(input)))
}

// workload is a machine's P-256 key.
type workload struct {
	key        *ecdsa.PrivateKey
	jwk        map[string]string
	thumbprint string
}

func newWorkload() workload {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	must(err)
	point, err := key.PublicKey.Bytes()
	must(err)
	x, y := b64.EncodeToString(point[1:33]), b64.EncodeToString(point[33:])
	sum := sha256.Sum256([]byte(`{"crv":"P-256","kty":"EC","x":"` + x + `","y":"` + y + `"}`))
	return workload{key, map[string]string{"kty": "EC", "crv": "P-256", "x": x, "y": y}, b64.EncodeToString(sum[:])}
}

func (w workload) jws(typ string, claims map[string]any) string {
	input := segment(map[string]any{"alg": "ES256", "typ": typ, "jwk": w.jwk}) + "." + segment(claims)
	digest := sha256.Sum256([]byte(input))
	r, s, err := ecdsa.Sign(rand.Reader, w.key, digest[:])
	must(err)
	sig := make([]byte, 64)
	r.FillBytes(sig[:32])
	s.FillBytes(sig[32:])
	return input + "." + b64.EncodeToString(sig)
}

func (w workload) proof(method, target, token string) string {
	claims := map[string]any{"htm": method, "htu": target, "iat": time.Now().Unix(), "jti": nonce()}
	if token != "" {
		sum := sha256.Sum256([]byte(token))
		claims["ath"] = b64.EncodeToString(sum[:])
	}
	return w.jws("dpop+jwt", claims)
}

type redeemed struct {
	status       int
	token, error string
	reason       string
}

func redeem(w workload, endpoint, capability string) redeemed {
	assertion := w.jws("JWT", map[string]any{"iss": "tensord", "sub": w.thumbprint, "aud": endpoint, "iat": time.Now().Unix(),
		"exp": time.Now().Add(time.Minute).Unix(), "jti": nonce(), "capability": capability})
	form := url.Values{"grant_type": {"urn:ietf:params:oauth:grant-type:jwt-bearer"}, "client_id": {"tensord"}, "assertion": {assertion}}
	req, _ := http.NewRequest("POST", endpoint, strings.NewReader(form.Encode()))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	req.Header.Set("DPoP", w.proof("POST", endpoint, ""))
	res, err := client.Do(req)
	must(err)
	defer res.Body.Close()
	var body struct {
		AccessToken string `json:"access_token"`
		Error       string `json:"error"`
		Reason      string `json:"reason"`
	}
	raw, _ := io.ReadAll(res.Body)
	_ = json.Unmarshal(raw, &body)
	return redeemed{res.StatusCode, body.AccessToken, body.Error, body.Reason}
}

var codes = map[string]string{}

// status sends method target with authorization and a DPoP proof, keeping the answer as NAME.json.
func status(name, method, target, authorization, proof string) int {
	req, _ := http.NewRequest(method, target, nil)
	if authorization != "" {
		req.Header.Set("Authorization", authorization)
	}
	if proof != "" {
		req.Header.Set("DPoP", proof)
	}
	res, err := client.Do(req)
	must(err)
	defer res.Body.Close()
	raw, _ := io.ReadAll(res.Body)
	_ = os.WriteFile(filepath.Join(out, name+".json"), raw, 0o644)
	var body struct {
		Error struct{ Code string } `json:"error"`
	}
	_ = json.Unmarshal(raw, &body)
	codes[name] = body.Error.Code
	return res.StatusCode
}

func call(name, method, target, authorization, proof string, body io.Reader, into any) {
	req, _ := http.NewRequest(method, target, body)
	if authorization != "" {
		req.Header.Set("Authorization", authorization)
	}
	res, err := client.Do(req)
	must(err)
	defer res.Body.Close()
	raw, _ := io.ReadAll(res.Body)
	_ = os.WriteFile(filepath.Join(out, name+".json"), raw, 0o644)
	if res.StatusCode != 200 || json.Unmarshal(raw, into) != nil {
		fmt.Printf("FAIL %s: %d %s\n", name, res.StatusCode, raw)
		os.Exit(1)
	}
}

func expect(name string, got, want int, code string) {
	if got == want && (code == "" || codes[name] == code) {
		fmt.Printf("ok   %s: %d %s\n", name, got, code)
		return
	}
	fail(fmt.Sprintf("%s: want %d %s, got %d %s", name, want, code, got, codes[name]))
}

func expectRefused(name string, got redeemed, reason string) {
	if got.status == 400 && got.error == "invalid_grant" && got.reason == reason {
		fmt.Printf("ok   %s: invalid_grant %s\n", name, reason)
		return
	}
	fail(fmt.Sprintf("%s: want invalid_grant %s, got %d %s %s", name, reason, got.status, got.error, got.reason))
}

func fail(why string) {
	fmt.Println("FAIL " + why)
	failed = true
}

func segment(v any) string {
	raw, err := json.Marshal(v)
	must(err)
	return b64.EncodeToString(raw)
}

func nonce() string {
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	return b64.EncodeToString(b)
}

func envOr(name, fallback string) string {
	if v := os.Getenv(name); v != "" {
		return v
	}
	return fallback
}

func must(err error) {
	if err != nil {
		fmt.Println("FAIL " + err.Error())
		os.Exit(1)
	}
}
