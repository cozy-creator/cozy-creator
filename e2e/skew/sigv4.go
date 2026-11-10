// sigv4 checks that an S3 store is as strict as R2 about SigV4's canonical URI (xs-166): a key holding ':' signed
// bare, as tensorhub v0.40.33-35 signed a workload's proof key, must be refused, and the same key signed with every
// reserved byte percent-encoded must be stored.
//
//	go run sigv4.go <endpoint> <access key> <secret> <bucket>
package main

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"
)

func main() {
	if len(os.Args) != 5 {
		fmt.Println("usage: sigv4 <endpoint> <access key> <secret> <bucket>")
		os.Exit(2)
	}
	endpoint, key, secret, bucket := os.Args[1], os.Args[2], os.Args[3], os.Args[4]
	object := "proofs/workloads/workload:sigv4-check/0"
	bare := put(endpoint, key, secret, "/"+bucket+"/"+object, false)
	encoded := put(endpoint, key, secret, "/"+bucket+"/"+object, true)
	if bare/100 == 2 || encoded/100 != 2 {
		fmt.Printf("a ':' key signed bare answered %d (wanted a refusal), signed encoded %d (wanted 2xx)\n", bare, encoded)
		os.Exit(1)
	}
	fmt.Printf("a ':' key signed bare is refused %d, signed %%3A stored %d\n", bare, encoded)
}

// put sends PUT path with an empty body; the canonical URI is the path bare or percent-encoded as SigV4 requires.
func put(endpoint, key, secret, path string, encode bool) int {
	u, _ := url.Parse(endpoint)
	canonical := path
	if encode {
		canonical = uriEncode(path)
	}
	now := time.Now().UTC()
	date, stamp := now.Format("20060102"), now.Format("20060102T150405Z")
	empty := sha256.Sum256(nil)
	payload := hex.EncodeToString(empty[:])
	headers := "host:" + u.Host + "\nx-amz-content-sha256:" + payload + "\nx-amz-date:" + stamp + "\n"
	signed := "host;x-amz-content-sha256;x-amz-date"
	request := strings.Join([]string{"PUT", canonical, "", headers, signed, payload}, "\n")
	scope := date + "/us-east-1/s3/aws4_request"
	digest := sha256.Sum256([]byte(request))
	toSign := "AWS4-HMAC-SHA256\n" + stamp + "\n" + scope + "\n" + hex.EncodeToString(digest[:])
	k := mac([]byte("AWS4"+secret), date)
	for _, part := range []string{"us-east-1", "s3", "aws4_request"} {
		k = mac(k, part)
	}
	req, _ := http.NewRequest(http.MethodPut, endpoint, http.NoBody)
	req.URL.Opaque = "//" + u.Host + canonical // sent exactly as signed
	req.Header.Set("x-amz-content-sha256", payload)
	req.Header.Set("x-amz-date", stamp)
	req.Header.Set("Authorization", fmt.Sprintf("AWS4-HMAC-SHA256 Credential=%s/%s, SignedHeaders=%s, Signature=%s",
		key, scope, signed, hex.EncodeToString(mac(k, toSign))))
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		fmt.Println(err)
		os.Exit(1)
	}
	resp.Body.Close()
	return resp.StatusCode
}

func mac(k []byte, s string) []byte {
	h := hmac.New(sha256.New, k)
	h.Write([]byte(s))
	return h.Sum(nil)
}

func uriEncode(s string) string {
	var b strings.Builder
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c >= 'A' && c <= 'Z' || c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || strings.IndexByte("-_.~/", c) >= 0 {
			b.WriteByte(c)
		} else {
			fmt.Fprintf(&b, "%%%02X", c)
		}
	}
	return b.String()
}
