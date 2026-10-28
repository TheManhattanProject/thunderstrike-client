# thunderstrike-client for PHP

PHP client for the [thunderstrike](https://hub.docker.com/r/sbansal793/thunderstrike)
key-value database: a pool of pipelined TLS 1.3 connections, with
blocking and batch operations.

It calls the compiled client library through PHP's FFI extension: no
PHP extension to compile, and nothing to rebuild when PHP is upgraded.

## Install

```sh
composer require themanhattanproject/thunderstrike-client
vendor/bin/thunderstrike-install
```

`thunderstrike-install` downloads the native library for this machine
from the matching GitHub release, checks it against the release's
`SHA256SUMS`, and puts it where the client looks. Run it once per
install (for example from your deploy step or Dockerfile).

**Requirements:**

- PHP 8.1 or newer with the **FFI** extension loaded (`extension=ffi`).
- FFI must be allowed for your SAPI. The CLI allows it by default. Under
  FPM, Apache or another web SAPI set `ffi.enable=true` in `php.ini`
  (the default, `preload`, only allows FFI in preloaded code).
- Linux x86-64 or ARM64 with glibc 2.17 or newer, or Windows x86-64.
  macOS comes in a later release.

## Quick start

The server speaks TLS 1.3 only. The client needs the CA certificate that
issued the server's certificate (`docker/tls/ca.crt` in the server's kit)
and a DNS name that certificate carries (`localhost` unless the server
was set up with other `THUNDERSTRIKE_TLS_NAMES`).

```php
<?php
require 'vendor/autoload.php';

use Thunderstrike\Client;
use Thunderstrike\Options;

$db = Client::connect(
    (new Options())
        ->endpoint('127.0.0.1:7380')
        ->tls('localhost', file_get_contents('ca.crt'))
);

$db->set('user:1', 'ada');
echo $db->get('user:1'), "\n";   // ada
var_dump($db->get('nope'));      // NULL: a miss

$db->close();
```

Give it one address. The client asks that server for the cluster layout
and connects to every core (core *c* listens on port 7380 + *c*), so
those ports must be reachable too. `connect` returns only once every
connection, TLS included, is up.

The server name is sent as SNI and checked against the certificate, so
it is the name the certificate was issued for, not necessarily the host
you dial. There is no way to turn verification off. For a server that
requires client certificates (mTLS), add `->clientIdentity($certPem, $keyPem)`.

## Operations

Keys and values are PHP strings, binary-safe. A key is at most 65,535
bytes; an empty value is refused. Keys do not expire: the server does
not support expiry, and refuses a `set` with a non-zero `$ttlMs` as
malformed.

```php
$db->set('k', 'v');
$db->get('k');                  // string, or null on a miss
$db->exists('k');               // true / false
$db->append('k', '-more');      // appends (creates if absent); returns the new length
$db->delete('k');               // false if it was not there
$db->ping();
```

### Batches

Every call crosses the FFI boundary, which costs more in PHP than a
plain function call. The batch calls pay it once for the whole batch and
put every request on the wire before waiting for any answer, so *n* keys
cost one round trip instead of *n*. Use them whenever you have more than
a couple of keys.

```php
$db->setMany(['a' => '1', 'b' => '2']);
$db->getMany(['a', 'b', 'missing']);     // ['1', '2', null]
$db->deleteMany(['a', 'b', 'missing']);  // [true, true, false]
```

### Lifetime

Connecting opens a TCP pool and costs far more than any single
operation, so keep one client per process and reuse it (under FPM: one
per worker). Calls block until the answer arrives; there is no async API
in PHP.

## Errors and retries

Every failure throws `Thunderstrike\ThunderstrikeException` (a
`RuntimeException`), or a subclass, with:

- `code()`: the numeric status, one of the `Thunderstrike\Code` constants.
- `retryClass()`: whether sending the request again is sound.
- `isBusy()`: the server answered Busy.
- `detail()`: the specific cause when there is one (e.g. the TLS reason).

| `retryClass()` | meaning |
|---|---|
| `Code::RETRY_ALWAYS` | Busy: the server did not apply it. Retry anything after a short backoff. |
| `Code::RETRY_IF_IDEMPOTENT` | A lost connection or a timeout: the outcome is unknown. Repeat `get`, `set`, `exists` and `ping` only. |
| `Code::RETRY_NEVER` | Everything else. |

The client has already retried Busy answers (every operation but
`delete`) and dropped connections (idempotent operations) before it
throws.

Subclasses: `BusyException`, `TimeoutException`, `DisconnectedException`,
`ClosedException`, `KeyNotFoundException`.

```php
use Thunderstrike\Code;
use Thunderstrike\ThunderstrikeException;

try {
    $db->set('k', 'v');
} catch (ThunderstrikeException $e) {
    if ($e->retryClass() === Code::RETRY_ALWAYS) {
        // not applied; safe to try again later
    }
    error_log($e->getMessage());
}
```

## Configuration

`Thunderstrike\Options` is a builder, all optional except an endpoint and
(against the real server) TLS:

| method | meaning |
|---|---|
| `endpoint($addr)` | A bootstrap address. One is enough. |
| `tls($serverName, $caPem)` | TLS 1.3: the certificate's name and the CA (PEM). |
| `clientIdentity($certPem, $keyPem)` | Client certificate (mTLS). |
| `requestTimeoutMs($ms)` | Per-request deadline, covering retries. Default 60000. |
| `connectTimeoutMs($ms)` | Connect and TLS handshake deadline. |
| `connectionsPerEndpoint($n)` | TCP connections per server core. |
| `maxInflight($n)` | Pipeline depth per connection. |
| `maxRetries($n)` | Retries for an idempotent request whose connection dropped. |
| `busyRetries($n)`, `busyBackoffMs($min, $max)`, `busyRetryWindowMs($ms)` | Busy retry policy. Defaults 4, 2 / 200, 5000. |
| `maxResponseBytes($n)` | Largest response the client will accept. |
| `tcpKeepaliveMs($ms)`, `tcpNoDelay($on)` | Socket options. |

`$db->stats()` returns pool counters, `$db->endpoints()` the number of
server cores connected, `$db->shards()` the number of shards.
`Client::version()` is the client's version. To load a different build
of the native library, set `THUNDERSTRIKE_CLIENT_LIB` to its path.

## Links

- Source and issues: https://github.com/TheManhattanProject/thunderstrike-client
- Server image: https://hub.docker.com/r/sbansal793/thunderstrike
- Clients for other languages: see the repository README.

## License

Proprietary, free to use: see `LICENSE`. You may build this library into
your own applications and ship it as part of them.

Copyright (c) 2026 SHORTSQUEEZE PRIVATE LIMITED.
