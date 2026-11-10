// podsecrets proves a rental pod holds no secret (xs-157). It runs the real Hub binary on a
// throwaway Postgres with its Docker provider, which rents a pod image holding the cozy binary
// under test, rents one CPU machine through POST /v1/rentals as an enrolled renter, and checks:
//
//  1. the pod's environment (the container's and pid 1's) names no secret;
//  2. the Hub pinned the leaf the pod serves at the provider's address, the pod read its lease,
//     and the rental is ready with that leaf as its certificate;
//  3. a request signed by the pod's key passes; one signed by another key, an unsigned one, a
//     worker token, a stale clock, another key's release and an endpoint report are refused, and
//     neither the pin nor the address moves;
//  4. after the pod restarts, the pin and boot hold and the pod's key still signs.
//
// Then the rental is released and the container, Hub, registry and database are removed.
//
//	podsecrets --tensorhub BIN --cozy BIN --out DIR
package main

import (
	"archive/tar"
	"bufio"
	"bytes"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"encoding/pem"
	"flag"
	"fmt"
	"io"
	"log"
	"math/big"
	"net"
	"net/http"
	"net/http/httptest"
	"net/http/httputil"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/google/go-containerregistry/pkg/name"
	"github.com/google/go-containerregistry/pkg/registry"
	v1 "github.com/google/go-containerregistry/pkg/v1"
	"github.com/google/go-containerregistry/pkg/v1/mutate"
	"github.com/google/go-containerregistry/pkg/v1/remote"
	"github.com/google/go-containerregistry/pkg/v1/tarball"
)

func main() {
	hubBin := flag.String("tensorhub", "", "the tensorhub binary")
	cozyBin := flag.String("cozy", "", "the cozy binary the pod runs (linux, this host's glibc)")
	out := flag.String("out", "", "where the logs go")
	flag.Parse()
	if *hubBin == "" || *cozyBin == "" || *out == "" {
		flag.Usage()
		os.Exit(2)
	}
	h := &harness{}
	code := 0
	func() {
		defer func() {
			if r := recover(); r != nil {
				fmt.Println("ABORT", r)
				code = 1
			}
		}()
		h.prove(abs(*hubBin), abs(*cozyBin), abs(*out))
		if h.failed > 0 {
			code = 1
		}
	}()
	h.teardown()
	fmt.Printf("%d checks, %d failed\n", h.checks, h.failed)
	os.Exit(code)
}

type harness struct {
	cleanups       []func()
	checks, failed int
	hub            string
	pg             string
	logMu          sync.Mutex
	hubLog         []string
}

func (h *harness) later(f func()) { h.cleanups = append(h.cleanups, f) }

func (h *harness) teardown() {
	for i := len(h.cleanups) - 1; i >= 0; i-- {
		h.cleanups[i]()
	}
	h.cleanups = nil
}

func (h *harness) check(ok bool, format string, args ...any) {
	h.checks++
	mark := "PASS"
	if !ok {
		mark = "FAIL"
		h.failed++
	}
	fmt.Printf("%s  %s\n", mark, fmt.Sprintf(format, args...))
}

