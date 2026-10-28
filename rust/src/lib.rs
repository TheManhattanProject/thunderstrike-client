//! Client for the thunderstrike key-value database.
//!
//! A [`Client`] is a pool of pipelined connections to a thunderstrike
//! deployment. Give it one endpoint: it asks that address for the routing
//! table and connects to every core the deployment runs.
//!
//! ```no_run
//! use thunderstrike::{Client, Config};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // The server speaks TLS 1.3 only: the CA that issued its certificate,
//! // and the name the certificate was issued for.
//! let ca = std::fs::read("ca.crt")?;
//! let db = Client::connect(&Config::new("127.0.0.1:7379").tls("localhost", ca))?;
//!
//! db.set(b"user:1", b"ada")?;
//! assert_eq!(db.get(b"user:1")?.as_deref(), Some(&b"ada"[..]));
//!
//! // One round trip for many keys. Use it wherever you would loop.
//! let values = db.mget(&[&b"user:1"[..], b"user:2"])?;
//! # let _ = values;
//! # Ok(())
//! # }
//! ```
//!
//! # Threading
//!
//! A [`Client`] is `Send + Sync`: share it (`&Client`, or an `Arc<Client>`)
//! across threads and call it concurrently; the library synchronises
//! internally. The blocking calls park the calling thread until the answer
//! arrives, so inside an async runtime use the `*_async` methods (or the
//! runtime's `spawn_blocking`). The futures they return work on any
//! executor: they need no particular runtime.
//!
//! # Retries
//!
//! The library has already applied the retry policy before an error
//! reaches you: a [`code::BUSY`] answer is retried with backoff (every
//! operation but delete), and a dropped connection is retried for the
//! idempotent operations. [`Error::retry_class`] says whether repeating it
//! yourself is sound.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod sys;

use std::collections::HashSet;
use std::ffi::{c_void, CStr, CString};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

/// Status codes. Positive ones are the status byte the server sent;
/// negative ones are conditions the client detected.
pub mod code {
    use crate::sys;

    /// Success.
    pub const OK: i32 = sys::THUNDERSTRIKE_OK;
    /// The key is absent. Reported as `None`/`false`, never as an [`Error`](crate::Error).
    pub const NOT_FOUND: i32 = sys::THUNDERSTRIKE_NOT_FOUND;
    /// The deployment's layout changed under a running client; reconnect.
    pub const WRONG_SHARD: i32 = sys::THUNDERSTRIKE_WRONG_SHARD;
    /// Never sent by the current server.
    pub const CAS_CONFLICT: i32 = sys::THUNDERSTRIKE_CAS_CONFLICT;
    /// The server's memory is exhausted. Not applied; back off for seconds.
    pub const SERVER_OOM: i32 = sys::THUNDERSTRIKE_SERVER_OOM;
    /// Never sent by the current server: it authenticates with TLS.
    pub const NOT_AUTHED: i32 = sys::THUNDERSTRIKE_NOT_AUTHED;
    /// The server does not implement the operation.
    pub const UNKNOWN_OP: i32 = sys::THUNDERSTRIKE_UNKNOWN_OP;
    /// Refused as sent: e.g. an empty value, or a value over the server's ceiling.
    pub const MALFORMED: i32 = sys::THUNDERSTRIKE_MALFORMED;
    /// The server does not speak this protocol version.
    pub const UNSUPPORTED_VERSION: i32 = sys::THUNDERSTRIKE_UNSUPPORTED_VERSION;
    /// The response exceeded the configured ceiling.
    pub const RESPONSE_TOO_LARGE: i32 = sys::THUNDERSTRIKE_RESPONSE_TOO_LARGE;
    /// The shard's disk is full or hit a structural ceiling. Not applied.
    pub const STORAGE_EXHAUSTED: i32 = sys::THUNDERSTRIKE_STORAGE_EXHAUSTED;
    /// Momentarily out of queue; NOT applied. The one always-retriable status.
    pub const BUSY: i32 = sys::THUNDERSTRIKE_BUSY;
    /// Integrity, device or invariant failure on the server. Not retriable.
    pub const SERVER_ERROR: i32 = sys::THUNDERSTRIKE_SERVER_ERROR;
    /// A status byte this client does not know.
    pub const UNKNOWN_STATUS: i32 = sys::THUNDERSTRIKE_UNKNOWN_STATUS;

