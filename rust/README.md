# thunderstrike

Rust client for the [thunderstrike](https://hub.docker.com/r/sbansal793/thunderstrike)
key-value database: a pool of pipelined TLS 1.3 connections, batch
operations, and runtime-agnostic futures.

This crate is the bindings to a prebuilt native client library; see
[How the native library gets here](#how-the-native-library-gets-here).

## Install

```sh
cargo add thunderstrike
```

Prebuilt targets in this release: `x86_64` / `aarch64` Linux (gnu with
glibc 2.17 or newer, and musl) and `x86_64` Windows (msvc and gnu). macOS
comes in a later release.

## Quick start

The server speaks TLS 1.3 only. The client needs the CA certificate that
issued the server's certificate (`docker/tls/ca.crt` in the server's kit)
and a DNS name that certificate carries (`localhost` unless the server
was set up with other `THUNDERSTRIKE_TLS_NAMES`).

```rust
use thunderstrike::{Client, Config};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ca = std::fs::read("ca.crt")?;
    let db = Client::connect(&Config::new("127.0.0.1:7380").tls("localhost", ca))?;

    db.set(b"user:1", b"ada")?;
    let v: Option<Vec<u8>> = db.get(b"user:1")?;
    assert_eq!(v.as_deref(), Some(&b"ada"[..]));
    Ok(())
}
```

Give it one address. The client asks that server for the cluster layout
and connects to every core (core *c* listens on port 7380 + *c*), so
those ports must be reachable too. `connect` returns only once every
connection, TLS included, is up.

The server name is sent as SNI and matched against the certificate, so
it is the name the certificate was issued for, not necessarily the host
you dial. There is no way to turn verification off. For a server that
requires client certificates (mTLS), add `.client_identity(cert_pem, key_pem)`.

## Operations

Keys and values are byte slices. A key is at most 65,535 bytes
(`thunderstrike::MAX_KEY_LEN`); an empty value is refused
(`code::MALFORMED`). Keys do not expire: the server does not support
expiry, and refuses `set_ttl` with a non-zero TTL as malformed.

```rust
db.set(b"k", b"v")?;
let v: Option<Vec<u8>> = db.get(b"k")?;      // None when absent
let there: bool = db.exists(b"k")?;
let len: u64 = db.append(b"k", b"-more")?;   // appends (creates if absent); returns the new length
let deleted: bool = db.delete(b"k")?;        // false if it was not there
db.ping()?;

// Read into your own buffer, no allocation. Some(n) with n > buf.len()
// means it did not fit: grow buf to n and call again.
let mut buf = [0u8; 4096];
let n: Option<usize> = db.get_into(b"k", &mut buf)?;
```

### Batches

For more than one key, use the batch calls. They put every request on
the wire before waiting for any answer, so *n* keys cost one round trip
instead of *n*.

```rust
db.mset(&[(&b"a"[..], &b"1"[..]), (b"b", b"2")])?;
let values = db.mget(&[&b"a"[..], b"b", b"missing"])?;   // [Some(..), Some(..), None]
let gone = db.mdelete(&[&b"a"[..], b"b", b"missing"])?;  // [true, true, false]
```

### Threading and async

`Client` is `Send + Sync`: share it by reference or in an `Arc` and call
it from any number of threads. The blocking methods park the calling
thread. In async code use `get_async`, `set_async` and `delete_async`:
their futures work on any executor (tokio, async-std, smol, or a
hand-rolled one) and borrow nothing, so they can be spawned.

```rust
let v = db.get_async(b"k").await?;
db.set_async(b"k", b"v").await?;
let deleted = db.delete_async(b"k").await?;
```

## Errors and retries

Every failure is a `thunderstrike::Error` (a miss is `None` / `false`, not
an error) with:

- `code()`: the numeric status, one of the `thunderstrike::code` constants.
- `message()`: the specific cause when there is one (e.g. the TLS reason).
- `retry_class()`: whether sending the request again is sound.
- `is_busy()`, `is_timeout()`, `is_disconnected()`, `is_tls()`.

| `RetryClass` | meaning |
|---|---|
| `Always` | Busy: the server did not apply it. Retry anything after a short backoff. |
| `IfIdempotent` | A lost connection or a timeout: the outcome is unknown. Repeat `get`, `set`, `exists` and `ping` only. |
| `Never` | Everything else. |

The client has already retried Busy answers (every operation but delete)
and dropped connections (idempotent operations) before it returns an
error.

```rust
use thunderstrike::RetryClass;

match db.set(b"k", b"v") {
    Ok(()) => {}
    Err(e) if e.retry_class() == RetryClass::Always => { /* not applied; try again later */ }
    Err(e) => return Err(e.into()),
}
```

## Configuration

`Config` has public fields, all optional beyond the endpoint and TLS:

```rust
use std::time::Duration;

let cfg = Config {
    request_timeout: Some(Duration::from_secs(10)),
    connections_per_endpoint: Some(2),
    ..Config::new("127.0.0.1:7380").tls("localhost", ca)
};
```

| field | meaning |
|---|---|
| `request_timeout` | Per-request deadline, covering retries. Default 60 s. |
| `connect_timeout` | Connect and TLS handshake deadline. |
| `connections_per_endpoint` | TCP connections per server core. |
| `max_inflight` | Pipeline depth per connection. |
| `max_retries` | Retries for an idempotent request whose connection dropped. |
| `busy_retries`, `busy_backoff`, `busy_retry_window` | Busy retry policy. Defaults 4, (2 ms, 200 ms), 5 s. |
| `max_response_bytes` | Largest response the client will accept. |
| `tcp_keepalive`, `tcp_nodelay` | Socket options. |
| `min_io_threads`, `max_io_threads` | Bounds on the client's I/O threads. |

`db.stats()` returns pool counters, `db.endpoint_count()` the number of
server cores connected, `db.shard_count()` the number of shards.

## How the native library gets here

The client is a prebuilt native library (`thunderstrike_client`); this
crate is the bindings to it. At build time `build.rs` downloads the bundle
for your target from the GitHub release matching this crate's version and
checks its SHA-256 against the hashes pinned in the crate. A mismatch fails
the build. It needs `curl` and `tar` on the build machine (both ship with
Windows 10+ and Linux).

On every target but Windows MSVC the library is linked statically: the
binary you build has nothing extra to ship.

| variable | effect |
|---|---|
| `THUNDERSTRIKE_LIB_DIR=/path` | link the library in that directory; nothing is downloaded (offline, air-gapped, vendored builds). Point it at an extracted bundle's `<rid>/` directory. |
| `THUNDERSTRIKE_DOWNLOAD_BASE=https://mirror/dir` | fetch `thunderstrike-native-<version>-<rid>.tar.gz` from this directory instead of the GitHub release. The pinned SHA-256 still applies. |
| `THUNDERSTRIKE_SKIP_VERIFY=1` | accept a bundle with no pinned hash. Not for production. |

docs.rs builds the documentation without the library (it has no network).

### Windows (MSVC): ship the DLL

There is no static library for the MSVC target. The crate links
`thunderstrike_client.dll` through its import library and copies the DLL
next to the binaries in `target/<profile>/` (and `deps/`, `examples/`), so
`cargo run` and `cargo test` work. **When you distribute your program, ship
`thunderstrike_client.dll` in the same directory as the `.exe`.** The
`x86_64-pc-windows-gnu` target links statically and needs no DLL.

## Links

- Source and issues: https://github.com/TheManhattanProject/thunderstrike-client
- Server image: https://hub.docker.com/r/sbansal793/thunderstrike
- Clients for other languages: see the repository README.

## License

Proprietary, free to use. This crate and the native library it downloads
are covered by the thunderstrike client license in [LICENSE](LICENSE); it
is not open source. You may build them into your own applications and
ship them as part of those applications.

Copyright (c) 2026 SHORTSQUEEZE PRIVATE LIMITED.
