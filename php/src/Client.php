<?php

declare(strict_types=1);

namespace Thunderstrike;

use FFI;
use InvalidArgumentException;

/**
 * A pooled, pipelined client for the thunderstrike key-value database.
 *
 * ```php
 * $db = Thunderstrike\Client::connect('127.0.0.1:7379');
 *
 * $db->set('user:1', 'ada');
 * echo $db->get('user:1');        // ada
 * var_dump($db->get('nope'));     // NULL
 *
 * // One round trip, not 1000.
 * $values = $db->getMany($keys);
 *
 * $db->close();
 * ```
 *
 * ## Batch, don't loop
 *
 * Every call crosses the FFI boundary, which costs more in PHP than a
 * plain function call. {@see getMany()}, {@see setMany()} and
 * {@see deleteMany()} pay that once for the whole batch *and* put
 * every request on the wire before awaiting any answer — so n keys
 * cost one round trip instead of n. Reach for them whenever you have
 * more than a couple of keys.
 *
 * ## Threading
 *
 * Calls block until the answer arrives, which is the right shape for
 * PHP's request-per-process model. The C ABI's asynchronous entry
 * points are deliberately not exposed: their callbacks fire on a
 * foreign thread, and the PHP engine cannot be entered from one.
 *
 * ## Lifetime
 *
 * One client per process, reused across the whole request, is the
 * intended shape — connecting opens a TCP pool and is far more
 * expensive than any single operation. Under FPM that means one per
 * worker process.
 */
final class Client
{
    private ?FFI\CData $handle;

    private readonly FFI $ffi;

    private function __construct(FFI $ffi, FFI\CData $handle)
    {
        $this->ffi = $ffi;
        $this->handle = $handle;
    }

    /**
     * Connect.
     *
     * Every connection in the pool is established (and authenticated)
     * before this returns, so a wrong address or a bad token throws
     * here rather than on first use.
     *
     * @param string|Options $target an endpoint like `"127.0.0.1:7379"`,
     *                               or an {@see Options} for anything else
     */
    public static function connect(string|Options $target): self
    {
        $options = \is_string($target) ? (new Options())->endpoint($target) : $target;
        $ffi = Lib::get();

        $cfg = $ffi->thunderstrike_config_new();
        if ($cfg === null) {
            throw new ThunderstrikeException(Code::RUNTIME, 'thunderstrike_config_new');
        }

        $keep = [];
        try {
            $options->applyTo($ffi, $cfg, $keep);

            $out = $ffi->new('void*');
            ThunderstrikeException::check($ffi->thunderstrike_connect($cfg, FFI::addr($out)), 'connect');

            return new self($ffi, $out);
        } finally {
            $ffi->thunderstrike_config_free($cfg);
            foreach ($keep as $buf) {
                Lib::freeBytes($buf);
            }
        }
    }

    /** Shut the pool down. Idempotent. */
    public function close(): void
    {
        if ($this->handle !== null) {
            $handle = $this->handle;
            $this->handle = null;
            $this->ffi->thunderstrike_client_free($handle);
        }
    }

    public function __destruct()
    {
        $this->close();
    }

    private function handle(): FFI\CData
    {
        if ($this->handle === null) {
            throw new ClosedException(Code::CLOSED);
        }

        return $this->handle;
    }

    // ── Single keys ────────────────────────────────────────────────

    /**
     * Read a key. `null` is a clean miss.
     *
     * Keys and values are binary-safe: PHP strings are byte strings,
     * and nothing here assumes text.
     */
    public function get(string $key): ?string
    {
        $this->checkKey($key);
        $buf = $this->ffi->new('thunderstrike_buf');
        try {
            $code = $this->ffi->thunderstrike_get($this->handle(), $key, \strlen($key), FFI::addr($buf));

            return match ($code) {
                Code::OK        => $this->readBuf($buf),
                Code::NOT_FOUND => null,
                default         => throw ThunderstrikeException::of($code, 'get'),
            };
        } finally {
            // Always: the buffer holds a reference into the client's
            // read buffer, and it is initialised even on a miss.
            $this->ffi->thunderstrike_buf_free(FFI::addr($buf));
        }
    }

    /** Write a key. */
    public function set(string $key, string $value): void
    {
        $this->checkKey($key);
        ThunderstrikeException::check(
            $this->ffi->thunderstrike_set(
                $this->handle(),
                $key,
                \strlen($key),
                $value,
                \strlen($value),
            ),
            'set',
        );
    }

    /** Delete a key. `false` means it was not there. */
    public function delete(string $key): bool
    {
        $this->checkKey($key);
        $code = $this->ffi->thunderstrike_delete($this->handle(), $key, \strlen($key));

        return match ($code) {
            Code::OK        => true,
            Code::NOT_FOUND => false,
            default         => throw ThunderstrikeException::of($code, 'delete'),
        };
    }