func (h *harness) prove(hubBin, cozyBin, out string) {
	must(os.MkdirAll(out, 0o700), "out")
	suffix := hex.EncodeToString(random(3))

	// A throwaway Postgres for this Hub alone.
	h.pg = "podsecrets-pg-" + suffix
	docker("run", "-d", "--rm", "--name", h.pg, "-e", "POSTGRES_HOST_AUTH_METHOD=trust", "-p", "127.0.0.1::5432", "postgres:18-alpine")
	h.later(func() { _, _ = run("docker", "rm", "-f", h.pg) })
	until("postgres", time.Minute, func() bool {
		_, err := run("docker", "exec", h.pg, "pg_isready", "-h", "127.0.0.1", "-U", "postgres")
		return err == nil
	})
	dsn := "postgres://postgres@" + strings.Split(docker("port", h.pg, "5432/tcp"), "\n")[0] + "/postgres?sslmode=disable"

	// The registry the Hub resolves worker tags from and the Docker Engine pulls the pod from.
	images := httptest.NewServer(registry.New(registry.Logger(log.New(io.Discard, "", 0))))
	h.later(images.Close)
	repo := strings.TrimPrefix(images.URL, "http://") + "/tensorhub/worker"
	image := h.podImage(cozyBin, repo)

	// The Hub's public origin as a pod reaches it: TLS on the Docker bridge, under a CA the Hub
	// hands its pods, in front of the Hub's own listener, as a deployment's ingress is.
	hubPort := freePort()
	h.hub = "http://127.0.0.1:" + strconv.Itoa(hubPort)
	gateway := docker("network", "inspect", "bridge", "--format", "{{(index .IPAM.Config 0).Gateway}}")
	origin, caPath := h.ingress(out, gateway, h.hub)

	// The Hub, migrated, its catalog applied, serving.
	empty := filepath.Join(out, "empty.env")
	must(os.WriteFile(empty, nil, 0o600), "dotenv")
	secrets := filepath.Join(out, "secrets")
	must(os.MkdirAll(secrets, 0o700), "secrets")
	args := []string{"--dotenv", empty, "--secrets-dir", secrets}
	for _, set := range []string{
		"env.name=development", "database.url=" + dsn,
		"server.host=127.0.0.1", "server.http_port=" + strconv.Itoa(hubPort), "server.public_origin=" + origin,
		"storage.backend.endpoint_url=http://127.0.0.1:9", "storage.backend.region=podsecrets",
		"storage.backend.access_key_id=podsecrets", "storage.backend.secret_access_key=podsecrets",
		"storage.repo_cas.bucket=repo", "storage.package_source.bucket=source",
		"package_release.scratch_root=" + filepath.Join(out, "release"), "auth.keys_path=" + filepath.Join(out, "authkit"),
		"auth.rate_limits=device_key_enroll_begin=1000/1h,device_key_enroll_finish=1000/1h",
		"worker_image.repository=" + repo, "provider.kind=docker", "catalog.parked_orgs=",
		"rental.hub_ca_path=" + caPath, "billing.signup_credit_usd_micros=10000000", "billing.rental_markup_percent=66",
		"provider.heartbeat_seconds=1", "provider.readback_interval_seconds=1",
	} {
		args = append(args, "--set", set)
	}
	hubLog, err := os.Create(filepath.Join(out, "tensorhub.log"))
	must(err, "hub log")
	h.later(func() { hubLog.Close() })
	for _, command := range [][]string{{"migrate"}, {"catalog", "apply"}} {
		cmd := exec.Command(hubBin, append(args, command...)...)
		cmd.Dir, cmd.Stdout, cmd.Stderr = out, hubLog, hubLog
		must(cmd.Run(), "tensorhub "+strings.Join(command, " ")+" (see tensorhub.log)")
	}
	h.serveHub(hubBin, args, out, hubLog)
	fmt.Printf("hub %s, its pods reach it at %s, worker images %s\n", h.hub, origin, repo)

	// A renter, enrolled as the CLI enrolls, and one CPU rental.
	renterKey, token := h.enroll("podsecrets" + suffix)
	var created struct {
		RentalID string `json:"rental_id"`
	}
	var tokenHash [32]byte
	_, _ = rand.Read(tokenHash[:])
	creator, _, _ := ed25519.GenerateKey(rand.Reader)
	h.call(token, http.MethodPost, "/v1/rentals", map[string]any{"name": "podsecrets", "sku": "cpu",
		"media_token_sha256": hex.EncodeToString(tokenHash[:]), "creator_public_key": b64(creator)},
		map[string]string{"Idempotency-Key": "podsecrets-" + suffix}, http.StatusAccepted, &created)
	rental := created.RentalID
	h.later(func() { _, _ = run("docker", "rm", "-f", rental) })
	view := h.untilReady(token, rental, 10*time.Minute)
	container := rental
	fmt.Printf("rental %s ready: worker %s at %s (container %s, image %s)\n", rental, view.WorkerID, view.WorkerAddress, container, image)
	worker := view.WorkerID

	// 1. The pod's environment is public facts only.
	var env []string
	must(json.Unmarshal([]byte(docker("inspect", "-f", "{{json .Config.Env}}", container)), &env), "container env")
	proc := strings.Split(strings.Trim(docker("exec", container, "cat", "/proc/1/environ"), "\x00"), "\x00")
	for _, where := range []struct {
		name string
		vars []string
	}{{"the container's environment", env}, {"pid 1's environment", proc}} {
		names, secret := envNames(where.vars)
		h.check(len(secret) == 0, "1. %s names no secret: %s", where.name, strings.Join(names, " "))
		if len(secret) > 0 {
			fmt.Printf("      secret-shaped: %s\n", strings.Join(secret, " "))
		}
	}

	// 2. The pin is the leaf the pod serves at the provider's address, and the pod read its lease.
	address := strings.Split(docker("port", container, "8443/tcp"), "\n")[0]
	served := presentedLeaf(address)
	podLeaf := pemDER(docker("exec", container, "cat", "/run/cozy/bootstrap/tls.crt"))
	pin := h.pin(rental)
	h.check(view.WorkerAddress == address, "2. the rental's address %s is the provider's mapping %s", view.WorkerAddress, address)
	h.check(bytes.Equal(served, podLeaf), "2. the leaf served at %s is the pod's own (sha256 %s)", address, short(served))
	h.check(pin.leaf == fmt.Sprintf("%x", sha256.Sum256(podLeaf)), "2. the Hub pinned that leaf to attempt %s (boot %s)", worker, pin.boot)
	h.check(bytes.Equal(pemDER(view.CertPEM), podLeaf), "2. the rental's certificate for clients is the pinned leaf")
	h.check(pin.leased, "2. the pinned pod read its lease (machine_leased_at %s) before it was ready", pin.leasedAt)
	lease := docker("exec", container, "cat", "/var/lib/cozy/machine/lease.json")
	h.check(strings.Contains(lease, b64(renterKey)), "2. the pod admits the renter's device key from its lease")
	mode := docker("exec", container, "stat", "-c", "%a", "/run/cozy/bootstrap/tls.key", "/var/lib/cozy/machine/lease.json")
	h.check(strings.Fields(mode)[0] == "400" && strings.Fields(mode)[1] == "600", "2. the leaf key is 0400 and the lease 0600 under the machine root (%s)", strings.Join(strings.Fields(mode), ", "))

	// 3. Only the pinned key speaks for the worker.
	podKey := pkcs8(docker("exec", container, "cat", "/run/cozy/bootstrap/tls.key"))
	other, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	now := time.Now().Unix()
	status, code := h.worker(http.MethodGet, "/v1/worker/rental/authorized-keys", worker, nil, podKey, now, nil)
	h.check(status == http.StatusOK, "3. a lease read signed by the pod's key: %d", status)
	status, code = h.worker(http.MethodGet, "/v1/worker/rental/authorized-keys", worker, nil, other, now, nil)
	h.check(status == http.StatusUnauthorized && code == "worker.unauthorized", "3. the same worker id signed by another key: %d %s", status, code)
	status, code = h.worker(http.MethodGet, "/v1/worker/rental/authorized-keys", worker, nil, nil, now, nil)
	h.check(status == http.StatusUnauthorized, "3. unsigned: %d %s", status, code)
	status, code = h.worker(http.MethodGet, "/v1/worker/rental/authorized-keys", worker, nil, nil, now,
		map[string]string{"X-Cozy-Worker-Token": b64(random(32))})
	h.check(status == http.StatusUnauthorized, "3. a worker token, as the old environment carried: %d %s", status, code)
	status, code = h.worker(http.MethodGet, "/v1/worker/rental/authorized-keys", worker, nil, podKey, now-3600, nil)
	h.check(status == http.StatusUnauthorized && code == "worker.clock_skew", "3. the pod's key an hour late (a replay): %d %s", status, code)
	release, _ := json.Marshal(map[string]string{"worker_id": worker})
	status, code = h.worker(http.MethodPost, "/v1/worker/rental/release", worker, release, other, now, nil)
	h.check(status == http.StatusUnauthorized, "3. another key ending the rental: %d %s", status, code)
	report, _ := json.Marshal(map[string]any{"public_host": "203.0.113.7", "ports": map[string]int{"8443": 4443}})
	for _, signer := range []struct {
		name string
		key  *ecdsa.PrivateKey
	}{{"another key", other}, {"the pod's own key", podKey}} {
		status, code = h.worker(http.MethodPost, "/v1/worker/rental/endpoint", worker, report, signer.key, now, nil)
		h.check(status == http.StatusNotFound, "3. an endpoint report signed by %s: %d %s", signer.name, status, code)
	}
	time.Sleep(3 * time.Second)
	after := h.view(token, rental)
	h.check(after.State == "ready" && after.WorkerAddress == address && h.pin(rental).leaf == pin.leaf,
		"3. the rental is still ready at %s with the same pin", after.WorkerAddress)

	// 4. A restart keeps the pin.
	docker("restart", container)
	restarted := time.Now()
	moved := strings.Split(docker("port", container, "8443/tcp"), "\n")[0]
	until("the Hub to verify the restarted pod", 5*time.Minute, func() bool {
		return h.verifiedSince(rental, restarted) && h.view(token, rental).WorkerAddress == moved
	})
	again := h.pin(rental)
	h.check(again.leaf == pin.leaf && again.boot == pin.boot, "4. after a restart (address %s, now %s) the pin and boot hold", address, moved)
	h.check(bytes.Equal(presentedLeaf(moved), podLeaf), "4. the restarted pod serves the same leaf")
	status, _ = h.worker(http.MethodGet, "/v1/worker/rental/authorized-keys", worker, nil, podKey, time.Now().Unix(), nil)
	h.check(status == http.StatusOK && h.view(token, rental).State == "ready", "4. its key still signs (%d) and the rental is ready", status)
	h.check(strings.Contains(docker("exec", container, "cat", "/var/lib/cozy/machine/lease.json"), b64(renterKey)),
		"4. the restarted pod kept its lease")

	// The renter ends it; the provider's container goes.
	h.call(token, http.MethodDelete, "/v1/rentals/"+rental, nil, nil, 0, nil)
	until("the rental to be released", 3*time.Minute, func() bool { return h.view(token, rental).State == "released" })
	_, err = run("docker", "inspect", container)
	h.check(err != nil, "the released rental's container is gone")
}

