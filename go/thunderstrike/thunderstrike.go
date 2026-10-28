// Package thunderstrike is a Go binding for the thunderstrike key-value database,
// over the client's C ABI.
//
// The compiled client is linked statically from lib/<goos_goarch>/: cgo
// is needed to build, nothing at run time.
//
// # Threading
//
// A Client is safe for concurrent use by multiple goroutines. Calls
// block the calling goroutine, and because cgo puts a blocking call on
// its own OS thread, the Go scheduler keeps running everything else.
// For high throughput prefer the batch calls (MGet, MSet, MDelete):
// they put the whole batch on the wire before awaiting any answer, so
// n keys cost one round trip instead of n.
package thunderstrike

/*
#include <stdlib.h>
#include "thunderstrike_client.h"
*/
import "C"

import (
	"errors"
	"fmt"
	"runtime"
	"unsafe"
)

// Status codes, mirroring thunderstrike_client.h. Positive codes are the
// status byte the server sent; negative ones are client-side.
const (
	codeOK       = C.THUNDERSTRIKE_OK
	codeNotFound = C.THUNDERSTRIKE_NOT_FOUND
	// The layout changed under a running client; reconnect.
	CodeWrongShard  = int32(C.THUNDERSTRIKE_WRONG_SHARD)
	CodeCASConflict = int32(C.THUNDERSTRIKE_CAS_CONFLICT) // never sent today
	// The server's memory is genuinely exhausted. Not applied.
	CodeServerOOM = int32(C.THUNDERSTRIKE_SERVER_OOM)
	// Never sent today: the server closes instead (it authenticates
	// with TLS).
	CodeNotAuthed          = int32(C.THUNDERSTRIKE_NOT_AUTHED)
	CodeUnknownOp          = int32(C.THUNDERSTRIKE_UNKNOWN_OP)
	CodeMalformed          = int32(C.THUNDERSTRIKE_MALFORMED) // e.g. an empty value
	CodeUnsupportedVersion = int32(C.THUNDERSTRIKE_UNSUPPORTED_VERSION)
	CodeResponseTooLarge   = int32(C.THUNDERSTRIKE_RESPONSE_TOO_LARGE)
	// The shard's disk is full, or a structural ceiling. Not applied.
	CodeStorageExhausted = int32(C.THUNDERSTRIKE_STORAGE_EXHAUSTED)
	// Not applied; the one retriable status.
	CodeBusy = int32(C.THUNDERSTRIKE_BUSY)
	// Integrity, device or invariant failure. Not retriable.
	CodeServerError   = int32(C.THUNDERSTRIKE_SERVER_ERROR)
	CodeUnknownStatus = int32(C.THUNDERSTRIKE_UNKNOWN_STATUS)

	CodeInvalidArg    = int32(C.THUNDERSTRIKE_ERR_INVALID_ARG)
	CodeConfig        = int32(C.THUNDERSTRIKE_ERR_CONFIG)
	CodeConnect       = int32(C.THUNDERSTRIKE_ERR_CONNECT)
	CodeIO            = int32(C.THUNDERSTRIKE_ERR_IO)
	CodeTimeout       = int32(C.THUNDERSTRIKE_ERR_TIMEOUT)
	CodeDisconnected  = int32(C.THUNDERSTRIKE_ERR_DISCONNECTED)
	CodeProtocol      = int32(C.THUNDERSTRIKE_ERR_PROTOCOL)
	CodeKeyTooLong    = int32(C.THUNDERSTRIKE_ERR_KEY_TOO_LONG)
	CodeClosed        = int32(C.THUNDERSTRIKE_ERR_CLOSED)
	CodeTLS           = int32(C.THUNDERSTRIKE_ERR_TLS) // was 13 before Busy took it
	CodeValueTooLarge = int32(C.THUNDERSTRIKE_ERR_VALUE_TOO_LARGE)

	RoutingShardByKey = int32(C.THUNDERSTRIKE_ROUTING_SHARD_BY_KEY)
	RoutingRoundRobin = int32(C.THUNDERSTRIKE_ROUTING_ROUND_ROBIN)
	MaxKeyLen         = int(C.THUNDERSTRIKE_MAX_KEY_LEN)

	// Error.RetryClass values.
	RetryNever        = int32(C.THUNDERSTRIKE_RETRY_NEVER)
	RetryIfIdempotent = int32(C.THUNDERSTRIKE_RETRY_IF_IDEMPOTENT) // GET/SET/EXISTS/PING only
	RetryAlways       = int32(C.THUNDERSTRIKE_RETRY_ALWAYS)        // Busy: not applied
)