    /// An argument was invalid (e.g. a string with an interior NUL).
    pub const INVALID_ARG: i32 = sys::THUNDERSTRIKE_ERR_INVALID_ARG;
    /// The configuration was refused (e.g. neither TLS nor plaintext).
    pub const CONFIG: i32 = sys::THUNDERSTRIKE_ERR_CONFIG;
    /// A connection could not be established.
    pub const CONNECT: i32 = sys::THUNDERSTRIKE_ERR_CONNECT;
    /// A socket error.
    pub const IO: i32 = sys::THUNDERSTRIKE_ERR_IO;
    /// The request deadline passed; it may still have executed.
    pub const TIMEOUT: i32 = sys::THUNDERSTRIKE_ERR_TIMEOUT;
    /// The connection dropped with the request in flight.
    pub const DISCONNECTED: i32 = sys::THUNDERSTRIKE_ERR_DISCONNECTED;
    /// The server sent something that is not the protocol.
    pub const PROTOCOL: i32 = sys::THUNDERSTRIKE_ERR_PROTOCOL;
    /// The key is longer than [`MAX_KEY_LEN`](crate::MAX_KEY_LEN).
    pub const KEY_TOO_LONG: i32 = sys::THUNDERSTRIKE_ERR_KEY_TOO_LONG;
    /// The client was closed.
    pub const CLOSED: i32 = sys::THUNDERSTRIKE_ERR_CLOSED;
    /// A caller-supplied buffer was too small.
    pub const BUFFER_TOO_SMALL: i32 = sys::THUNDERSTRIKE_ERR_BUFFER_TOO_SMALL;
    /// A blocking call was made from a client worker thread.
    pub const RUNTIME: i32 = sys::THUNDERSTRIKE_ERR_RUNTIME;
    /// The library caught an internal panic.
    pub const PANIC: i32 = sys::THUNDERSTRIKE_ERR_PANIC;
    /// The TLS handshake failed or a record did not decrypt. See [`Error::message`](crate::Error::message).
    pub const TLS: i32 = sys::THUNDERSTRIKE_ERR_TLS;
    /// The value is longer than one request carries (4 GiB - 1).
    pub const VALUE_TOO_LARGE: i32 = sys::THUNDERSTRIKE_ERR_VALUE_TOO_LARGE;
}

/// The longest key the wire carries.
pub const MAX_KEY_LEN: usize = sys::THUNDERSTRIKE_MAX_KEY_LEN;

/// Shorthand for this crate's results.
pub type Result<T, E = Error> = std::result::Result<T, E>;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Whether a failed request may be sent again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryClass {
    /// The same request fails the same way, or must not be repeated.
    Never,
    /// The outcome is unknown (connection dropped, deadline passed):
    /// repeat only get, set, exists and ping.
    IfIdempotent,
    /// Not applied ([`code::BUSY`]): repeat any operation after a short backoff.
    Always,
}

impl RetryClass {
    /// The class of a bare status code.
    pub fn of(code: i32) -> Self {
        // SAFETY: a pure function of its argument.
        match unsafe { sys::thunderstrike_retry_class(code) } {
            sys::THUNDERSTRIKE_RETRY_ALWAYS => RetryClass::Always,
            sys::THUNDERSTRIKE_RETRY_IF_IDEMPOTENT => RetryClass::IfIdempotent,
            _ => RetryClass::Never,
        }
    }
}

/// Any status other than success (and other than not-found, which the
/// API reports as `None` or `false`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    code: i32,
    message: String,
}

impl Error {
    /// The numeric status: one of the [`code`] constants.
    pub fn code(&self) -> i32 {
        self.code
    }

    /// The specific cause, when the library recorded one; otherwise empty.
    ///
    /// The code names the category ("TLS failed"); this names the cause
    /// ("X509(NameMismatch)"). For TLS that is the whole diagnosis: a
    /// certificate for another name, an untrusted issuer and an expired
    /// one share a code and have different fixes.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Whether repeating the request is sound.
    pub fn retry_class(&self) -> RetryClass {
        RetryClass::of(self.code)
    }

    /// The server did not apply the request; it may be sent again.
    pub fn is_busy(&self) -> bool {
        self.code == code::BUSY
    }

    /// The deadline passed; the request may still have executed.
    pub fn is_timeout(&self) -> bool {
        self.code == code::TIMEOUT
    }

    /// The connection dropped with the request in flight.
    pub fn is_disconnected(&self) -> bool {
        self.code == code::DISCONNECTED
    }

    /// The TLS handshake or a record failed.
    pub fn is_tls(&self) -> bool {
        self.code == code::TLS
    }

    fn new(code: i32, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    /// Read this thread's detail now: it lasts only until the next failing call.
    fn last(code: i32) -> Self {
        // SAFETY: never null, NUL-terminated, valid until the next failing
        // call on this thread, and copied before that.
        let detail = unsafe { CStr::from_ptr(sys::thunderstrike_last_error_message()) };
        Self::new(code, detail.to_string_lossy())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "thunderstrike: {} (code {})", strerror(self.code), self.code)?;
        if !self.message.is_empty() {
            write!(f, ": {}", self.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

fn check(rc: i32) -> Result<()> {
    if rc == code::OK {
        Ok(())
    } else {
        Err(Error::last(rc))
    }
}

/// The static description of a status code.
pub fn strerror(code: i32) -> &'static str {
    // SAFETY: static, NUL-terminated, never null.
    unsafe { CStr::from_ptr(sys::thunderstrike_strerror(code)) }
        .to_str()
        .unwrap_or("unknown status")
}

/// The native library's version.
pub fn version() -> &'static str {
    // SAFETY: static, NUL-terminated, never null.
    unsafe { CStr::from_ptr(sys::thunderstrike_version()) }
        .to_str()
        .unwrap_or("")
}

/// Which of `shards` shards owns `key`: CRC32C(key) % shards, the same
/// function the server places data with. For callers that route
/// themselves. `shards` of 0 reads as 1.
pub fn hash_shard(key: &[u8], shards: u16) -> Result<u16> {
    let mut out = 0u16;
    // SAFETY: key is readable for its length; out is writable.
    check(unsafe { sys::thunderstrike_hash_shard(key.as_ptr(), key.len(), shards, &mut out) })?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// How requests are spread over the connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Routing {
    /// Each key goes to the core that owns its shard. The default.
    #[default]
    ShardByKey,
    /// Requests rotate over connections; for servers without shards.
    RoundRobin,
}

/// TLS 1.3 settings, which the thunderstrike server requires.
#[derive(Clone)]
pub struct Tls {
    /// Sent as SNI AND matched against the certificate's subject
    /// alternative names, so it must be a name the certificate was issued
    /// for, not necessarily the host you dial. Dialling 10.0.0.1:7379 with
    /// a certificate for db.internal means `server_name = "db.internal"`.
    pub server_name: String,
    /// The CA certificate (PEM) that issued the server's certificate. There
    /// is no way to disable verification.
    pub ca_pem: Vec<u8>,
}

impl fmt::Debug for Tls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tls")
            .field("server_name", &self.server_name)
            .field("ca_pem", &format_args!("<{} bytes>", self.ca_pem.len()))
            .finish()
    }
}

