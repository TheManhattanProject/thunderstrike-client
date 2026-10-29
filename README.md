# thunderstrike client

Official client libraries for the [thunderstrike](https://hub.docker.com/r/sbansal793/thunderstrike)
key-value database, for Go, Rust, Python, Node.js, .NET, Java, PHP, C and C++.

Every language uses the same compiled core: a connection pool that learns
the cluster layout from one address, pipelines requests, multiplexes large
values, and speaks TLS 1.3. The language packages here are thin adapters
over it. The core ships compiled, so a package needs no compiler or build
step on your machine, except Go's usual cgo.

## Install

| Language | Install | Package |
|---|---|---|
| Go | `go get github.com/TheManhattanProject/thunderstrike-client/go@v0.1.3` | this repo, `go/` |
| Rust | `cargo add thunderstrike` | [crates.io](https://crates.io/crates/thunderstrike) |
| Python | `pip install thunderstrike` | [PyPI](https://pypi.org/project/thunderstrike/) |
| Node.js | `npm install @shortsqueeze/thunderstrike` | [npm](https://www.npmjs.com/package/@shortsqueeze/thunderstrike) |
| .NET | `dotnet add package Thunderstrike` | [NuGet](https://www.nuget.org/packages/Thunderstrike) |
| Java | `io.github.maverick-ai:thunderstrike-client:0.1.3` | [Maven Central](https://central.sonatype.com/artifact/io.github.maverick-ai/thunderstrike-client), JDK 22+ |
| PHP | `composer require themanhattanproject/thunderstrike-client` then `vendor/bin/thunderstrike-install` | [Packagist](https://packagist.org/packages/themanhattanproject/thunderstrike-client) |
| C / C++ | `thunderstrike-native-0.1.3-<platform>.tar.gz` from the [release](https://github.com/TheManhattanProject/thunderstrike-client/releases) | header + libraries |

Platforms: Linux x86-64 and ARM64 (glibc 2.17 or newer; musl too for Go,
Rust and C), Windows x86-64, and macOS on Intel x86-64 and Apple silicon.

## Connect

The server speaks TLS 1.3 only. A client needs the CA certificate that
issued the server's certificate (`docker/tls/ca.crt` in the server's
release kit) and a DNS name the certificate carries (`localhost` unless you
set `THUNDERSTRIKE_TLS_NAMES`). Give it one address: it asks the server for
the layout and connects to every core's port itself.

**Go**

```go
import "github.com/TheManhattanProject/thunderstrike-client/go/thunderstrike"

ca, _ := os.ReadFile("ca.crt")
db, err := thunderstrike.Connect(thunderstrike.Config{
    Endpoints:     []string{"127.0.0.1:7380"},
    TLSServerName: "localhost",
    TLSCAPem:      ca,
})
if err != nil { log.Fatal(err) }
defer db.Close()

db.Set([]byte("user:1"), []byte("ada"))
v, found, _ := db.Get([]byte("user:1"))
```

**Rust**

```rust
let ca = std::fs::read("ca.crt")?;
let db = thunderstrike::Client::connect(
    &thunderstrike::Config::new("127.0.0.1:7380").tls("localhost", ca),
)?;
db.set(b"user:1", b"ada")?;
let v: Option<Vec<u8>> = db.get(b"user:1")?;
```

**Python**

```python
from thunderstrike import Thunderstrike

db = Thunderstrike("127.0.0.1:7380",
                   tls_server_name="localhost",
                   tls_ca_pem=open("ca.crt").read())
db.set(b"user:1", b"ada")
print(db.get(b"user:1"))   # b"ada", or None on a miss
```

**Node.js**

```js
// An ES module (top-level await); require() works too.
import { ThunderstrikeClient } from "@shortsqueeze/thunderstrike";
import { readFileSync } from "node:fs";

const db = await ThunderstrikeClient.connect({
  endpoints: ["127.0.0.1:7380"],
  tlsServerName: "localhost",
  tlsCaPem: readFileSync("ca.crt"),
});
await db.set(Buffer.from("user:1"), Buffer.from("ada"));
const v = await db.get(Buffer.from("user:1"));   // Buffer | null
```

**.NET**

```csharp
using Thunderstrike;

using var db = new ThunderstrikeClient(new ThunderstrikeOptions {
    Endpoints = { "127.0.0.1:7380" },
    TlsServerName = "localhost",
    TlsCaPem = File.ReadAllBytes("ca.crt"),
});
db.Set("user:1"u8.ToArray(), "ada"u8.ToArray());
byte[]? v = db.Get("user:1"u8.ToArray());   // null on a miss
```

**Java** (run with `--enable-native-access=ALL-UNNAMED`)

```java
import dev.thunderstrike.client.*;

byte[] ca = Files.readAllBytes(Path.of("ca.crt"));
try (ThunderstrikeClient db = ThunderstrikeClient.connect(new ThunderstrikeOptions()
        .endpoint("127.0.0.1:7380")
        .tls("localhost", ca))) {
    db.set("user:1".getBytes(), "ada".getBytes());
    byte[] v = db.get("user:1".getBytes());   // null on a miss
}
```

**PHP** (needs `ffi.enable=true`)

```php
use Thunderstrike\{Client, Options};

$db = Client::connect(
    (new Options())
        ->endpoint('127.0.0.1:7380')
        ->tls('localhost', file_get_contents('ca.crt')),
);
$db->set('user:1', 'ada');
echo $db->get('user:1');   // "ada", or null on a miss
```

**C**: `include/thunderstrike_client.h` documents every function. Link
`libthunderstrike_client.a` (or the shared library) from the release bundle.

## Errors and retries

Every failure carries a numeric code and a retry class:

- **always**: the server was busy and did not apply the request.
- **if idempotent**: a lost connection or a timeout. The outcome is
  unknown, so repeat reads only.
- **never**: everything else.

The client has already retried busy answers and dropped connections before
it reports either.

## Versions

Every language is released together, under one version and one tag. A bug
report that names a version names the same core everywhere.

## License

Proprietary, free to use: see [LICENSE](LICENSE). You may build these
libraries into your own applications and ship them as part of those
applications. Third-party notices are in
[THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES).

Copyright (c) 2026 SHORTSQUEEZE PRIVATE LIMITED.