// ErrNotFound is returned by APIs that promise a value. Get reports a
// miss with a false second return instead.
var ErrNotFound = errors.New("thunderstrike: key not found")

// ErrBusy matches (errors.Is) an Error whose code is CodeBusy: the
// server did not apply the request, and it may be sent again after a
// short backoff. The client has already retried it (every operation
// but Delete) before returning it.
var ErrBusy = errors.New("thunderstrike: server busy")

// Error is any non-OK status. Code is the numeric status.
type Error struct {
	Code int32
	Op   string

	// Detail is the specific cause, when the native side recorded one.
	//
	// The code names the category ("TLS failed"); this names the cause
	// ("X509(NameMismatch)"). For TLS that difference is the whole
	// diagnosis — a certificate issued for another name, an untrusted
	// issuer and an expired one share a code and have different fixes.
	Detail string
}

func (e *Error) Error() string {
	msg := C.GoString(C.thunderstrike_strerror(C.int32_t(e.Code)))
	where := "thunderstrike"
	if e.Op != "" {
		where = "thunderstrike: " + e.Op
	}
	if e.Detail != "" {
		return fmt.Sprintf("%s: %s (code %d): %s", where, msg, e.Code, e.Detail)
	}
	return fmt.Sprintf("%s: %s (code %d)", where, msg, e.Code)
}

// Is lets errors.Is(err, ErrNotFound) and errors.Is(err, ErrBusy) work.
func (e *Error) Is(target error) bool {
	return (target == ErrNotFound && e.Code == int32(codeNotFound)) ||
		(target == ErrBusy && e.Code == CodeBusy)
}

// RetryClass says whether repeating the request is sound: RetryAlways
// (Busy — not applied), RetryIfIdempotent (the outcome is unknown —
// Get, Set, Exists and Ping only) or RetryNever.
func (e *Error) RetryClass() int32 { return RetryClassOf(e.Code) }

// RetryClassOf is RetryClass for a bare code.
func RetryClassOf(code int32) int32 {
	return int32(C.thunderstrike_retry_class(C.int32_t(code)))
}

func check(rc C.int32_t, op string) error {
	if rc == C.int32_t(codeOK) {
		return nil
	}
	// Read the detail now: it is valid only until the next failing
	// call on this thread, and cgo gives no guarantee that the
	// goroutine stays on this one after the next call.
	return &Error{
		Code:   int32(rc),
		Op:     op,
		Detail: C.GoString(C.thunderstrike_last_error_message()),
	}
}

// Version reports the client library's version.
func Version() string { return C.GoString(C.thunderstrike_version()) }

// ShardOf reports which of `shards` shards owns key: CRC32C(key) %
// shards, the same function the server places data with. Useful for
// routing without a client in hand.
func ShardOf(key []byte, shards uint16) (uint16, error) {
	kp, kl, pin := bytesArg(key)
	defer runtime.KeepAlive(pin)
	var out C.uint16_t
	rc := C.thunderstrike_hash_shard(kp, kl, C.uint16_t(shards), &out)
	if err := check(rc, "ShardOf"); err != nil {
		return 0, err
	}
	return uint16(out), nil
}