/// A client certificate (mTLS), for a server started with
/// `THUNDERSTRIKE_TLS_CLIENT_CA`.
#[derive(Clone)]
pub struct ClientIdentity {
    /// The certificate chain, leaf first, PEM.
    pub cert_pem: Vec<u8>,
    /// The leaf's PKCS#8 Ed25519 private key, PEM.
    pub key_pem: Vec<u8>,
}

impl fmt::Debug for ClientIdentity {
    // The key is a secret: never print it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientIdentity")
            .field("cert_pem", &format_args!("<{} bytes>", self.cert_pem.len()))
            .field("key_pem", &"<redacted>")
            .finish()
    }
}

/// A stated cluster layout, checked against the one the server serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster {
    /// The host every core listens on.
    pub host: String,
    /// Core `c` listens on `base_port + c`.
    pub base_port: u16,
    /// The deployment's `--shards`.
    pub shards: u16,
    /// The deployment's `--cores`.
    pub cores: u16,
}

/// Connection settings. `None` leaves the library's default.
///
/// ```no_run
/// # use thunderstrike::Config;
/// # use std::time::Duration;
/// let cfg = Config {
///     request_timeout: Some(Duration::from_secs(10)),
///     ..Config::new("10.0.0.1:7379").tls("db.internal", std::fs::read("ca.crt").unwrap())
/// };
/// ```
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Endpoints to bootstrap from. ONE is enough: the client asks it for
    /// the routing table and connects to every port that names.
    pub endpoints: Vec<String>,
    /// State the layout and have [`Client::connect`] fail if the server's
    /// differs. Optional; replaces `endpoints`.
    pub cluster: Option<Cluster>,
    /// TLS 1.3. Required by the thunderstrike server.
    pub tls: Option<Tls>,
    /// Present a client certificate (mTLS).
    pub client_identity: Option<ClientIdentity>,
    /// Connect WITHOUT TLS. Only a local stand-in server answers that; the
    /// thunderstrike server is TLS-only. With neither this nor `tls`,
    /// [`Client::connect`] fails with [`code::CONFIG`].
    pub insecure_plaintext: bool,

    /// TCP connections per endpoint. Default 4, or 1 for 8 or more endpoints.
    pub connections_per_endpoint: Option<u32>,
    /// Pipeline depth per connection, 1..=65535. Default 1024.
    pub max_inflight: Option<u32>,
    /// Encoded-but-unwritten bytes per connection. Default 8 MiB.
    pub max_buffered_bytes: Option<u64>,
    /// Largest response frame buffered. Default: the largest the server sends.
    pub max_response_bytes: Option<u64>,
    /// Per-request deadline, covering queueing and every retry. Zero
    /// disables it. Default 60 s, above the server's own bounded waits.
    pub request_timeout: Option<Duration>,
    /// Extra deadline per MiB of a set/append value. Default 100 ms.
    pub timeout_per_mib: Option<Duration>,
    /// Connect + TLS handshake deadline; non-zero. Default 5 s.
    pub connect_timeout: Option<Duration>,
    /// TCP keepalive idle time; zero leaves the OS default. Default 15 s.
    pub tcp_keepalive: Option<Duration>,
    /// TCP_NODELAY. Default on.
    pub tcp_nodelay: Option<bool>,
    /// Retries of an idempotent request whose connection dropped. Default 1.
    pub max_retries: Option<u32>,
    /// Retries of a request answered [`code::BUSY`] (every operation but
    /// delete). Default 4; `Some(0)` returns every Busy.
    pub busy_retries: Option<u32>,
    /// The Busy backoff: first wait and ceiling. Default 2 ms and 200 ms.
    pub busy_backoff: Option<(Duration, Duration)>,
    /// No Busy retry starts once an operation has run this long. Default 5 s.
    pub busy_retry_window: Option<Duration>,
    /// Request routing. Default [`Routing::ShardByKey`].
    pub routing: Option<Routing>,
    /// Worker threads for the client's runtime; 0 is one per core.
    pub worker_threads: Option<u32>,
    /// Frame multiplexing: large values travel in chunks, so a small request
    /// never waits behind a whole large one. Default on.
    pub multiplexing: Option<bool>,
    /// Chunk size on a multiplexed connection. Default 1 MiB.
    pub mux_chunk_bytes: Option<u64>,
    /// Chunked request bodies open at once per connection. Default 1.
    pub mux_max_open_bodies: Option<u32>,
    /// The I/O thread pool's floor. Default 1.
    pub min_io_threads: Option<u32>,
    /// The I/O thread pool's ceiling. Default: the machine's cores.
    pub max_io_threads: Option<u32>,
    /// How often the I/O pool reconsiders its width.
    pub io_sample_interval: Option<Duration>,
    /// How long the I/O pool stays comfortable before giving a thread back.
    pub io_idle_timeout: Option<Duration>,
}

