# thunderstrike-go

Go client for the thunderstrike key-value database, version 0.1.0.

```sh
go get github.com/TheManhattanProject/thunderstrike-client/go
```

```go
import "github.com/TheManhattanProject/thunderstrike-client/go/thunderstrike"

// The thunderstrike server speaks TLS 1.3 only: give it the CA that issued
// its certificate and the name the certificate was issued for.
ca, err := os.ReadFile("server.crt")
if err != nil {
    log.Fatal(err)
}
db, err := thunderstrike.Connect(thunderstrike.Config{
    Endpoints:     []string{"127.0.0.1:7379"},
    TLSServerName: "localhost",
    TLSCAPem:      ca,
    // mTLS (server THUNDERSTRIKE_TLS_CLIENT_CA): TLSClientCertPem, TLSClientKeyPem.
})
if err != nil {
    log.Fatal(err)
}
defer db.Close()

db.Set([]byte("user:1"), []byte("ada"))
v, found, _ := db.Get([]byte("user:1"))

// One round trip, not N. Use this whenever you have more than one key.
values, _ := db.MGet(keys)
```

## Requirements

**To build:** cgo (`CGO_ENABLED=1`) and a C compiler. Linux and macOS
already have one. On **Windows** cgo needs mingw-w64 — MSVC will not
work, because cgo does not use it.

**To run: nothing.** The client is linked statically into your binary.
No cgo, no shared library, no Rust — `ldd` on the result shows only
libc, libm and libgcc_s. Ship the one file.

### Alpine / musl

A glibc archive will not link against musl, so Alpine builds need the
musl variant:

```sh
go build -tags musl ./...
```

```dockerfile
FROM golang:alpine AS build
RUN apk add --no-cache gcc musl-dev     # cgo needs a C compiler
WORKDIR /src
COPY . .
RUN CGO_ENABLED=1 go build -tags musl -o /app ./cmd/...

FROM alpine                              # nothing to install here
COPY --from=build /app /app
ENTRYPOINT ["/app"]
```

Without `-tags musl` on Alpine you get an obscure linker error, which
is why both libcs are shipped.

### Cross-compiling

`GOOS=... GOARCH=... go build` does **not** work out of the box: cgo
needs a C cross-toolchain for the target, which Go does not supply.
Build on the target platform, in a container for it, or set `CC` to a
cross-compiler. This is a cgo property, not something this module
does.

## Platforms included

- linux_amd64
- linux_arm64
- linux_amd64_musl
- linux_arm64_musl
- windows_amd64

## Errors and retrying

Every failure is a `*thunderstrike.Error` with a numeric `Code` and a
`RetryClass()`: `RetryAlways` for `CodeBusy` (the server did not
apply the request; `errors.Is(err, thunderstrike.ErrBusy)`),
`RetryIfIdempotent` for a lost connection or a timeout (the outcome is
unknown: repeat Get, Set, Exists and Ping only), `RetryNever` for
everything else — `CodeServerOOM`, `CodeStorageExhausted`,
`CodeServerError`, `CodeMalformed` (an empty value, say). The client
has already retried Busy (`BusyRetries`, every operation but Delete)
and dropped connections (`MaxRetries`, idempotent operations only)
before returning.

## Batching

`MGet`, `MSet` and `MDelete` put every request on the wire before
awaiting any answer, so n keys cost one round trip instead of n.
Prefer them to a loop over the single-key calls.

## Frame multiplexing

On by default. Every connection asks the server for it (a HELLO right
after the TLS handshake); granted, a value larger than
`MuxChunkBytes` (1 MiB) travels as chunks with other requests and
replies between them, and the server sends large replies the same way,
so a small operation never waits behind a whole 128 MiB value on its
connection. A server that does not grant it (`THUNDERSTRIKE_MUX=0`) or does
not know HELLO gets the legacy framing. `DisableMultiplexing: true`
forces the legacy framing; `MuxMaxOpenBodies` is how many chunked
request bodies a connection keeps open at once (default 1: more than one
is unsafe against the server at 36db4921 under arena pressure);
`MuxStats()` reports what was negotiated.

## Thread safety

A `Client` is safe for concurrent use by any number of goroutines.
Share one per process.