// Config describes a pool. The zero value is not usable — it needs at
// least one entry in Endpoints.
type Config struct {
	// Endpoints to bootstrap from. ONE is enough: the client asks it
	// for the deployment's routing table and opens a connection to
	// every port that names.
	Endpoints []string

	// Host, BasePort, Shards and Cores STATE the layout of a server
	// started with --shards n --cores m: core c listens on
	// BasePort+c and owns a contiguous run of shards.
	//
	// Optional, and usually unnecessary — Endpoints plus discovery
	// finds the same thing. What this adds is a check: the stated
	// layout is compared against the one the server serves, and
	// Connect fails if they differ. Set all four together.
	Host     string
	BasePort uint16
	Shards   uint16
	Cores    uint16

	// TLSServerName and TLSCAPem turn on TLS 1.3, which the thunderstrike
	// server REQUIRES. Both are needed together; without them Connect
	// fails unless InsecurePlaintext is set.
	//
	// TLSServerName is sent as SNI AND matched against the
	// certificate's subject alternative names, so it must be a name
	// the certificate was issued for — not necessarily the host in
	// Endpoints. Dialling 10.0.0.1:7379 with a certificate for
	// db.internal means TLSServerName = "db.internal": the address
	// says where to dial, the name says who must answer.
	//
	// TLSCAPem is PEM text — the server's CA certificate
	// (ca.crt in its release kit). There is no way to disable verification.
	TLSServerName string
	TLSCAPem      []byte

	// TLSClientCertPem and TLSClientKeyPem present a client
	// certificate (mTLS) to a server started with
	// THUNDERSTRIKE_TLS_CLIENT_CA: the chain, leaf first, and the leaf's
	// PKCS#8 Ed25519 key, both PEM.
	TLSClientCertPem []byte
	TLSClientKeyPem  []byte

	// InsecurePlaintext connects WITHOUT TLS. Only the in-repo mock
	// server answers that; the thunderstrike server is TLS-only.
	InsecurePlaintext bool

	// BusyRetries: retries for a request the server answered Busy (not
	// applied), every operation but Delete. 0 keeps the default (4);
	// a negative value disables them.
	BusyRetries int
	// The Busy backoff (first wait, ceiling) and the window after which
	// no Busy retry starts. 0 keeps the defaults: 2 ms, 200 ms, 5000 ms.
	BusyBackoffMinMS  uint64
	BusyBackoffMaxMS  uint64
	BusyRetryWindowMS uint64
	// Extra deadline per MiB of a Set/Append value. 0 keeps the
	// default, 100 ms.
	TimeoutPerMiBMS uint64

	// Zero means "leave the client's default".
	ConnectionsPerEndpoint uint32
	MaxInflight            uint32
	MaxBufferedBytes       uint64
	MaxResponseBytes       uint64
	RequestTimeoutMS       uint64 // default 60000, above the server's own bounded waits
	ConnectTimeoutMS       uint64
	TCPKeepaliveMS         uint64
	MaxRetries             uint32
	WorkerThreads          uint32

	// The I/O thread pool. Connections are sockets; their readers and
	// writers are tasks, so 1024 connections are not 1024 threads.
	// These two decide how many threads — and so how many cores — the
	// client may use. The pool starts at MinIOThreads, divides the
	// connections among the threads running, and widens towards
	// MaxIOThreads when they saturate.
	//
	// Zero leaves the defaults: floor 1, ceiling the machine's cores,
	// clamped by how many connections there are to drive.
	MinIOThreads uint32
	MaxIOThreads uint32

	// How often the pool reconsiders its width, and how long it must
	// stay comfortable before giving a thread back.
	IOSampleIntervalMS uint64
	IOIdleTimeoutMS    uint64

	// Routing is RoutingShardByKey (default) or RoutingRoundRobin.
	Routing int32

	// DisableTCPNoDelay turns Nagle back on. Leave it false.
	DisableTCPNoDelay bool

	// DisableMultiplexing forces the legacy framing: no HELLO is sent.
	// By default every connection asks the server for frame
	// multiplexing, so a value larger than MuxChunkBytes travels in
	// chunks with other requests and replies between them, and a small
	// operation never waits behind a whole large one on its connection.
	// A server that does not grant it gets the legacy framing anyway.
	DisableMultiplexing bool
	// MuxChunkBytes is the chunk size for large values on a multiplexed
	// connection. 0 keeps the default, 1 MiB; held to the server's
	// grant (4 MiB today).
	MuxChunkBytes uint64
	// MuxMaxOpenBodies is how many chunked request bodies a multiplexed
	// connection keeps open on the wire at once. 0 means the default, 1:
	// one body's chunks run to its last before the next opens (small
	// requests still go between chunks). Held to the server's grant (4
	// today). More than one is unsafe against the server at 36db4921
	// under arena pressure: two bodies on one connection can wait on each
	// other.
	MuxMaxOpenBodies uint32
}

// Client is a pooled, pipelined thunderstrike client, safe for concurrent
// use. Close it when done.
type Client struct {
	h *C.thunderstrike_client
}