// podImage is the pod image the Hub rents: ubuntu:24.04 (this host's glibc) plus the cozy binary
// under test, run as the worker images run it, written under the CPU tags the Hub resolves.
func (h *harness) podImage(cozyBin, repo string) string {
	base, err := remote.Image(must1(name.ParseReference(baseImage)), remote.WithPlatform(v1.Platform{OS: "linux", Architecture: "amd64"}))
	must(err, "pull "+baseImage)
	binary, err := os.ReadFile(cozyBin)
	must(err, "cozy binary")
	var layer bytes.Buffer
	files := tar.NewWriter(&layer)
	for _, dir := range []string{"usr/", "usr/local/", "usr/local/bin/"} {
		must(files.WriteHeader(&tar.Header{Name: dir, Typeflag: tar.TypeDir, Mode: 0o755}), "layer")
	}
	must(files.WriteHeader(&tar.Header{Name: "usr/local/bin/cozy", Mode: 0o755, Size: int64(len(binary))}), "layer")
	_, err = files.Write(binary)
	must(err, "layer")
	must(files.Close(), "layer")
	cozy, err := tarball.LayerFromOpener(func() (io.ReadCloser, error) { return io.NopCloser(bytes.NewReader(layer.Bytes())), nil })
	must(err, "layer")
	image := must1(mutate.AppendLayers(base, cozy))
	config := must1(image.ConfigFile()).DeepCopy()
	config.Config.Entrypoint = []string{"/usr/local/bin/cozy", "machine", "serve", "--rental"}
	config.Config.Cmd = nil
	config.Config.Env = append(config.Config.Env, "COZY_TENSORFS_ROOT=/var/lib/tensorfs")
	config.Config.Labels = map[string]string{"cozy.runtime": "0.22.0", "cozy.tensorfs": "0.22.0", "cozy.cozy": "0.22.0"}
	image = must1(mutate.ConfigFile(image, config))
	for _, tag := range []string{"cpu-linux-x86", "torch2.14.0-cpu-linux-x86"} {
		must(remote.Write(must1(name.NewTag(repo+":"+tag)), image), "push "+tag)
	}
	digest := must1(image.Digest()).String()
	h.later(func() { _, _ = run("docker", "rmi", "-f", repo+"@"+digest) })
	return digest
}