impl Config {
    /// A configuration with one bootstrap endpoint, e.g. `"127.0.0.1:7379"`.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self { endpoints: vec![endpoint.into()], ..Self::default() }
    }

    /// Turn on TLS 1.3: the name the server's certificate was issued for,
    /// and the CA (PEM) that issued it.
    pub fn tls(mut self, server_name: impl Into<String>, ca_pem: impl Into<Vec<u8>>) -> Self {
        self.tls = Some(Tls { server_name: server_name.into(), ca_pem: ca_pem.into() });
        self
    }

    /// Present a client certificate chain and its key (both PEM).
    pub fn client_identity(mut self, cert_pem: impl Into<Vec<u8>>, key_pem: impl Into<Vec<u8>>) -> Self {
        self.client_identity = Some(ClientIdentity { cert_pem: cert_pem.into(), key_pem: key_pem.into() });
        self
    }

    /// Connect without TLS: for a local stand-in server only.
    pub fn insecure_plaintext(mut self) -> Self {
        self.insecure_plaintext = true;
        self
    }

    /// Translate into a native config. Every field goes through the
    /// library's own setter, so validation lives in one place.
    fn build(&self) -> Result<NativeConfig> {
        // SAFETY: no preconditions; null only on allocation failure.
        let raw = NonNull::new(unsafe { sys::thunderstrike_config_new() })
            .ok_or_else(|| Error::new(code::CONFIG, "out of memory"))?;
        let cfg = NativeConfig(raw);
        let c = cfg.0.as_ptr();

        // SAFETY (every call below): `c` is a live config owned by `cfg`;
        // strings are NUL-terminated and buffers readable for their lengths,
        // and the library copies them before returning.
        for addr in &self.endpoints {
            let a = cstring(addr)?;
            check(unsafe { sys::thunderstrike_config_add_endpoint(c, a.as_ptr()) })?;
        }
        if let Some(cl) = &self.cluster {
            let h = cstring(&cl.host)?;
            check(unsafe { sys::thunderstrike_config_set_cluster(c, h.as_ptr(), cl.base_port, cl.shards, cl.cores) })?;
        }
        if let Some(tls) = &self.tls {
            let name = cstring(&tls.server_name)?;
            check(unsafe {
                sys::thunderstrike_config_set_tls(c, name.as_ptr(), tls.ca_pem.as_ptr(), tls.ca_pem.len())
            })?;
        }
        if let Some(id) = &self.client_identity {
            check(unsafe {
                sys::thunderstrike_config_set_client_identity(
                    c,
                    id.cert_pem.as_ptr(),
                    id.cert_pem.len(),
                    id.key_pem.as_ptr(),
                    id.key_pem.len(),
                )
            })?;
        }
        if self.insecure_plaintext {
            check(unsafe { sys::thunderstrike_config_set_insecure_plaintext(c, 1) })?;
        }

        type U32 = unsafe extern "C" fn(*mut sys::thunderstrike_config, u32) -> i32;
        type U64 = unsafe extern "C" fn(*mut sys::thunderstrike_config, u64) -> i32;
        let u32s: [(Option<u32>, U32); 8] = [
            (self.connections_per_endpoint, sys::thunderstrike_config_set_connections_per_endpoint),
            (self.max_inflight, sys::thunderstrike_config_set_max_inflight),
            (self.max_retries, sys::thunderstrike_config_set_max_retries),
            (self.busy_retries, sys::thunderstrike_config_set_busy_retries),
            (self.worker_threads, sys::thunderstrike_config_set_worker_threads),
            (self.mux_max_open_bodies, sys::thunderstrike_config_set_mux_max_open_bodies),
            (self.min_io_threads, sys::thunderstrike_config_set_min_io_threads),
            (self.max_io_threads, sys::thunderstrike_config_set_max_io_threads),
        ];
        for (v, set) in u32s {
            if let Some(v) = v {
                check(unsafe { set(c, v) })?;
            }
        }
        let u64s: [(Option<u64>, U64); 10] = [
            (self.max_buffered_bytes, sys::thunderstrike_config_set_max_buffered_bytes),
            (self.max_response_bytes, sys::thunderstrike_config_set_max_response_bytes),
            (self.mux_chunk_bytes, sys::thunderstrike_config_set_mux_chunk_bytes),
            (self.request_timeout.map(ms), sys::thunderstrike_config_set_request_timeout_ms),
            (self.timeout_per_mib.map(ms), sys::thunderstrike_config_set_timeout_per_mib_ms),
            (self.connect_timeout.map(ms), sys::thunderstrike_config_set_connect_timeout_ms),
            (self.tcp_keepalive.map(ms), sys::thunderstrike_config_set_tcp_keepalive_ms),
            (self.busy_retry_window.map(ms), sys::thunderstrike_config_set_busy_retry_window_ms),
            (self.io_sample_interval.map(ms), sys::thunderstrike_config_set_io_sample_interval_ms),
            (self.io_idle_timeout.map(ms), sys::thunderstrike_config_set_io_idle_timeout_ms),
        ];
        for (v, set) in u64s {
            if let Some(v) = v {
                check(unsafe { set(c, v) })?;
            }
        }
        if let Some(on) = self.tcp_nodelay {
            check(unsafe { sys::thunderstrike_config_set_tcp_nodelay(c, on as i32) })?;
        }
        if let Some(on) = self.multiplexing {
            check(unsafe { sys::thunderstrike_config_set_multiplexing(c, on as i32) })?;
        }
        if let Some((lo, hi)) = self.busy_backoff {
            check(unsafe { sys::thunderstrike_config_set_busy_backoff_ms(c, ms(lo), ms(hi)) })?;
        }
        if let Some(r) = self.routing {
            let r = match r {
                Routing::ShardByKey => sys::THUNDERSTRIKE_ROUTING_SHARD_BY_KEY,
                Routing::RoundRobin => sys::THUNDERSTRIKE_ROUTING_ROUND_ROBIN,
            };
            check(unsafe { sys::thunderstrike_config_set_routing(c, r) })?;
        }
        Ok(cfg)
    }
}