// Connect opens every connection in the pool (and completes its TLS
// handshake) before returning, so a bad address or a certificate that
// does not verify fails here.
func Connect(cfg Config) (*Client, error) {
	c := C.thunderstrike_config_new()
	if c == nil {
		return nil, errors.New("thunderstrike: out of memory")
	}
	defer C.thunderstrike_config_free(c)

	for _, addr := range cfg.Endpoints {
		ca := C.CString(addr)
		rc := C.thunderstrike_config_add_endpoint(c, ca)
		C.free(unsafe.Pointer(ca))
		if err := check(rc, "AddEndpoint"); err != nil {
			return nil, err
		}
	}
	if cfg.Host != "" {
		ch := C.CString(cfg.Host)
		rc := C.thunderstrike_config_set_cluster(c, ch,
			C.uint16_t(cfg.BasePort), C.uint16_t(cfg.Shards), C.uint16_t(cfg.Cores))
		C.free(unsafe.Pointer(ch))
		if err := check(rc, "SetCluster"); err != nil {
			return nil, err
		}
	}

	type u32set struct {
		v  uint32
		fn func(*C.thunderstrike_config, C.uint32_t) C.int32_t
		nm string
	}
	for _, s := range []u32set{
		{cfg.ConnectionsPerEndpoint, func(c *C.thunderstrike_config, v C.uint32_t) C.int32_t {
			return C.thunderstrike_config_set_connections_per_endpoint(c, v)
		}, "ConnectionsPerEndpoint"},
		{cfg.MaxInflight, func(c *C.thunderstrike_config, v C.uint32_t) C.int32_t {
			return C.thunderstrike_config_set_max_inflight(c, v)
		}, "MaxInflight"},
		{cfg.MaxRetries, func(c *C.thunderstrike_config, v C.uint32_t) C.int32_t {
			return C.thunderstrike_config_set_max_retries(c, v)
		}, "MaxRetries"},
		{cfg.WorkerThreads, func(c *C.thunderstrike_config, v C.uint32_t) C.int32_t {
			return C.thunderstrike_config_set_worker_threads(c, v)
		}, "WorkerThreads"},
		{cfg.MinIOThreads, func(c *C.thunderstrike_config, v C.uint32_t) C.int32_t {
			return C.thunderstrike_config_set_min_io_threads(c, v)
		}, "MinIOThreads"},
		{cfg.MaxIOThreads, func(c *C.thunderstrike_config, v C.uint32_t) C.int32_t {
			return C.thunderstrike_config_set_max_io_threads(c, v)
		}, "MaxIOThreads"},
	} {
		if s.v != 0 {
			if err := check(s.fn(c, C.uint32_t(s.v)), s.nm); err != nil {
				return nil, err
			}
		}
	}
	if cfg.MaxBufferedBytes != 0 {
		if err := check(C.thunderstrike_config_set_max_buffered_bytes(c, C.uint64_t(cfg.MaxBufferedBytes)), "MaxBufferedBytes"); err != nil {
			return nil, err
		}
	}
	if cfg.MaxResponseBytes != 0 {
		if err := check(C.thunderstrike_config_set_max_response_bytes(c, C.uint64_t(cfg.MaxResponseBytes)), "MaxResponseBytes"); err != nil {
			return nil, err
		}
	}
	if cfg.RequestTimeoutMS != 0 {
		if err := check(C.thunderstrike_config_set_request_timeout_ms(c, C.uint64_t(cfg.RequestTimeoutMS)), "RequestTimeout"); err != nil {
			return nil, err
		}
	}
	if cfg.TLSServerName != "" || len(cfg.TLSCAPem) > 0 {
		if cfg.TLSServerName == "" || len(cfg.TLSCAPem) == 0 {
			return nil, errors.New(
				"thunderstrike: TLSServerName and TLSCAPem must be set together — " +
					"a trust anchor with no name to check it against verifies nothing")
		}
		name := C.CString(cfg.TLSServerName)
		defer C.free(unsafe.Pointer(name))
		if err := check(C.thunderstrike_config_set_tls(
			c, name,
			(*C.uint8_t)(unsafe.Pointer(&cfg.TLSCAPem[0])),
			C.size_t(len(cfg.TLSCAPem)),
		), "TLS"); err != nil {
			return nil, err
		}
	}
	if len(cfg.TLSClientCertPem) > 0 || len(cfg.TLSClientKeyPem) > 0 {
		if len(cfg.TLSClientCertPem) == 0 || len(cfg.TLSClientKeyPem) == 0 {
			return nil, errors.New("thunderstrike: TLSClientCertPem and TLSClientKeyPem go together")
		}
		if err := check(C.thunderstrike_config_set_client_identity(
			c,
			(*C.uint8_t)(unsafe.Pointer(&cfg.TLSClientCertPem[0])), C.size_t(len(cfg.TLSClientCertPem)),
			(*C.uint8_t)(unsafe.Pointer(&cfg.TLSClientKeyPem[0])), C.size_t(len(cfg.TLSClientKeyPem)),
		), "ClientIdentity"); err != nil {
			return nil, err
		}
	}
	if cfg.InsecurePlaintext {
		if err := check(C.thunderstrike_config_set_insecure_plaintext(c, 1), "InsecurePlaintext"); err != nil {
			return nil, err
		}
	}
	if cfg.BusyRetries != 0 {
		n := cfg.BusyRetries
		if n < 0 {
			n = 0
		}
		if err := check(C.thunderstrike_config_set_busy_retries(c, C.uint32_t(n)), "BusyRetries"); err != nil {
			return nil, err
		}
	}
	if cfg.BusyBackoffMinMS != 0 || cfg.BusyBackoffMaxMS != 0 {
		lo, hi := cfg.BusyBackoffMinMS, cfg.BusyBackoffMaxMS
		if lo == 0 {
			lo = 2
		}
		if hi == 0 {
			hi = 200
		}
		if err := check(C.thunderstrike_config_set_busy_backoff_ms(c, C.uint64_t(lo), C.uint64_t(hi)), "BusyBackoff"); err != nil {
			return nil, err
		}
	}
	if cfg.BusyRetryWindowMS != 0 {
		if err := check(C.thunderstrike_config_set_busy_retry_window_ms(c, C.uint64_t(cfg.BusyRetryWindowMS)), "BusyRetryWindow"); err != nil {
			return nil, err
		}
	}
	if cfg.TimeoutPerMiBMS != 0 {
		if err := check(C.thunderstrike_config_set_timeout_per_mib_ms(c, C.uint64_t(cfg.TimeoutPerMiBMS)), "TimeoutPerMiB"); err != nil {
			return nil, err
		}
	}
	if cfg.IOSampleIntervalMS != 0 {
		if err := check(C.thunderstrike_config_set_io_sample_interval_ms(c, C.uint64_t(cfg.IOSampleIntervalMS)), "IOSampleInterval"); err != nil {
			return nil, err
		}
	}
	if cfg.IOIdleTimeoutMS != 0 {
		if err := check(C.thunderstrike_config_set_io_idle_timeout_ms(c, C.uint64_t(cfg.IOIdleTimeoutMS)), "IOIdleTimeout"); err != nil {
			return nil, err
		}
	}
	if cfg.ConnectTimeoutMS != 0 {
		if err := check(C.thunderstrike_config_set_connect_timeout_ms(c, C.uint64_t(cfg.ConnectTimeoutMS)), "ConnectTimeout"); err != nil {
			return nil, err
		}
	}
	if cfg.TCPKeepaliveMS != 0 {
		if err := check(C.thunderstrike_config_set_tcp_keepalive_ms(c, C.uint64_t(cfg.TCPKeepaliveMS)), "TCPKeepalive"); err != nil {
			return nil, err
		}
	}
	if cfg.Routing != 0 {
		if err := check(C.thunderstrike_config_set_routing(c, C.int32_t(cfg.Routing)), "Routing"); err != nil {
			return nil, err
		}
	}
	if cfg.DisableTCPNoDelay {
		if err := check(C.thunderstrike_config_set_tcp_nodelay(c, 0), "TCPNoDelay"); err != nil {
			return nil, err
		}
	}
	if cfg.DisableMultiplexing {
		if err := check(C.thunderstrike_config_set_multiplexing(c, 0), "Multiplexing"); err != nil {
			return nil, err
		}
	}
	if cfg.MuxChunkBytes != 0 {
		if err := check(C.thunderstrike_config_set_mux_chunk_bytes(c, C.uint64_t(cfg.MuxChunkBytes)), "MuxChunkBytes"); err != nil {
			return nil, err
		}
	}
	if cfg.MuxMaxOpenBodies != 0 {
		if err := check(C.thunderstrike_config_set_mux_max_open_bodies(c, C.uint32_t(cfg.MuxMaxOpenBodies)), "MuxMaxOpenBodies"); err != nil {
			return nil, err
		}
	}

	var h *C.thunderstrike_client
	if err := check(C.thunderstrike_connect(c, &h), "Connect"); err != nil {
		return nil, err
	}
	cl := &Client{h: h}
	// A forgotten Close should not leak the pool and its runtime
	// threads for the life of the process.
	runtime.SetFinalizer(cl, func(x *Client) { x.Close() })
	return cl, nil
}