const baseImage = "index.docker.io/library/ubuntu:24.04"

func must1[T any](value T, err error) T {
	must(err, "image")
	return value
}

// ingress terminates TLS for the Hub on the Docker bridge under a fresh CA, which it writes.
func (h *harness) ingress(out, gateway, hub string) (string, string) {
	caKey, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	ca := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "podsecrets CA"},
		NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(24 * time.Hour), IsCA: true,
		BasicConstraintsValid: true, KeyUsage: x509.KeyUsageCertSign}
	caDER, err := x509.CreateCertificate(rand.Reader, ca, ca, &caKey.PublicKey, caKey)
	must(err, "CA")
	ca, _ = x509.ParseCertificate(caDER)
	leafKey, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	leaf := &x509.Certificate{SerialNumber: big.NewInt(2), Subject: pkix.Name{CommonName: gateway},
		NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(24 * time.Hour),
		IPAddresses: []net.IP{net.ParseIP(gateway)}, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		KeyUsage: x509.KeyUsageDigitalSignature}
	leafDER, err := x509.CreateCertificate(rand.Reader, leaf, ca, &leafKey.PublicKey, caKey)
	must(err, "ingress leaf")
	caPath := filepath.Join(out, "ca.pem")
	must(os.WriteFile(caPath, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: caDER}), 0o600), "ca.pem")
	listener, err := net.Listen("tcp", net.JoinHostPort(gateway, "0"))
	must(err, "ingress listen")
	target, _ := url.Parse(hub)
	server := &http.Server{Handler: httputil.NewSingleHostReverseProxy(target), TLSConfig: &tls.Config{
		Certificates: []tls.Certificate{{Certificate: [][]byte{leafDER}, PrivateKey: leafKey}}}}
	go func() { _ = server.ServeTLS(listener, "", "") }()
	h.later(func() { server.Close() })
	return "https://" + listener.Addr().String(), caPath
}