    /** Test for a key without transferring its value. */
    public function exists(string $key): bool
    {
        $this->checkKey($key);
        $code = $this->ffi->thunderstrike_exists($this->handle(), $key, \strlen($key));

        return match ($code) {
            Code::OK        => true,
            Code::NOT_FOUND => false,
            default         => throw ThunderstrikeException::of($code, 'exists'),
        };
    }

    /**
     * Append to a key's value, creating it when absent. Returns the
     * value's total length afterwards.
     */
    public function append(string $key, string $value): int
    {
        $this->checkKey($key);
        $total = $this->ffi->new('uint64_t');
        ThunderstrikeException::check(
            $this->ffi->thunderstrike_append(
                $this->handle(),
                $key,
                \strlen($key),
                $value,
                \strlen($value),
                FFI::addr($total),
            ),
            'append',
        );

        return (int) $total->cdata;
    }

    /** Round-trip a liveness probe. */
    public function ping(): void
    {
        ThunderstrikeException::check($this->ffi->thunderstrike_ping($this->handle()), 'ping');
    }

    // ── Batches ────────────────────────────────────────────────────

    /**
     * Read many keys in ONE round trip.
     *
     * @param  list<string>        $keys
     * @return list<string|null>   in the order of `$keys`; `null` is a miss
     */
    public function getMany(array $keys): array
    {
        $n = \count($keys);
        if ($n === 0) {
            return [];
        }

        [$ptrs, $lens, $keep] = $this->packBatch($keys, 'getMany');
        $bufs = $this->ffi->new("thunderstrike_buf[$n]");
        $codes = $this->ffi->new("int32_t[$n]");

        try {
            ThunderstrikeException::check(
                $this->ffi->thunderstrike_mget($this->handle(), $ptrs, $lens, $n, $bufs, $codes),
                'getMany',
            );

            $out = [];
            for ($i = 0; $i < $n; $i++) {
                $out[] = match ($codes[$i]) {
                    Code::OK        => $this->readBuf($bufs[$i]),
                    Code::NOT_FOUND => null,
                    default         => throw ThunderstrikeException::of($codes[$i], "getMany[$i]"),
                };
            }

            return $out;
        } finally {
            for ($i = 0; $i < $n; $i++) {
                $this->ffi->thunderstrike_buf_free(FFI::addr($bufs[$i]));
            }
            $this->freeAll($keep);
        }
    }

    /**
     * Write many pairs in ONE round trip.
     *
     * @param array<string,string> $items key => value
     */
    public function setMany(array $items): void
    {
        $n = \count($items);
        if ($n === 0) {
            return;
        }

        $keys = \array_map('strval', \array_keys($items));
        $values = \array_values($items);

        [$kPtrs, $kLens, $keepK] = $this->packBatch($keys, 'setMany');
        [$vPtrs, $vLens, $keepV] = $this->packBatch($values, 'setMany', checkKeys: false);
        $codes = $this->ffi->new("int32_t[$n]");

        try {
            ThunderstrikeException::check(
                $this->ffi->thunderstrike_mset(
                    $this->handle(),
                    $kPtrs,
                    $kLens,
                    $vPtrs,
                    $vLens,
                    $n,
                    $codes,
                ),
                'setMany',
            );
            for ($i = 0; $i < $n; $i++) {
                if ($codes[$i] !== Code::OK) {
                    throw ThunderstrikeException::of($codes[$i], "setMany[$i]");
                }
            }
        } finally {
            $this->freeAll($keepK);
            $this->freeAll($keepV);
        }
    }

    /**
     * Delete many keys in ONE round trip.
     *
     * @param  list<string> $keys
     * @return list<bool>   whether each key was there
     */
    public function deleteMany(array $keys): array
    {
        $n = \count($keys);
        if ($n === 0) {
            return [];
        }

        [$ptrs, $lens, $keep] = $this->packBatch($keys, 'deleteMany');
        $codes = $this->ffi->new("int32_t[$n]");

        try {
            ThunderstrikeException::check(
                $this->ffi->thunderstrike_mdelete($this->handle(), $ptrs, $lens, $n, $codes),
                'deleteMany',
            );

            $out = [];
            for ($i = 0; $i < $n; $i++) {
                $out[] = match ($codes[$i]) {
                    Code::OK        => true,
                    Code::NOT_FOUND => false,
                    default         => throw ThunderstrikeException::of($codes[$i], "deleteMany[$i]"),
                };
            }

            return $out;
        } finally {
            $this->freeAll($keep);
        }
    }

    // ── Introspection ──────────────────────────────────────────────

    /** The pool's counters. */
    public function stats(): Stats
    {
        $raw = $this->ffi->new('thunderstrike_stats');
        ThunderstrikeException::check(
            $this->ffi->thunderstrike_get_stats($this->handle(), FFI::addr($raw)),
            'stats',
        );

        return Stats::from($raw);
    }