/// Milliseconds for the ABI. A non-zero duration never rounds to 0, which
/// would read as "disabled".
fn ms(d: Duration) -> u64 {
    let m = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
    if m == 0 && !d.is_zero() {
        1
    } else {
        m
    }
}

fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error::new(code::INVALID_ARG, format!("{s:?} contains a NUL byte")))
}

struct NativeConfig(NonNull<sys::thunderstrike_config>);

impl Drop for NativeConfig {
    fn drop(&mut self) {
        // SAFETY: owned, freed once.
        unsafe { sys::thunderstrike_config_free(self.0.as_ptr()) }
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Pool counters, from [`Client::stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct Stats {
    pub connections: u64,
    pub connections_ready: u64,
    /// Requests on the wire right now.
    pub inflight: u64,
    /// Requests that could be on the wire at once.
    pub capacity: u64,
    pub requests: u64,
    pub responses: u64,
    pub bytes_written: u64,
    pub bytes_read: u64,
    /// `requests / write_batches` is the batching factor.
    pub write_batches: u64,
    pub reconnects: u64,
    /// Answers whose waiter had already given up.
    pub orphan_responses: u64,
    /// I/O threads running now; how many cores the client can occupy.
    pub io_threads: u64,
    pub io_threads_min: u64,
    pub io_threads_max: u64,
    pub io_grew: u64,
    pub io_shrank: u64,
    /// Connections moved between I/O threads.
    pub io_moved: u64,
}

impl Stats {
    /// Requests per socket write: how well pipelining is batching.
    pub fn batching_factor(&self) -> f64 {
        if self.write_batches == 0 {
            0.0
        } else {
            self.requests as f64 / self.write_batches as f64
        }
    }
}

/// Frame multiplexing counters, from [`Client::mux_stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MuxStats {
    /// Connections up now that negotiated multiplexing.
    pub connections: u64,
    /// Requests sent in chunks.
    pub chunked_requests: u64,
    /// Replies received in chunks.
    pub chunked_replies: u64,
}

/// A pooled, pipelined connection to a thunderstrike deployment.
///
/// `Send + Sync`: call it from any number of threads at once. Dropping it
/// shuts the pool down; an `*_async` future still pending then resolves to
/// [`code::CLOSED`].
pub struct Client {
    raw: NonNull<sys::thunderstrike_client>,
    // Boxed so its address survives the Client moving: callbacks hold it.
    pending: Box<PendingSet>,
}

// SAFETY: the header's contract: the handle is internally synchronised and
// every function but thunderstrike_client_free may be called from any thread,
// concurrently. Free happens only in Drop, which has exclusive access.
unsafe impl Send for Client {}
unsafe impl Sync for Client {}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

impl Client {
    /// Connect every connection in the pool (and complete its TLS handshake)
    /// before returning, so a bad address or a certificate that does not
    /// verify fails here, not on first use.
    pub fn connect(config: &Config) -> Result<Client> {
        let cfg = config.build()?;
        let mut out = ptr::null_mut();
        // SAFETY: cfg is live; out is writable. The config is not consumed.
        check(unsafe { sys::thunderstrike_connect(cfg.0.as_ptr(), &mut out) })?;
        let raw = NonNull::new(out).ok_or_else(|| Error::new(code::CONNECT, "null handle"))?;
        Ok(Client { raw, pending: Box::new(PendingSet(Mutex::new(HashSet::new()))) })
    }

    fn h(&self) -> *mut sys::thunderstrike_client {
        self.raw.as_ptr()
    }

    /// Read a key. `None` when it is absent.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let mut buf = OwnedBuf(sys::thunderstrike_buf::empty());
        // SAFETY: live handle; key readable; buf writable and freed by OwnedBuf.
        match unsafe { sys::thunderstrike_get(self.h(), key.as_ptr(), key.len(), &mut buf.0) } {
            code::OK => Ok(Some(buf.to_vec())),
            code::NOT_FOUND => Ok(None),
            rc => Err(Error::last(rc)),
        }
    }

    /// Read a key into `dst` without allocating.
    ///
    /// Returns the value's true length, or `None` when it is absent. A
    /// length over `dst.len()` means it did not fit and `dst` holds nothing
    /// useful: grow `dst` to that length and call again.
    pub fn get_into(&self, key: &[u8], dst: &mut [u8]) -> Result<Option<usize>> {
        let mut len = 0usize;
        // SAFETY: live handle; key readable; dst writable for its length.
        let rc = unsafe {
            sys::thunderstrike_get_into(self.h(), key.as_ptr(), key.len(), dst.as_mut_ptr(), dst.len(), &mut len)
        };
        match rc {
            code::OK | code::BUFFER_TOO_SMALL => Ok(Some(len)),
            code::NOT_FOUND => Ok(None),
            rc => Err(Error::last(rc)),
        }
    }

    /// Write a key. The server refuses an empty value ([`code::MALFORMED`]).
    pub fn set(&self, key: &[u8], value: &[u8]) -> Result<()> {
        // SAFETY: live handle; key and value readable.
        check(unsafe { sys::thunderstrike_set(self.h(), key.as_ptr(), key.len(), value.as_ptr(), value.len()) })
    }

    /// Delete a key. `false` when it was not there.
    pub fn delete(&self, key: &[u8]) -> Result<bool> {
        // SAFETY: live handle; key readable.
        found(unsafe { sys::thunderstrike_delete(self.h(), key.as_ptr(), key.len()) })
    }

    /// Whether a key is present.
    pub fn exists(&self, key: &[u8]) -> Result<bool> {
        // SAFETY: live handle; key readable.
        found(unsafe { sys::thunderstrike_exists(self.h(), key.as_ptr(), key.len()) })
    }

    /// Append to a key's value (creating it if absent); returns the new length.
    pub fn append(&self, key: &[u8], value: &[u8]) -> Result<u64> {
        let mut total = 0u64;
        // SAFETY: live handle; key and value readable; total writable.
        check(unsafe {
            sys::thunderstrike_append(self.h(), key.as_ptr(), key.len(), value.as_ptr(), value.len(), &mut total)
        })?;
        Ok(total)
    }

    /// A round trip to the server.
    pub fn ping(&self) -> Result<()> {
        // SAFETY: live handle.
        check(unsafe { sys::thunderstrike_ping(self.h()) })
    }

    /// Read many keys in ONE round trip. `None` for each absent key; the
    /// first key that failed otherwise fails the batch.
    pub fn mget<K: AsRef<[u8]>>(&self, keys: &[K]) -> Result<Vec<Option<Vec<u8>>>> {
        let n = keys.len();
        if n == 0 {
            return Ok(Vec::new());
        }
        let (kp, kl) = split(keys);
        let mut bufs = OwnedBufs((0..n).map(|_| sys::thunderstrike_buf::empty()).collect());
        let mut codes = vec![0i32; n];
        // SAFETY: live handle; n readable key pointers/lengths; n writable
        // bufs (freed by OwnedBufs) and codes.
        let rc = unsafe {
            sys::thunderstrike_mget(self.h(), kp.as_ptr(), kl.as_ptr(), n, bufs.0.as_mut_ptr(), codes.as_mut_ptr())
        };
        check(rc)?;
        codes
            .iter()
            .zip(&bufs.0)
            .enumerate()
            .map(|(i, (&c, b))| match c {
                code::OK => Ok(Some(buf_to_vec(b))),
                code::NOT_FOUND => Ok(None),
                c => Err(Error::new(c, format!("mget: key {i}"))),
            })
            .collect()
    }

    /// Write many pairs in ONE round trip. The first pair that failed fails
    /// the batch; the others may have landed.
    pub fn mset<K: AsRef<[u8]>, V: AsRef<[u8]>>(&self, pairs: &[(K, V)]) -> Result<()> {
        let n = pairs.len();
        if n == 0 {
            return Ok(());
        }
        let kp: Vec<*const u8> = pairs.iter().map(|(k, _)| k.as_ref().as_ptr()).collect();
        let kl: Vec<usize> = pairs.iter().map(|(k, _)| k.as_ref().len()).collect();
        let vp: Vec<*const u8> = pairs.iter().map(|(_, v)| v.as_ref().as_ptr()).collect();
        let vl: Vec<usize> = pairs.iter().map(|(_, v)| v.as_ref().len()).collect();
        let mut codes = vec![0i32; n];
        // SAFETY: live handle; n readable key/value pointers and lengths
        // (borrowed from `pairs` for the call); n writable codes.
        let rc = unsafe {
            sys::thunderstrike_mset(
                self.h(),
                kp.as_ptr(),
                kl.as_ptr(),
                vp.as_ptr(),
                vl.as_ptr(),
                n,
                codes.as_mut_ptr(),
            )
        };
        batch_codes(rc, &codes, "mset")
    }

    /// Delete many keys in ONE round trip. `false` for each key that was
    /// not there.
    pub fn mdelete<K: AsRef<[u8]>>(&self, keys: &[K]) -> Result<Vec<bool>> {
        let n = keys.len();
        if n == 0 {
            return Ok(Vec::new());
        }
        let (kp, kl) = split(keys);
        let mut codes = vec![0i32; n];
        // SAFETY: live handle; n readable key pointers/lengths; n writable codes.
        let rc = unsafe { sys::thunderstrike_mdelete(self.h(), kp.as_ptr(), kl.as_ptr(), n, codes.as_mut_ptr()) };
        check(rc)?;
        codes
            .iter()
            .enumerate()
            .map(|(i, &c)| match c {
                code::OK => Ok(true),
                code::NOT_FOUND => Ok(false),
                c => Err(Error::new(c, format!("mdelete: key {i}"))),
            })
            .collect()
    }

    /// [`get`](Self::get) without blocking. The key is copied before this
    /// returns; the future borrows nothing and runs on any executor.
    pub fn get_async(&self, key: &[u8]) -> Completion<Option<Vec<u8>>> {
        self.submit(
            |h, cb, ud| {
                // SAFETY: live handle; key readable (copied by the library).
                unsafe { sys::thunderstrike_get_async(h, key.as_ptr(), key.len(), cb, ud) }
            },
            |c, v, m| match c {
                code::OK => Ok(Some(v)),
                code::NOT_FOUND => Ok(None),
                c => Err(Error::new(c, m)),
            },
        )
    }

    /// [`set`](Self::set) without blocking. Key and value are copied before
    /// this returns.
    pub fn set_async(&self, key: &[u8], value: &[u8]) -> Completion<()> {
        self.submit(
            |h, cb, ud| {
                // SAFETY: live handle; key and value readable (copied).
                unsafe { sys::thunderstrike_set_async(h, key.as_ptr(), key.len(), value.as_ptr(), value.len(), cb, ud) }
            },
            |c, _, m| if c == code::OK { Ok(()) } else { Err(Error::new(c, m)) },
        )
    }

    /// [`delete`](Self::delete) without blocking. `false` when the key was
    /// not there.
    pub fn delete_async(&self, key: &[u8]) -> Completion<bool> {
        self.submit(
            |h, cb, ud| {
                // SAFETY: live handle; key readable (copied).
                unsafe { sys::thunderstrike_delete_async(h, key.as_ptr(), key.len(), cb, ud) }
            },
            |c, _, m| match c {
                code::OK => Ok(true),
                code::NOT_FOUND => Ok(false),
                c => Err(Error::new(c, m)),
            },
        )
    }

    /// The pool's counters.
    pub fn stats(&self) -> Result<Stats> {
        let mut s = sys::thunderstrike_stats::default();
        // SAFETY: live handle; s writable.
        check(unsafe { sys::thunderstrike_get_stats(self.h(), &mut s) })?;
        Ok(Stats {
            connections: s.connections,
            connections_ready: s.connections_ready,
            inflight: s.inflight,
            capacity: s.capacity,
            requests: s.requests,
            responses: s.responses,
            bytes_written: s.bytes_written,
            bytes_read: s.bytes_read,
            write_batches: s.write_batches,
            reconnects: s.reconnects,
            orphan_responses: s.orphan_responses,
            io_threads: s.io_threads,
            io_threads_min: s.io_threads_min,
            io_threads_max: s.io_threads_max,
            io_grew: s.io_grew,
            io_shrank: s.io_shrank,
            io_moved: s.io_moved,
        })
    }

    /// Frame multiplexing counters.
    pub fn mux_stats(&self) -> Result<MuxStats> {
        let mut m = MuxStats::default();
        // SAFETY: live handle; out pointers writable.
        check(unsafe {
            sys::thunderstrike_get_mux_stats(self.h(), &mut m.connections, &mut m.chunked_requests, &mut m.chunked_replies)
        })?;
        Ok(m)
    }

    /// Which shard owns `key`, under the routing table read from the
    /// server. 0 under round-robin routing.
    pub fn shard_of(&self, key: &[u8]) -> Result<u16> {
        let mut out = 0u16;
        // SAFETY: live handle; key readable; out writable.
        check(unsafe { sys::thunderstrike_shard_of(self.h(), key.as_ptr(), key.len(), &mut out) })?;
        Ok(out)
    }

    /// The deployment's shard count. 0 under round-robin routing.
    pub fn shard_count(&self) -> Result<u16> {
        let mut out = 0u16;
        // SAFETY: live handle; out writable.
        check(unsafe { sys::thunderstrike_shard_count(self.h(), &mut out) })?;
        Ok(out)
    }

    /// Endpoints connected to: one per server core, discovered.
    pub fn endpoint_count(&self) -> Result<u32> {
        let mut out = 0u32;
        // SAFETY: live handle; out writable.
        check(unsafe { sys::thunderstrike_endpoint_count(self.h(), &mut out) })?;
        Ok(out)
    }

    /// Queue an `*_async` call whose callback completes a [`Completion`].
    fn submit<T>(
        &self,
        call: impl FnOnce(*mut sys::thunderstrike_client, sys::thunderstrike_callback, *mut c_void) -> i32,
        map: fn(i32, Vec<u8>, String) -> Result<T>,
    ) -> Completion<T> {
        let slot = Arc::new(Slot::default());
        let ticket = Box::into_raw(Box::new(Ticket { slot: slot.clone(), pending: &*self.pending }));
        // Registered BEFORE the call: the callback may run before it returns.
        lock(&self.pending.0).insert(ticket as usize);
        let rc = call(self.h(), Some(on_complete), ticket.cast());
        if rc != code::OK {
            // Refused synchronously: the callback will never run.
            let detail = Error::last(rc).message;
            lock(&self.pending.0).remove(&(ticket as usize));
            // SAFETY: still ours; the library did not take it.
            drop(unsafe { Box::from_raw(ticket) });
            slot.complete(rc, Vec::new(), detail);
        }
        Completion { slot, map }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: &mut self: no other call on the handle is in progress.
        // Free joins the library's worker threads, so no callback runs after.
        unsafe { sys::thunderstrike_client_free(self.raw.as_ptr()) };
        // Requests still queued were cancelled with the runtime: their
        // callbacks never ran. Resolve their futures rather than leak them.
        let left: Vec<usize> = lock(&self.pending.0).drain().collect();
        for t in left {
            // SAFETY: a ticket in the set was never reclaimed by a callback.
            let ticket = unsafe { Box::from_raw(t as *mut Ticket) };
            ticket.slot.complete(code::CLOSED, Vec::new(), "client dropped with the request in flight".into());
        }
    }
}

fn found(rc: i32) -> Result<bool> {
    match rc {
        code::OK => Ok(true),
        code::NOT_FOUND => Ok(false),
        rc => Err(Error::last(rc)),
    }
}

fn batch_codes(rc: i32, codes: &[i32], op: &str) -> Result<()> {
    check(rc)?;
    match codes.iter().position(|&c| c != code::OK) {
        None => Ok(()),
        Some(i) => Err(Error::new(codes[i], format!("{op}: key {i}"))),
    }
}

fn split<K: AsRef<[u8]>>(keys: &[K]) -> (Vec<*const u8>, Vec<usize>) {
    keys.iter().map(|k| (k.as_ref().as_ptr(), k.as_ref().len())).unzip()
}

fn buf_to_vec(b: &sys::thunderstrike_buf) -> Vec<u8> {
    if b.len == 0 || b.data.is_null() {
        Vec::new()
    } else {
        // SAFETY: the library's buffer: `data` readable for `len` until freed.
        unsafe { std::slice::from_raw_parts(b.data, b.len) }.to_vec()
    }
}

/// One returned buffer, freed on every path.
struct OwnedBuf(sys::thunderstrike_buf);

impl OwnedBuf {
    fn to_vec(&self) -> Vec<u8> {
        buf_to_vec(&self.0)
    }
}

impl Drop for OwnedBuf {
    fn drop(&mut self) {
        // SAFETY: safe on an empty or already-freed buffer.
        unsafe { sys::thunderstrike_buf_free(&mut self.0) }
    }
}

/// Every buffer of a batch, all freed: the library initialises each one.
struct OwnedBufs(Vec<sys::thunderstrike_buf>);

impl Drop for OwnedBufs {
    fn drop(&mut self) {
        for b in &mut self.0 {
            // SAFETY: as OwnedBuf.
            unsafe { sys::thunderstrike_buf_free(b) }
        }
    }
}

// ---------------------------------------------------------------------------
// Async: a oneshot completed by the C callback.
// ---------------------------------------------------------------------------

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Nothing here panics while holding a lock, but a callback must never
    // unwind into the library: never propagate poisoning.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Tickets handed to the library and not yet called back, so Drop can
/// reclaim the ones whose callbacks the shutdown cancelled.
struct PendingSet(Mutex<HashSet<usize>>);

/// What `user_data` points at.
struct Ticket {
    slot: Arc<Slot>,
    // Outlives every callback: Client::drop frees the handle (joining the
    // workers) before dropping the set.
    pending: *const PendingSet,
}

#[derive(Default)]
struct Slot(Mutex<SlotState>);

#[derive(Default)]
struct SlotState {
    done: Option<(i32, Vec<u8>, String)>,
    waker: Option<Waker>,
}

impl Slot {
    fn complete(&self, code: i32, value: Vec<u8>, message: String) {
        let waker = {
            let mut s = lock(&self.0);
            s.done = Some((code, value, message));
            s.waker.take()
        };
        // Woken with no lock held: the executor may poll at once.
        if let Some(w) = waker {
            w.wake();
        }
    }
}

unsafe extern "C" fn on_complete(user: *mut c_void, code: i32, data: *const u8, len: usize) {
    let ticket = user.cast::<Ticket>();
    // SAFETY: `user` is the ticket `submit` registered, live until reclaimed
    // here; its `pending` set outlives every callback.
    unsafe { lock(&(*(*ticket).pending).0).remove(&(ticket as usize)) };
    // SAFETY: removed from the set, so this is the only owner now.
    let ticket = unsafe { Box::from_raw(ticket) };
    let value = if len == 0 || data.is_null() {
        Vec::new()
    } else {
        // SAFETY: readable for `len` for the duration of this call; copied.
        unsafe { std::slice::from_raw_parts(data, len) }.to_vec()
    };
    let message = if code == sys::THUNDERSTRIKE_OK || code == sys::THUNDERSTRIKE_NOT_FOUND {
        String::new()
    } else {
        // The library recorded the cause on THIS worker thread just before calling.
        Error::last(code).message
    };
    ticket.slot.complete(code, value, message);
}

/// The result of an `*_async` call. Resolves on any executor; dropping it
/// does not cancel the request (it is already queued).
#[must_use = "a Completion does nothing unless awaited; the request is queued regardless"]
pub struct Completion<T> {
    slot: Arc<Slot>,
    map: fn(i32, Vec<u8>, String) -> Result<T>,
}

impl<T> fmt::Debug for Completion<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Completion").finish_non_exhaustive()
    }
}

impl<T> Future for Completion<T> {
    type Output = Result<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<T>> {
        let mut s = lock(&self.slot.0);
        if let Some((c, v, m)) = s.done.take() {
            return Poll::Ready((self.map)(c, v, m));
        }
        match &mut s.waker {
            Some(w) if w.will_wake(cx.waker()) => {}
            w => *w = Some(cx.waker().clone()),
        }
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ms_never_rounds_a_real_duration_to_disabled() {
        assert_eq!(ms(Duration::ZERO), 0);
        assert_eq!(ms(Duration::from_micros(10)), 1);
        assert_eq!(ms(Duration::from_millis(1500)), 1500);
        assert_eq!(ms(Duration::MAX), u64::MAX);
    }

    #[test]
    fn completion_is_send_and_static() {
        fn assert_send<T: Send + 'static>() {}
        assert_send::<Completion<Option<Vec<u8>>>>();
        assert_send::<Client>();
        fn assert_sync<T: Sync>() {}
        assert_sync::<Client>();
    }
}