func (h *harness) serveHub(hubBin string, args []string, out string, file *os.File) {
	cmd := exec.Command(hubBin, append(args, "serve")...)
	cmd.Dir = out
	stderr, err := cmd.StderrPipe()
	must(err, "hub stderr")
	cmd.Stdout = file
	must(cmd.Start(), "tensorhub serve")
	go func() {
		lines := bufio.NewScanner(stderr)
		lines.Buffer(make([]byte, 0, 1<<20), 1<<24)
		for lines.Scan() {
			_, _ = file.Write(append(lines.Bytes(), '\n'))
			h.logMu.Lock()
			h.hubLog = append(h.hubLog, lines.Text())
			h.logMu.Unlock()
		}
	}()
	h.later(func() {
		_ = cmd.Process.Signal(syscall.SIGTERM)
		done := make(chan struct{})
		go func() { _ = cmd.Wait(); close(done) }()
		select {
		case <-done:
		case <-time.After(30 * time.Second):
			_ = cmd.Process.Kill()
		}
	})
	until("the Hub to serve", 2*time.Minute, func() bool {
		res, err := http.Get(h.hub + "/healthz")
		if err == nil {
			res.Body.Close()
		}
		return err == nil && res.StatusCode == http.StatusNoContent
	})
}

// enroll makes an account the way the CLI does: an Ed25519 device key enrolled with the code the
// development Hub logs in place of its email. It answers the device's public key and its token.
func (h *harness) enroll(name string) (ed25519.PublicKey, string) {
	public, private, _ := ed25519.GenerateKey(rand.Reader)
	email := name + "@podsecrets.test"
	var begin struct {
		EnrollmentID string `json:"enrollment_id"`
		Challenge    string `json:"challenge"`
	}
	h.call("", http.MethodPost, "/v1/auth/v1/device-keys/enroll/begin", map[string]string{"email": email,
		"public_key": b64(public), "label": "podsecrets"}, nil, http.StatusOK, &begin)
	challenge, err := base64.RawURLEncoding.DecodeString(begin.Challenge)
	must(err, "challenge")
	var code string
	until("the enrollment code", 30*time.Second, func() bool {
		h.logMu.Lock()
		defer h.logMu.Unlock()
		for _, line := range h.hubLog {
			var entry map[string]any
			if json.Unmarshal([]byte(line), &entry) == nil && entry["msg"] == "development device enrollment code" && entry["email"] == email {
				code, _ = entry["code"].(string)
			}
		}
		return code != ""
	})
	var finish struct {
		TokenSet struct {
			AccessToken string `json:"access_token"`
		} `json:"token_set"`
	}
	signature := ed25519.Sign(private, append(append([]byte("authkit.device-key-enrollment/1"), 0), challenge...))
	h.call("", http.MethodPost, "/v1/auth/v1/device-keys/enroll/finish", map[string]string{
		"enrollment_id": begin.EnrollmentID, "code": code, "signature": b64(signature)}, nil, http.StatusOK, &finish)
	token := finish.TokenSet.AccessToken
	h.call(token, http.MethodPut, "/v1/accounts/"+name, map[string]string{}, nil, http.StatusOK, nil)
	return public, token
}

