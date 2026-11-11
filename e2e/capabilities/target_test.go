package main

import "testing"

func TestVerificationTarget(t *testing.T) {
	for _, raw := range []string{"", "https://tensorhub.com", "https://api.tensorhub.com", "http://127.0.0.1.example.com", "http://user:secret@127.0.0.1"} {
		if _, err := verificationTarget(raw, true); err == nil {
			t.Errorf("CI accepted %q", raw)
		}
	}
	for _, raw := range []string{"http://127.0.0.1:8819", "https://localhost:8443", "http://[::1]:8819"} {
		if _, err := verificationTarget(raw, true); err != nil {
			t.Errorf("local target %q: %v", raw, err)
		}
	}
	if _, err := verificationTarget("https://tensorhub.com", false); err != nil {
		t.Fatal("explicit manual target refused", err)
	}
	if _, err := verificationTarget("", false); err == nil {
		t.Fatal("manual invocation accepted an implicit target")
	}
}