// Close shuts the pool down. Idempotent.
func (c *Client) Close() {
	if c.h != nil {
		C.thunderstrike_client_free(c.h)
		c.h = nil
		runtime.SetFinalizer(c, nil)
	}
}

func (c *Client) handle() (*C.thunderstrike_client, error) {
	if c == nil || c.h == nil {
		return nil, &Error{Code: CodeClosed, Op: "use after Close"}
	}
	return c.h, nil
}

// bytesArg borrows a Go slice as (ptr, len) for a C call.
//
// An empty slice yields a nil pointer with length 0, which the ABI
// treats as an empty value without dereferencing. The third return
// must be kept alive across the call — cgo's rules allow passing a
// pointer into Go memory only for the duration of the call, and the
// KeepAlive is what tells the compiler the slice is still in use.
func bytesArg(b []byte) (*C.uint8_t, C.size_t, any) {
	if len(b) == 0 {
		return nil, 0, nil
	}
	return (*C.uint8_t)(unsafe.Pointer(&b[0])), C.size_t(len(b)), b
}

// copyOut copies a returned buffer into Go memory.
//
// Not C.GoBytes: that takes a C int, so a value of 2 GiB or more —
// the server stores values up to 4 GiB - 1 — would overflow it.
func copyOut(b C.thunderstrike_buf) []byte {
	n := int(b.len)
	if n == 0 || b.data == nil {
		return []byte{}
	}
	out := make([]byte, n)
	copy(out, unsafe.Slice((*byte)(unsafe.Pointer(b.data)), n))
	return out
}