type rentalView struct {
	State         string `json:"state"`
	Detail        string `json:"detail"`
	WorkerID      string `json:"worker_id"`
	WorkerAddress string `json:"worker_address"`
	CertPEM       string `json:"cert_pem"`
}

func (h *harness) view(token, rental string) rentalView {
	var v rentalView
	h.call(token, http.MethodGet, "/v1/rentals/"+rental, nil, nil, http.StatusOK, &v)
	return v
}

func (h *harness) untilReady(token, rental string, within time.Duration) rentalView {
	var v rentalView
	last := ""
	until("rental "+rental+" to be ready", within, func() bool {
		v = h.view(token, rental)
		if said := v.State + ": " + v.Detail; said != last {
			fmt.Printf("      %s %s\n", time.Now().Format("15:04:05"), said)
			last = said
		}
		if v.State == "failed" || v.State == "released" {
			panic("rental " + rental + " ended: " + v.Detail)
		}
		return v.State == "ready"
	})
	return v
}

type pinned struct {
	leaf, boot, leasedAt string
	leased               bool
}

// pin is what the Hub's database holds for the rental's attempt.
func (h *harness) pin(rental string) pinned {
	row := h.sql(`SELECT encode(sha256(machine_leaf_der),'hex'),COALESCE(machine_boot_id,''),COALESCE(machine_leased_at::text,'')
 FROM hub_v2.rental_acquisition_attempts WHERE rental_id='` + rental + `' ORDER BY ordinal DESC LIMIT 1`)
	fields := strings.Split(row, "|")
	if len(fields) != 3 {
		panic("no pin row: " + row)
	}
	return pinned{leaf: fields[0], boot: fields[1], leasedAt: fields[2], leased: fields[2] != ""}
}

func (h *harness) verifiedSince(rental string, at time.Time) bool {
	row := h.sql(`SELECT r.last_verified_at > '` + at.UTC().Format(time.RFC3339Nano) + `'::timestamptz
 FROM hub_v2.rental_readiness_receipts r JOIN hub_v2.rental_acquisition_attempts a ON a.id=r.acquisition_attempt_id
 WHERE a.rental_id='` + rental + `' ORDER BY a.ordinal DESC LIMIT 1`)
	return row == "t"
}

func (h *harness) sql(query string) string {
	return docker("exec", h.pg, "psql", "-U", "postgres", "-At", "-c", query)
}

// worker sends one worker request as a pod does, signed by key (none: unsigned) at unix.
func (h *harness) worker(method, path, worker string, body []byte, key *ecdsa.PrivateKey, unix int64,
	headers map[string]string,
) (int, string) {
	req, err := http.NewRequest(method, h.hub+path, bytes.NewReader(body))
	must(err, "worker request")
	req.Header.Set("X-Cozy-Worker-ID", worker)
	if key != nil {
		digest := sha256.Sum256(message(method, req.URL.RequestURI(), worker, unix, body))
		signature, err := ecdsa.SignASN1(rand.Reader, key, digest[:])
		must(err, "sign")
		req.Header.Set("X-Cozy-Worker-Time", strconv.FormatInt(unix, 10))
		req.Header.Set("X-Cozy-Worker-Signature", b64(signature))
	}
	for name, value := range headers {
		req.Header.Set(name, value)
	}
	res, err := http.DefaultClient.Do(req)
	must(err, "worker call")
	defer res.Body.Close()
	var refusal struct {
		Error struct {
			Code string `json:"code"`
		} `json:"error"`
	}
	raw, _ := io.ReadAll(res.Body)
	_ = json.Unmarshal(raw, &refusal)
	return res.StatusCode, refusal.Error.Code
}

