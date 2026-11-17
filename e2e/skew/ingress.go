// ingress terminates TLS for the skew harness's Hub as a deployment's ingress does, under a fresh CA: machines reach the
// Hub at its https public origin (workload grants are served only there) and trust the CA the Hub hands them
// (rental.hub_ca_path). It writes <dir>/ca.pem, prints the origin, and proxies until it is ended. While <dir>/hold-grants
// exists it stands in for a Hub that takes a machine's capability redemption (the jwt-bearer grant) and answers nothing
// until its client gives up (cl-1189), each one a `held` line in <dir>/requests.jsonl.
//
//	go run ingress.go <listen 127.0.0.1:PORT> <hub http://127.0.0.1:8819> <dir>
package main

import (
	"bytes"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"io"
	"log"
	"math/big"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

func main() {
	if len(os.Args) != 4 {
		log.Fatal("usage: ingress <listen> <hub> <dir>")
	}
	listen, hub, dir := os.Args[1], os.Args[2], os.Args[3]
	host, _, err := net.SplitHostPort(listen)
	if err != nil {
		log.Fatal(err)
	}
	caKey, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	ca := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "skew CA"},
		NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(24 * time.Hour), IsCA: true,
		BasicConstraintsValid: true, KeyUsage: x509.KeyUsageCertSign}
	caDER, err := x509.CreateCertificate(rand.Reader, ca, ca, &caKey.PublicKey, caKey)
	if err != nil {
		log.Fatal(err)
	}
	ca, _ = x509.ParseCertificate(caDER)
	leafKey, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	leaf := &x509.Certificate{SerialNumber: big.NewInt(2), Subject: pkix.Name{CommonName: host},
		NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(24 * time.Hour),
		IPAddresses: []net.IP{net.ParseIP(host)}, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		KeyUsage: x509.KeyUsageDigitalSignature}
	leafDER, err := x509.CreateCertificate(rand.Reader, leaf, ca, &leafKey.PublicKey, caKey)
	if err != nil {
		log.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "ca.pem"), pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: caDER}), 0o600); err != nil {
		log.Fatal(err)
	}
	listener, err := net.Listen("tcp", listen)
	if err != nil {
		log.Fatal(err)
	}
	target, _ := url.Parse(hub)
	proxy := httputil.NewSingleHostReverseProxy(target)
	trace, err := os.OpenFile(filepath.Join(dir, "requests.jsonl"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		log.Fatal(err)
	}
	var traceMu sync.Mutex
	proxy.ModifyResponse = func(response *http.Response) error {
		traceMu.Lock()
		defer traceMu.Unlock()
		// Evidence of scoped origin reads without credentials, bodies or query strings.
		return json.NewEncoder(trace).Encode(map[string]any{
			"method": response.Request.Method, "path": response.Request.URL.EscapedPath(), "status": response.StatusCode,
		})
	}
	sdk := http.StripPrefix("/__cozy_sdk/", http.FileServer(http.Dir(filepath.Join(dir, "sdk"))))
	hold := filepath.Join(dir, "hold-grants")
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if _, err := os.Stat(hold); err == nil && r.Method == http.MethodPost {
			body, _ := io.ReadAll(io.LimitReader(r.Body, 1<<20))
			r.Body = io.NopCloser(bytes.NewReader(body))
			if form, err := url.ParseQuery(string(body)); err == nil && form.Get("grant_type") == "urn:ietf:params:oauth:grant-type:jwt-bearer" {
				traceMu.Lock()
				_ = json.NewEncoder(trace).Encode(map[string]any{"method": r.Method, "path": r.URL.EscapedPath(), "held": true, "at": time.Now().UnixMilli()})
				traceMu.Unlock()
				<-r.Context().Done()
				return
			}
		}
		if !strings.HasPrefix(r.URL.Path, "/__cozy_sdk/") {
			proxy.ServeHTTP(w, r)
			return
		}
		observed := &statusWriter{ResponseWriter: w, status: http.StatusOK}
		sdk.ServeHTTP(observed, r)
		traceMu.Lock()
		defer traceMu.Unlock()
		_ = json.NewEncoder(trace).Encode(map[string]any{"method": r.Method, "path": r.URL.EscapedPath(), "status": observed.status})
	})
	server := &http.Server{Handler: handler, TLSConfig: &tls.Config{
		Certificates: []tls.Certificate{{Certificate: [][]byte{leafDER}, PrivateKey: leafKey}}}}
	fmt.Println("https://" + listener.Addr().String())
	log.Fatal(server.ServeTLS(listener, "", ""))
}

type statusWriter struct {
	http.ResponseWriter
	status int
}

func (w *statusWriter) WriteHeader(status int) {
	w.status = status
	w.ResponseWriter.WriteHeader(status)
}
