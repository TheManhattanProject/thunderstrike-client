# thunderstrike/client

PHP client for the thunderstrike key-value database.

```php
$db = Thunderstrike\Client::connect('127.0.0.1:7379');

$db->set('user:1', 'ada');
echo $db->get('user:1');          // ada
var_dump($db->get('nope'));       // NULL

// One round trip, not 1000.
$values = $db->getMany($keys);

$db->close();
```

## Requirements

- PHP 8.1+ with the **FFI** extension.
  - Enabled by default in the **CLI** SAPI.
  - Under **FPM / Apache** it is not: set `ffi.enable=true`, or better,
    list this binding's files in `ffi.preload` and leave `ffi.enable`
    at `preload`.
- The native library (`libthunderstrike_client.so` / `.dll` / `.dylib`).
  Point `THUNDERSTRIKE_CLIENT_LIB` at it, or let the binding find it under
  `target/release` in a checkout.

No compiler, and nothing to rebuild when PHP is upgraded — FFI calls
the same C ABI as every other binding.

## Batch, don't loop

Every call crosses the FFI boundary, which costs more in PHP than a
plain function call. `getMany()`, `setMany()` and `deleteMany()` pay
that once for the whole batch *and* put every request on the wire
before awaiting any answer.

Measured against the stand-in server, 200 keys:

| | write syscalls |
|---|---|
| a `set()` loop | 200 |
| one `setMany()` | **5** |

## Lifetime

One client per process, reused for the whole request, is the intended
shape: connecting opens a TCP pool and costs far more than any single
operation. Under FPM that is one per worker.

## No async

The C ABI's asynchronous entry points are deliberately not exposed.
Their callbacks fire on a foreign thread, and the PHP engine cannot
be entered from one — so PHP gets the blocking and batch operations,
which is what a request-scoped runtime wants anyway.
