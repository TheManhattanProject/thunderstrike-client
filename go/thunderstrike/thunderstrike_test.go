package thunderstrike

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"testing"
)

// The routing hash is the canary for whether this binding talks to the
// same implementation as every other: user:1 is shard 104 of 1024.
func TestShardOfIsPinned(t *testing.T) {
	got, err := ShardOf([]byte("user:1"), 1024)
	if err != nil || got != 104 {
		t.Fatalf("ShardOf(user:1, 1024) = %d, %v; want 104", got, err)
	}
}

// Every code classifies as the C ABI says, and Busy is the one that is
// always retriable.
func TestRetryClasses(t *testing.T) {
	if RetryClassOf(CodeBusy) != RetryAlways {
		t.Fatal("Busy must be RetryAlways")
	}
	for _, c := range []int32{CodeTimeout, CodeDisconnected, CodeConnect, CodeIO} {
		if RetryClassOf(c) != RetryIfIdempotent {
			t.Fatalf("code %d must be RetryIfIdempotent", c)
		}
	}
	for _, c := range []int32{CodeServerOOM, CodeStorageExhausted, CodeServerError, CodeMalformed, CodeTLS, CodeClosed} {
		if RetryClassOf(c) != RetryNever {
			t.Fatalf("code %d must be RetryNever", c)
		}
	}
	busy := &Error{Code: CodeBusy}
	if !errors.Is(busy, ErrBusy) || errors.Is(busy, ErrNotFound) {
		t.Fatal("errors.Is must match ErrBusy, and only it")
	}
	// The codes are the wire status bytes, and TLS moved off 13.
	if CodeBusy != 13 || CodeServerError != 14 || CodeStorageExhausted != 12 || CodeTLS != -13 {
		t.Fatal("code numbering drifted from thunderstrike.h")
	}
}

// A configuration without TLS is refused: the server is TLS-only.
func TestPlaintextIsRefused(t *testing.T) {
	_, err := Connect(Config{Endpoints: []string{"127.0.0.1:1"}})
	var e *Error
	if !errors.As(err, &e) || e.Code != CodeConfig {
		t.Fatalf("want a config error naming TLS, got %v", err)
	}
}

// Against a live server: THUNDERSTRIKE_TEST_ADDR=host:port, with THUNDERSTRIKE_CA
// (and THUNDERSTRIKE_SERVER_NAME) for the real, TLS-only server. Without
// THUNDERSTRIKE_CA the stand-in is assumed (plaintext). THUNDERSTRIKE_SMOKE_BIG=1
// adds a 128 MiB value (the default with TLS). THUNDERSTRIKE_SMOKE_MUX=off
// forces the legacy framing; by default every connection must have
// negotiated multiplexing and values over 1 MiB gone chunked.
func TestAgainstAServer(t *testing.T) {
	addr := os.Getenv("THUNDERSTRIKE_TEST_ADDR")
	if addr == "" {
		t.Skip("set THUNDERSTRIKE_TEST_ADDR (and THUNDERSTRIKE_CA for the real server)")
	}
	mux := os.Getenv("THUNDERSTRIKE_SMOKE_MUX") != "off"
	cfg := Config{Endpoints: []string{addr}, ConnectionsPerEndpoint: 2, DisableMultiplexing: !mux}
	ca := os.Getenv("THUNDERSTRIKE_CA")
	if ca != "" {
		pem, err := os.ReadFile(ca)
		if err != nil {
			t.Fatal(err)
		}
		cfg.TLSCAPem, cfg.TLSServerName = pem, os.Getenv("THUNDERSTRIKE_SERVER_NAME")
		if cfg.TLSServerName == "" {
			cfg.TLSServerName = "localhost"
		}
	} else {
		cfg.InsecurePlaintext = true
	}
	db, err := Connect(cfg)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()

	sizes := []int{1, 100, 4097, 65537, 1048577, 4<<20 + 1}
	if big := os.Getenv("THUNDERSTRIKE_SMOKE_BIG"); (big == "" && ca != "") || big == "1" {
		sizes = append(sizes, 128<<20)
	}
	for i, n := range sizes {
		k := []byte(fmt.Sprintf("go:test:%d", n))
		v := make([]byte, n)
		for j := range v {
			v[j] = byte(j*131 + i*7 + n)
		}
		if err := db.Set(k, v); err != nil {
			t.Fatalf("Set %d: %v", n, err)
		}
		got, found, err := db.Get(k)
		if err != nil || !found || !bytes.Equal(got, v) {
			t.Fatalf("Get %d: found=%v err=%v", n, found, err)
		}
		total, err := db.Append(k, []byte("tail"))
		if err != nil || total != uint64(n+4) {
			t.Fatalf("Append %d: %d %v", n, total, err)
		}
		many, err := db.MGet([][]byte{k, []byte("go:test:absent")})
		if err != nil || len(many) != 2 || len(many[0]) != n+4 || many[1] != nil {
			t.Fatalf("MGet %d: %v", n, err)
		}
		if ok, err := db.Delete(k); err != nil || !ok {
			t.Fatalf("Delete %d: %v %v", n, ok, err)
		}
		if _, found, _ := db.Get(k); found {
			t.Fatalf("Get after Delete %d: still there", n)
		}
	}

	// The server refuses an empty value as malformed: permanent.
	err = db.Set([]byte("go:test:empty"), nil)
	var e *Error
	if !errors.As(err, &e) || e.Code != CodeMalformed || e.RetryClass() != RetryNever {
		t.Fatalf("empty Set: %v", err)
	}

	// The framing that was asked for is the framing that was used.
	ms, err := db.MuxStats()
	if err != nil {
		t.Fatal(err)
	}
	st, _ := db.Stats()
	if mux {
		if ms.Connections != st.ConnectionsReady || ms.ChunkedRequests < 2 || ms.ChunkedReplies < 2 {
			t.Fatalf("multiplexing not in use: %+v (ready %d)", ms, st.ConnectionsReady)
		}
	} else if ms != (MuxStats{}) {
		t.Fatalf("legacy framing asked for, got %+v", ms)
	}
	t.Logf("framing: mux=%v %+v", mux, ms)
}