// Get reads a key. The second return is false on a clean miss.
func (c *Client) Get(key []byte) ([]byte, bool, error) {
	h, err := c.handle()
	if err != nil {
		return nil, false, err
	}
	kp, kl, pin := bytesArg(key)
	var buf C.thunderstrike_buf
	rc := C.thunderstrike_get(h, kp, kl, &buf)
	runtime.KeepAlive(pin)
	// Always safe, whatever the outcome — and it releases the
	// client's hold on the read buffer.
	defer C.thunderstrike_buf_free(&buf)

	switch rc {
	case C.int32_t(codeOK):
		return copyOut(buf), true, nil
	case C.int32_t(codeNotFound):
		return nil, false, nil
	default:
		return nil, false, check(rc, "Get")
	}
}

// Set writes a key.
func (c *Client) Set(key, value []byte) error {
	return c.SetTTL(key, value, 0)
}

// SetTTL writes a key with a ttl_ms hint in the request header.
func (c *Client) SetTTL(key, value []byte, ttlMS uint32) error {
	h, err := c.handle()
	if err != nil {
		return err
	}
	kp, kl, kpin := bytesArg(key)
	vp, vl, vpin := bytesArg(value)
	rc := C.thunderstrike_set_ttl(h, kp, kl, vp, vl, C.uint32_t(ttlMS))
	runtime.KeepAlive(kpin)
	runtime.KeepAlive(vpin)
	return check(rc, "Set")
}

// Delete removes a key, reporting whether it was there.
func (c *Client) Delete(key []byte) (bool, error) {
	h, err := c.handle()
	if err != nil {
		return false, err
	}
	kp, kl, pin := bytesArg(key)
	rc := C.thunderstrike_delete(h, kp, kl)
	runtime.KeepAlive(pin)
	switch rc {
	case C.int32_t(codeOK):
		return true, nil
	case C.int32_t(codeNotFound):
		return false, nil
	default:
		return false, check(rc, "Delete")
	}
}

// Exists tests for a key without transferring its value.
func (c *Client) Exists(key []byte) (bool, error) {
	h, err := c.handle()
	if err != nil {
		return false, err
	}
	kp, kl, pin := bytesArg(key)
	rc := C.thunderstrike_exists(h, kp, kl)
	runtime.KeepAlive(pin)
	switch rc {
	case C.int32_t(codeOK):
		return true, nil
	case C.int32_t(codeNotFound):
		return false, nil
	default:
		return false, check(rc, "Exists")
	}
}

// Append adds to a key's value, returning its new total length.
func (c *Client) Append(key, value []byte) (uint64, error) {
	h, err := c.handle()
	if err != nil {
		return 0, err
	}
	kp, kl, kpin := bytesArg(key)
	vp, vl, vpin := bytesArg(value)
	var total C.uint64_t
	rc := C.thunderstrike_append(h, kp, kl, vp, vl, &total)
	runtime.KeepAlive(kpin)
	runtime.KeepAlive(vpin)
	if err := check(rc, "Append"); err != nil {
		return 0, err
	}
	return uint64(total), nil
}

// Ping round-trips a liveness probe.
func (c *Client) Ping() error {
	h, err := c.handle()
	if err != nil {
		return err
	}
	return check(C.thunderstrike_ping(h), "Ping")
}

