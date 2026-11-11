// ingress terminates TLS for the skew harness's Hub as a deployment's ingress does, under a fresh CA: machines reach the
// Hub at its https public origin (workload grants are served only there) and trust the CA the Hub hands them
// (rental.hub_ca_path). It writes <dir>/ca.pem, prints the origin, and proxies until it is ended.
//
//	go run ingress.go <listen 127.0.0.1:PORT> <hub http://127.0.0.1:8819> <dir>
package main

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"fmt"
	"log"
	"math/big"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"path/filepath"
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
	server := &http.Server{Handler: proxy, TLSConfig: &tls.Config{
		Certificates: []tls.Certificate{{Certificate: [][]byte{leafDER}, PrivateKey: leafKey}}}}
	fmt.Println("https://" + listener.Addr().String())
	log.Fatal(server.ServeTLS(listener, "", ""))
}