    /**
     * Frame multiplexing counters: `connections` multiplexed and up now,
     * `chunkedRequests` sent in chunks, `chunkedReplies` received in chunks.
     *
     * @return array{connections: int, chunkedRequests: int, chunkedReplies: int}
     */
    public function muxStats(): array
    {
        $c = $this->ffi->new('uint64_t');
        $q = $this->ffi->new('uint64_t');
        $p = $this->ffi->new('uint64_t');
        ThunderstrikeException::check(
            $this->ffi->thunderstrike_get_mux_stats($this->handle(), FFI::addr($c), FFI::addr($q), FFI::addr($p)),
            'muxStats',
        );

        return ['connections' => $c->cdata, 'chunkedRequests' => $q->cdata, 'chunkedReplies' => $p->cdata];
    }

    /** Which shard this client routes `$key` to, under the discovered table. */
    public function shardOf(string $key): int
    {
        $this->checkKey($key);
        $out = $this->ffi->new('uint16_t');
        ThunderstrikeException::check(
            $this->ffi->thunderstrike_shard_of($this->handle(), $key, \strlen($key), FFI::addr($out)),
            'shardOf',
        );

        return (int) $out->cdata;
    }

    /**
     * Which of `$shards` shards owns `$key`: `CRC32C(key) % shards`,
     * the same function the server places data with. Needs no client.
     */
    public static function shardFor(string $key, int $shards): int
    {
        $ffi = Lib::get();
        $out = $ffi->new('uint16_t');
        ThunderstrikeException::check(
            $ffi->thunderstrike_hash_shard($key, \strlen($key), $shards, FFI::addr($out)),
            'shardFor',
        );

        return (int) $out->cdata;
    }

    /**
     * How many shards the deployment has, as the server stated it.
     * 0 under round-robin routing, which has no shards.
     */
    public function shards(): int
    {
        $out = $this->ffi->new('uint16_t');
        ThunderstrikeException::check(
            $this->ffi->thunderstrike_shard_count($this->handle(), FFI::addr($out)),
            'shards',
        );

        return (int) $out->cdata;
    }

    /**
     * How many endpoints this client holds - one per server core,
     * discovered rather than configured.
     */
    public function endpoints(): int
    {
        $out = $this->ffi->new('uint32_t');
        ThunderstrikeException::check(
            $this->ffi->thunderstrike_endpoint_count($this->handle(), FFI::addr($out)),
            'endpoints',
        );

        return (int) $out->cdata;
    }

    /** The native library's version. */
    public static function version(): string
    {
        return Lib::str(Lib::get()->thunderstrike_version());
    }

    // ── Plumbing ───────────────────────────────────────────────────

    /**
     * Build the parallel pointer/length arrays the batch ABI takes.
     *
     * @param  list<string> $items
     * @return array{0: FFI\CData, 1: FFI\CData, 2: list<FFI\CData|null>}
     */
    private function packBatch(array $items, string $op, bool $checkKeys = true): array
    {
        $n = \count($items);
        $ptrs = $this->ffi->new("uint8_t*[$n]");
        $lens = $this->ffi->new("size_t[$n]");
        // The single-key paths hand PHP strings straight to
        // `const char *` parameters, with no copy. A batch cannot:
        // the ABI wants an array of pointers, and PHP offers no way
        // to take the address of a string's buffer. So these copies
        // stay — amortised over the whole batch — and are handed back
        // for the caller's finally block to release.
        $keep = [];

        $i = 0;
        foreach ($items as $item) {
            if (!\is_string($item)) {
                $this->freeAll($keep);
                throw new InvalidArgumentException("thunderstrike: {$op}: item {$i} is not a string");
            }
            if ($checkKeys && \strlen($item) > Code::MAX_KEY_LENGTH) {
                $this->freeAll($keep);
                throw new ThunderstrikeException(Code::KEY_TOO_LONG, "{$op}[{$i}]");
            }
            $buf = Lib::bytes($item);
            $keep[] = $buf;
            $ptrs[$i] = $buf === null ? null : $this->ffi->cast('uint8_t*', $buf);
            $lens[$i] = \strlen($item);
            $i++;
        }

        return [$ptrs, $lens, $keep];
    }

    /** @param list<FFI\CData|null> $keep */
    private function freeAll(array $keep): void
    {
        foreach ($keep as $buf) {
            Lib::freeBytes($buf);
        }
    }

    private function readBuf(FFI\CData $buf): string
    {
        $len = (int) $buf->len;
        if ($len === 0) {
            return '';
        }

        // FFI::string copies into a PHP string, which is what we want:
        // the native buffer is released immediately afterwards.
        return FFI::string($buf->data, $len);
    }

    private function checkKey(string $key): void
    {
        if (\strlen($key) > Code::MAX_KEY_LENGTH) {
            throw new ThunderstrikeException(
                Code::KEY_TOO_LONG,
                'key is ' . \strlen($key) . ' bytes; the wire key_len field is 16 bits',
            );
        }
    }
}
