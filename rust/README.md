# thunderstrike

Rust client for the thunderstrike key-value database: a pool of pipelined
connections, TLS 1.3, batch operations, and runtime-agnostic futures.

```sh
cargo add thunderstrike
```

```rust
use thunderstrike::{Client, Config};

fn main() -> Result<(), thunderstrike::Error> {
    // The thunderstrike server speaks TLS 1.3 only: give it the CA that
    // issued its certificate and the name the certificate was issued for.
    let ca = std::fs::read("ca.crt").expect("read ca.crt");
    let db = Client::connect(
        &Config::new("127.0.0.1:7379").tls("localhost", ca),
        // mTLS (server THUNDERSTRIKE_TLS_CLIENT_CA): .client_identity(cert_pem, key_pem)
    )?;

    db.set(b"user:1", b"ada")?;
    let v: Option<Vec<u8>> = db.get(b"user:1")?;

    // One round trip, not N. Use this whenever you have more than one key.
    let values = db.mget(&[&b"user:1"[..], b"user:2"])?;
    Ok(())
}
```

One endpoint is enough: the client asks it for the deployment's routing
table and connects to every core.

`server_name` is sent as SNI and matched against the certificate, so it is
the name the certificate was issued for, not necessarily the host you dial.
There is no way to turn verification off.

## Threading and async

`Client` is `Send + Sync`; share it by reference or in an `Arc` and call it
from any number of threads. The blocking methods park the calling thread.
In async code use `get_async`, `set_async` and `delete_async`: their futures
work on any executor (tokio, async-std, smol, or a hand-rolled one) and
borrow nothing, so they can be spawned.

Errors carry a `code()`, the cause in `message()`, and `retry_class()`,
which says whether repeating the request is sound. Busy answers and dropped
connections are already retried for you before an error is returned.

## How the native library gets here

The client is a prebuilt native library (`thunderstrike_client`); this
crate is the bindings to it. At build time `build.rs` downloads the bundle
for your target from the GitHub release matching this crate's version and
checks its SHA-256 against the hashes pinned in the crate. A mismatch fails
the build. It needs `curl` and `tar` on the build machine (both ship with
Windows 10+, macOS and Linux).

Prebuilt targets: `x86_64`/`aarch64` Linux (gnu and musl), `x86_64`/`aarch64`
macOS, `x86_64` Windows (msvc and gnu). On every target but Windows MSVC the
library is linked statically: the binary you build has nothing extra to ship.

| variable | effect |
|---|---|
| `THUNDERSTRIKE_LIB_DIR=/path` | link the library in that directory; nothing is downloaded (offline, air-gapped, vendored builds). Point it at an extracted bundle's `<rid>/` directory. |
| `THUNDERSTRIKE_DOWNLOAD_BASE=https://mirror/dir` | fetch `thunderstrike-native-<version>-<rid>.tar.gz` from this directory instead of the GitHub release. The pinned SHA-256 still applies. |
| `THUNDERSTRIKE_SKIP_VERIFY=1` | accept a bundle with no pinned hash. Not for production. |

### Windows (MSVC): ship the DLL

There is no static library for the MSVC target. The crate links
`thunderstrike_client.dll` through its import library and copies the DLL
next to the binaries in `target/<profile>/` (and `deps/`, `examples/`), so
`cargo run` and `cargo test` work. **When you distribute your program, ship
`thunderstrike_client.dll` in the same directory as the `.exe`.** The
`x86_64-pc-windows-gnu` target links statically and needs no DLL.

## License

Proprietary. This crate and the native library it downloads are covered by
the thunderstrike client license in [LICENSE](LICENSE); it is not open
source.