// message is the text a worker request signs (Tensorhub internal/workerauth, hub-client
// worker_message): the harness speaks the wire contract as an independent peer.
func message(method, uri, worker string, unix int64, body []byte) []byte {
	sum := sha256.Sum256(body)
	return []byte(strings.Join([]string{"cozy.worker-request/1", method, uri, worker, strconv.FormatInt(unix, 10),
		hex.EncodeToString(sum[:])}, "\n"))
}

// call sends one request to the Hub as token (none: anonymous); want 0 accepts any 2xx.
func (h *harness) call(token, method, path string, body any, headers map[string]string, want int, out any) {
	var reader io.Reader
	if body != nil {
		raw, _ := json.Marshal(body)
		reader = bytes.NewReader(raw)
	}
	req, err := http.NewRequest(method, h.hub+path, reader)
	must(err, "request")
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	for name, value := range headers {
		req.Header.Set(name, value)
	}
	res, err := http.DefaultClient.Do(req)
	must(err, method+" "+path)
	defer res.Body.Close()
	answer, _ := io.ReadAll(res.Body)
	if want == 0 && res.StatusCode/100 != 2 || want != 0 && res.StatusCode != want {
		panic(fmt.Sprintf("%s %s: %d: %s", method, path, res.StatusCode, answer))
	}
	if out != nil {
		must(json.Unmarshal(answer, out), method+" "+path+" answer")
	}
}

// secretShaped is a name that would carry a credential.
var secretShaped = regexp.MustCompile(`(?i)TOKEN|SECRET|HMAC|PASSWORD|AUTHORIZED|PRIVATE|_KEY($|_)`)

// envNames answers an environment's names, and those shaped like a secret (the development SSH
// public key is public).
func envNames(vars []string) (names, secret []string) {
	for _, v := range vars {
		name, _, _ := strings.Cut(v, "=")
		if name == "" {
			continue
		}
		names = append(names, name)
		if secretShaped.MatchString(name) && name != "COZY_SSH_PUBLIC_KEY" {
			secret = append(secret, name)
		}
	}
	return names, secret
}

// presentedLeaf is the leaf whoever listens at address presents.
func presentedLeaf(address string) []byte {
	conn, err := tls.Dial("tcp", address, &tls.Config{InsecureSkipVerify: true}) // #nosec G402 -- read the leaf, as the Hub does
	must(err, "dial "+address)
	defer conn.Close()
	return conn.ConnectionState().PeerCertificates[0].Raw
}

func pemDER(text string) []byte {
	block, _ := pem.Decode([]byte(text))
	if block == nil {
		panic("no PEM block")
	}
	return block.Bytes
}

func pkcs8(text string) *ecdsa.PrivateKey {
	key, err := x509.ParsePKCS8PrivateKey(pemDER(text))
	must(err, "the pod's key")
	ecKey, ok := key.(*ecdsa.PrivateKey)
	if !ok {
		panic("the pod's key is not ECDSA")
	}
	return ecKey
}

func short(der []byte) string { return fmt.Sprintf("%x", sha256.Sum256(der))[:16] }

func b64(raw []byte) string { return base64.RawURLEncoding.EncodeToString(raw) }

func random(n int) []byte {
	raw := make([]byte, n)
	_, _ = rand.Read(raw)
	return raw
}

func freePort() int {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	must(err, "port")
	defer listener.Close()
	return listener.Addr().(*net.TCPAddr).Port
}

func until(what string, within time.Duration, ok func() bool) {
	deadline := time.Now().Add(within)
	for !ok() {
		if time.Now().After(deadline) {
			panic(what + ": not within " + within.String())
		}
		time.Sleep(250 * time.Millisecond)
	}
}

func run(name string, args ...string) (string, error) {
	out, err := exec.Command(name, args...).CombinedOutput()
	return string(out), err
}

func docker(args ...string) string {
	out, err := run("docker", args...)
	if err != nil {
		panic(fmt.Sprintf("docker %s: %v: %s", strings.Join(args, " "), err, out))
	}
	return strings.TrimSpace(out)
}

func must(err error, what string) {
	if err != nil {
		panic(fmt.Errorf("%s: %w", what, err))
	}
}

func abs(path string) string {
	out, err := filepath.Abs(path)
	must(err, path)
	return out
}