// batchArgs builds the parallel pointer/length arrays the batch ABI
// takes. The arrays are C memory (freed by the returned func) while
// the pointers inside refer to the caller's Go slices, which the
// caller keeps alive for the call.
func batchArgs(items [][]byte) (**C.uint8_t, *C.size_t, func()) {
	n := len(items)
	ptrSize := C.size_t(unsafe.Sizeof(uintptr(0)))
	lenSize := C.size_t(unsafe.Sizeof(C.size_t(0)))
	ptrs := C.malloc(ptrSize * C.size_t(n))
	lens := C.malloc(lenSize * C.size_t(n))

	ptrSlice := unsafe.Slice((**C.uint8_t)(ptrs), n)
	lenSlice := unsafe.Slice((*C.size_t)(lens), n)
	for i, b := range items {
		if len(b) == 0 {
			ptrSlice[i] = nil
			lenSlice[i] = 0
			continue
		}
		ptrSlice[i] = (*C.uint8_t)(unsafe.Pointer(&b[0]))
		lenSlice[i] = C.size_t(len(b))
	}
	return (**C.uint8_t)(ptrs), (*C.size_t)(lens), func() {
		C.free(ptrs)
		C.free(lens)
	}
}

// MGet reads many keys in ONE round trip. A nil entry is a miss.
//
// Prefer this to a loop over Get for anything above a couple of keys:
// the whole batch goes on the wire before any answer is awaited.
func (c *Client) MGet(keys [][]byte) ([][]byte, error) {
	h, err := c.handle()
	if err != nil {
		return nil, err
	}
	n := len(keys)
	if n == 0 {
		return nil, nil
	}
	kp, kl, free := batchArgs(keys)
	defer free()

	bufs := (*C.thunderstrike_buf)(C.calloc(C.size_t(n), C.size_t(unsafe.Sizeof(C.thunderstrike_buf{}))))
	defer C.free(unsafe.Pointer(bufs))
	codes := (*C.int32_t)(C.malloc(C.size_t(n) * C.size_t(unsafe.Sizeof(C.int32_t(0)))))
	defer C.free(unsafe.Pointer(codes))

	rc := C.thunderstrike_mget(h, kp, kl, C.size_t(n), bufs, codes)
	runtime.KeepAlive(keys)

	bufSlice := unsafe.Slice(bufs, n)
	defer func() {
		for i := range bufSlice {
			C.thunderstrike_buf_free(&bufSlice[i])
		}
	}()
	if err := check(rc, "MGet"); err != nil {
		return nil, err
	}

	codeSlice := unsafe.Slice(codes, n)
	out := make([][]byte, n)
	for i := 0; i < n; i++ {
		switch codeSlice[i] {
		case C.int32_t(codeOK):
			out[i] = copyOut(bufSlice[i])
		case C.int32_t(codeNotFound):
			out[i] = nil
		default:
			return nil, &Error{Code: int32(codeSlice[i]), Op: fmt.Sprintf("MGet[%d]", i)}
		}
	}
	return out, nil
}

// MSet writes many pairs in ONE round trip.
func (c *Client) MSet(keys, values [][]byte) error {
	h, err := c.handle()
	if err != nil {
		return err
	}
	if len(keys) != len(values) {
		return errors.New("thunderstrike: MSet needs as many values as keys")
	}
	n := len(keys)
	if n == 0 {
		return nil
	}
	kp, kl, freeK := batchArgs(keys)
	defer freeK()
	vp, vl, freeV := batchArgs(values)
	defer freeV()

	codes := (*C.int32_t)(C.malloc(C.size_t(n) * C.size_t(unsafe.Sizeof(C.int32_t(0)))))
	defer C.free(unsafe.Pointer(codes))

	rc := C.thunderstrike_mset(h, kp, kl, vp, vl, C.size_t(n), codes)
	runtime.KeepAlive(keys)
	runtime.KeepAlive(values)
	if err := check(rc, "MSet"); err != nil {
		return err
	}
	for i, code := range unsafe.Slice(codes, n) {
		if code != C.int32_t(codeOK) {
			return &Error{Code: int32(code), Op: fmt.Sprintf("MSet[%d]", i)}
		}
	}
	return nil
}

// MDelete removes many keys in ONE round trip, reporting which were
// there.
func (c *Client) MDelete(keys [][]byte) ([]bool, error) {
	h, err := c.handle()
	if err != nil {
		return nil, err
	}
	n := len(keys)
	if n == 0 {
		return nil, nil
	}
	kp, kl, free := batchArgs(keys)
	defer free()
	codes := (*C.int32_t)(C.malloc(C.size_t(n) * C.size_t(unsafe.Sizeof(C.int32_t(0)))))
	defer C.free(unsafe.Pointer(codes))

	rc := C.thunderstrike_mdelete(h, kp, kl, C.size_t(n), codes)
	runtime.KeepAlive(keys)
	if err := check(rc, "MDelete"); err != nil {
		return nil, err
	}
	out := make([]bool, n)
	for i, code := range unsafe.Slice(codes, n) {
		switch code {
		case C.int32_t(codeOK):
			out[i] = true
		case C.int32_t(codeNotFound):
			out[i] = false
		default:
			return nil, &Error{Code: int32(code), Op: fmt.Sprintf("MDelete[%d]", i)}
		}
	}
	return out, nil
}

