package main

import (
	"fmt"
	"net"
	"net/url"
	"strings"
)

// verificationTarget rejects implicit targets, and CI can only use a local test Hub.
// The outer fixture guard additionally verifies resource/issuer metadata against proxies.
func verificationTarget(raw string, ci bool) (*url.URL, error) {
	target, err := url.Parse(strings.TrimRight(raw, "/"))
	if err != nil || target == nil || target.Hostname() == "" || (target.Scheme != "http" && target.Scheme != "https") || target.User != nil || target.RawQuery != "" || target.Fragment != "" || target.Path != "" {
		return nil, fmt.Errorf("--hub or COZY_HUB_URL must explicitly name an HTTP(S) Hub origin")
	}
	host := target.Hostname()
	ip := net.ParseIP(host)
	if ci && host != "localhost" && (ip == nil || !ip.IsLoopback()) {
		return nil, fmt.Errorf("CI Hub verification requires a loopback test Hub; production is forbidden")
	}
	return target, nil
}
