# thunderstrike for Go

Go client for the [thunderstrike](https://hub.docker.com/r/sbansal793/thunderstrike)
key-value database: a pool of pipelined TLS 1.3 connections, safe for
any number of goroutines.

The compiled client is linked statically into your binary through cgo.
At run time there is nothing to install: ship the one file.

## Install

```sh
go get github.com/TheManhattanProject/thunderstrike-client/go@v0.1.1
```

```go
import "github.com/TheManhattanProject/thunderstrike-client/go/thunderstrike"
```

Go 1.21 or newer. **Building needs cgo** (`CGO_ENABLED=1`) and a C
compiler: Linux already has one; on Windows install mingw-w64 (cgo does
not use MSVC).

Platforms in this release: `linux/amd64` and `linux/arm64` (glibc 2.17 or
newer, or musl with `-tags musl`) and `windows/amd64`. macOS comes in a
later release.

## Quick start

The server speaks TLS 1.3 only. The client needs the CA certificate that
issued the server's certificate (`docker/tls/ca.crt` in the server's kit)
and a DNS name that certificate carries (`localhost` unless the server
was set up with other `THUNDERSTRIKE_TLS_NAMES`).

```go
package main

import (
	"fmt"
	"log"
	"os"

	"github.com/TheManhattanProject/thunderstrike-client/go/thunderstrike"
)

func main() {
	ca, err := os.ReadFile("ca.crt")
	if err != nil {
		log.Fatal(err)
	}
	db, err := thunderstrike.Connect(thunderstrike.Config{
		Endpoints:     []string{"127.0.0.1:7380"},
		TLSServerName: "localhost",
		TLSCAPem:      ca,
	})
	if err != nil {
		log.Fatal(err)
	}
	defer db.Close()

	if err := db.Set([]byte("user:1"), []byte("ada")); err != nil {
		log.Fatal(err)
	}
	v, found, err := db.Get([]byte("user:1"))
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(string(v), found) // ada true
}
```

Give it one address. The client asks that server for the cluster layout
and connects to every core (core *c* listens on port 7380 + *c*), so
those ports must be reachable too. `Connect` returns only once every
connection, TLS included, is up.

`TLSServerName` is sent as SNI and checked against the certificate, so
it is the name the certificate was issued for, not necessarily the host
you dial. There is no way to turn verification off. For a server that
requires client certificates (mTLS), set `TLSClientCertPem` and
`TLSClientKeyPem`.

## Operations

Keys and values are `[]byte`. A key is at most 65,535 bytes
(`thunderstrike.MaxKeyLen`); an empty value is refused (`CodeMalformed`). Keys do not expire: the server does not support
expiry, and refuses `SetTTL` with a non-zero TTL as malformed.

```go
err := db.Set(k, v)
v, found, err := db.Get(k)          // found is false on a miss
ok, err := db.Exists(k)
n, err := db.Append(k, []byte("-more")) // appends (creates if absent); returns the new length
deleted, err := db.Delete(k)        // false if it was not there
err = db.Ping()
```

### Batches

For more than one key, use the batch calls. They put every request on
the wire before waiting for any answer, so *n* keys cost one round trip
instead of *n*.

```go
keys := [][]byte{[]byte("a"), []byte("b"), []byte("missing")}

err := db.MSet(keys[:2], [][]byte{[]byte("1"), []byte("2")})
values, err := db.MGet(keys)     // a nil entry is a miss
deleted, err := db.MDelete(keys) // [true true false]
```

A `Client` is safe for concurrent use. Share one per process. Calls block
the calling goroutine; cgo gives each blocking call its own OS thread, so
the rest of the program keeps running.

## Errors and retries

Every failure is a `*thunderstrike.Error` with a numeric `Code`, the
operation `Op`, the specific cause `Detail` when there is one (e.g. the
TLS reason), and `RetryClass()`:

| `RetryClass()` | meaning |
|---|---|
| `thunderstrike.RetryAlways` | Busy: the server did not apply it. Retry anything after a short backoff. `errors.Is(err, thunderstrike.ErrBusy)`. |
| `thunderstrike.RetryIfIdempotent` | A lost connection or a timeout: the outcome is unknown. Repeat `Get`, `Set`, `Exists` and `Ping` only. |
| `thunderstrike.RetryNever` | Everything else. |

The client has already retried Busy answers (every operation but
`Delete`) and dropped connections (idempotent operations) before it
returns an error.

```go
var te *thunderstrike.Error
if errors.As(err, &te) {
	switch {
	case te.RetryClass() == thunderstrike.RetryAlways:
		// not applied; safe to try again later
	case te.Code == thunderstrike.CodeTLS:
		log.Printf("TLS: %s", te.Detail)
	}
}
```

## Configuration

`thunderstrike.Config` fields; a zero value keeps the client's default:

| field | meaning |
|---|---|
| `Endpoints` | Bootstrap addresses. One is enough. |
| `TLSServerName`, `TLSCAPem` | TLS 1.3: the certificate's name and the CA (PEM). Set both. |
| `TLSClientCertPem`, `TLSClientKeyPem` | Client certificate (mTLS). |
| `RequestTimeoutMS` | Per-request deadline, covering retries. Default 60000. |
| `ConnectTimeoutMS` | Connect and TLS handshake deadline. |
| `ConnectionsPerEndpoint` | TCP connections per server core. |
| `MaxInflight` | Pipeline depth per connection. |
| `MaxRetries` | Retries for an idempotent request whose connection dropped. |
| `BusyRetries`, `BusyBackoffMinMS`, `BusyBackoffMaxMS`, `BusyRetryWindowMS` | Busy retry policy. Defaults 4, 2, 200, 5000. A negative `BusyRetries` disables them. |
| `MaxResponseBytes` | Largest response the client will accept. |
| `TCPKeepaliveMS` | TCP keepalive idle period. |
| `MinIOThreads`, `MaxIOThreads` | Bounds on the client's I/O threads. |

`db.Stats()` returns pool counters, `db.Endpoints()` the number of server
cores connected, `db.Shards()` the number of shards.

## Alpine / musl

A glibc archive will not link against musl, so Alpine builds select the
musl one with a build tag:

```sh
go build -tags musl ./...
```

```dockerfile
FROM golang:alpine AS build
RUN apk add --no-cache gcc musl-dev     # cgo needs a C compiler
WORKDIR /src
COPY . .
RUN CGO_ENABLED=1 go build -tags musl -o /app .

FROM alpine                              # nothing to install here
COPY --from=build /app /app
ENTRYPOINT ["/app"]
```

## Cross-compiling

`GOOS=... GOARCH=... go build` needs a C cross-toolchain for the target,
which Go does not supply (a cgo property, not this module's). Build on the
target platform, in a container for it, or set `CC` to a cross-compiler.

## Links

- Source and issues: https://github.com/TheManhattanProject/thunderstrike-client
- Server image: https://hub.docker.com/r/sbansal793/thunderstrike
- Clients for other languages: see the repository README.

## License

Proprietary, free to use: see [LICENSE](LICENSE). You may build this
library into your own applications and ship it as part of them.

Copyright (c) 2026 SHORTSQUEEZE PRIVATE LIMITED.