// Stats holds the pool's counters.
type Stats struct {
	Connections      uint64
	ConnectionsReady uint64
	Inflight         uint64
	Capacity         uint64
	Requests         uint64
	Responses        uint64
	BytesWritten     uint64
	BytesRead        uint64
	WriteBatches     uint64
	Reconnects       uint64
	OrphanResponses  uint64

	// The I/O pool: how many threads are running, the range they may
	// move in, and how often the pool has reshaped. IOThreads is the
	// number of cores this client can be occupying — the connection
	// count is not.
	IOThreads    uint64
	IOThreadsMin uint64
	IOThreadsMax uint64
	IOGrew       uint64
	IOShrank     uint64
	IOMoved      uint64
}

// BatchingFactor is requests per write syscall. Higher is better.
func (s Stats) BatchingFactor() float64 {
	if s.WriteBatches == 0 {
		return 0
	}
	return float64(s.Requests) / float64(s.WriteBatches)
}

// Stats reports the pool's counters.
func (c *Client) Stats() (Stats, error) {
	h, err := c.handle()
	if err != nil {
		return Stats{}, err
	}
	var raw C.thunderstrike_stats
	if err := check(C.thunderstrike_get_stats(h, &raw), "Stats"); err != nil {
		return Stats{}, err
	}
	return Stats{
		Connections:      uint64(raw.connections),
		ConnectionsReady: uint64(raw.connections_ready),
		Inflight:         uint64(raw.inflight),
		Capacity:         uint64(raw.capacity),
		Requests:         uint64(raw.requests),
		Responses:        uint64(raw.responses),
		BytesWritten:     uint64(raw.bytes_written),
		BytesRead:        uint64(raw.bytes_read),
		WriteBatches:     uint64(raw.write_batches),
		Reconnects:       uint64(raw.reconnects),
		OrphanResponses:  uint64(raw.orphan_responses),
		IOThreads:        uint64(raw.io_threads),
		IOThreadsMin:     uint64(raw.io_threads_min),
		IOThreadsMax:     uint64(raw.io_threads_max),
		IOGrew:           uint64(raw.io_grew),
		IOShrank:         uint64(raw.io_shrank),
		IOMoved:          uint64(raw.io_moved),
	}, nil
}

// MuxStats are the frame multiplexing counters.
type MuxStats struct {
	// Connections that negotiated multiplexing and are up now.
	Connections uint64
	// Requests whose value went in chunks.
	ChunkedRequests uint64
	// Replies that came in chunks.
	ChunkedReplies uint64
}

// MuxStats reports the frame multiplexing counters.
func (c *Client) MuxStats() (MuxStats, error) {
	h, err := c.handle()
	if err != nil {
		return MuxStats{}, err
	}
	var conns, req, rep C.uint64_t
	if err := check(C.thunderstrike_get_mux_stats(h, &conns, &req, &rep), "MuxStats"); err != nil {
		return MuxStats{}, err
	}
	return MuxStats{Connections: uint64(conns), ChunkedRequests: uint64(req), ChunkedReplies: uint64(rep)}, nil
}

// Shards reports how many shards the deployment has, as the server
// stated it. 0 under round-robin routing, which has no shards.
func (c *Client) Shards() (uint16, error) {
	h, err := c.handle()
	if err != nil {
		return 0, err
	}
	var out C.uint16_t
	if err := check(C.thunderstrike_shard_count(h, &out), "Shards"); err != nil {
		return 0, err
	}
	return uint16(out), nil
}

// Endpoints reports how many endpoints this client is connected to —
// one per server core, discovered rather than configured.
func (c *Client) Endpoints() (uint32, error) {
	h, err := c.handle()
	if err != nil {
		return 0, err
	}
	var out C.uint32_t
	if err := check(C.thunderstrike_endpoint_count(h, &out), "Endpoints"); err != nil {
		return 0, err
	}
	return uint32(out), nil
}

// ShardOf reports which shard this client routes key to.
func (c *Client) ShardOf(key []byte) (uint16, error) {
	h, err := c.handle()
	if err != nil {
		return 0, err
	}
	kp, kl, pin := bytesArg(key)
	var out C.uint16_t
	rc := C.thunderstrike_shard_of(h, kp, kl, &out)
	runtime.KeepAlive(pin)
	if err := check(rc, "ShardOf"); err != nil {
		return 0, err
	}
	return uint16(out), nil
}
