<?php

declare(strict_types=1);

namespace Thunderstrike;

use FFI;
use RuntimeException;

/**
 * Loads libthunderstrike_client and holds the FFI handle.
 *
 * Named `Lib`, not `Ffi`: PHP class names are case-insensitive, so a
 * class called `Ffi` is the same name as the built-in `FFI` and any
 * file with `use FFI;` would resolve to the wrong one.
 *
 * This calls the same C ABI as the C, Go, Python, .NET and Java
 * bindings — no PHP extension to compile, no build step, and nothing
 * to rebuild when PHP itself is upgraded.
 *
 * @internal Use {@see Client}.
 */
final class Lib
{
    /**
     * The subset of thunderstrike_client.h that PHP's FFI parser needs.
     *
     * Trimmed on purpose: FFI has no preprocessor, so the header
     * cannot be included directly. The opaque handles are declared as
     * `void *` rather than named struct types — identical at the ABI
     * level, and it avoids FFI's limits around incomplete types.
     *
     * The *_async functions are deliberately absent. Their callbacks
     * fire on a Rust runtime thread, and calling into the PHP engine
     * from a foreign thread is not safe at any PHP version. PHP gets
     * the blocking and batch entry points, which is what a
     * request-scoped runtime wants anyway.
     */
    private const DECLARATIONS = <<<'CDEF'
        typedef struct { const uint8_t *data; size_t len; void *owner; } thunderstrike_buf;

        typedef struct {
            uint64_t connections;
            uint64_t connections_ready;
            uint64_t inflight;
            uint64_t capacity;
            uint64_t requests;
            uint64_t responses;
            uint64_t bytes_written;
            uint64_t bytes_read;
            uint64_t write_batches;
            uint64_t reconnects;
            uint64_t orphan_responses;
            /* The I/O pool. Declared even if unread: the native side
               writes the whole struct, so a short cdef is an
               out-of-bounds write. */
            uint64_t io_threads;
            uint64_t io_threads_min;
            uint64_t io_threads_max;
            uint64_t io_grew;
            uint64_t io_shrank;
            uint64_t io_moved;
        } thunderstrike_stats;

        void *thunderstrike_config_new(void);
        void thunderstrike_config_free(void *cfg);
        int32_t thunderstrike_config_add_endpoint(void *cfg, const char *addr);
        int32_t thunderstrike_config_set_cluster(void *cfg, const char *host, uint16_t base_port, uint16_t shards, uint16_t cores);
        const char *thunderstrike_last_error_message(void);
        int32_t thunderstrike_config_set_tls(void *cfg, const char *server_name, const uint8_t *ca_pem, size_t ca_pem_len);
        int32_t thunderstrike_config_set_connections_per_endpoint(void *cfg, uint32_t n);
        int32_t thunderstrike_config_set_max_inflight(void *cfg, uint32_t n);
        int32_t thunderstrike_config_set_max_buffered_bytes(void *cfg, uint64_t n);
        int32_t thunderstrike_config_set_max_response_bytes(void *cfg, uint64_t n);
        int32_t thunderstrike_config_set_request_timeout_ms(void *cfg, uint64_t ms);
        int32_t thunderstrike_config_set_connect_timeout_ms(void *cfg, uint64_t ms);
        int32_t thunderstrike_config_set_tcp_keepalive_ms(void *cfg, uint64_t ms);
        int32_t thunderstrike_config_set_tcp_nodelay(void *cfg, int32_t on);
        int32_t thunderstrike_config_set_max_retries(void *cfg, uint32_t n);
        int32_t thunderstrike_config_set_routing(void *cfg, int32_t routing);
        int32_t thunderstrike_config_set_worker_threads(void *cfg, uint32_t n);
        int32_t thunderstrike_config_set_client_identity(void *cfg, const uint8_t *cert_pem, size_t cert_pem_len, const uint8_t *key_pem, size_t key_pem_len);
        int32_t thunderstrike_config_set_insecure_plaintext(void *cfg, int32_t on);
        int32_t thunderstrike_config_set_busy_retries(void *cfg, uint32_t n);
        int32_t thunderstrike_config_set_busy_backoff_ms(void *cfg, uint64_t min_ms, uint64_t max_ms);
        int32_t thunderstrike_config_set_busy_retry_window_ms(void *cfg, uint64_t ms);
        int32_t thunderstrike_config_set_timeout_per_mib_ms(void *cfg, uint64_t ms);
        int32_t thunderstrike_config_set_multiplexing(void *cfg, int32_t on);
        int32_t thunderstrike_config_set_mux_chunk_bytes(void *cfg, uint64_t n);
        int32_t thunderstrike_retry_class(int32_t code);

        int32_t thunderstrike_connect(void *cfg, void **out);
        void thunderstrike_client_free(void *client);

        int32_t thunderstrike_get(void *client, const char *key, size_t key_len, thunderstrike_buf *out);
        int32_t thunderstrike_get_into(void *client, const char *key, size_t key_len, uint8_t *dst, size_t dst_cap, size_t *out_len);
        int32_t thunderstrike_set(void *client, const char *key, size_t key_len, const char *value, size_t value_len);
        int32_t thunderstrike_delete(void *client, const char *key, size_t key_len);
        int32_t thunderstrike_exists(void *client, const char *key, size_t key_len);
        int32_t thunderstrike_append(void *client, const char *key, size_t key_len, const char *value, size_t value_len, uint64_t *out_total);
        int32_t thunderstrike_ping(void *client);

        int32_t thunderstrike_mget(void *client, const uint8_t **keys, const size_t *key_lens, size_t n, thunderstrike_buf *out_values, int32_t *out_codes);
        int32_t thunderstrike_mset(void *client, const uint8_t **keys, const size_t *key_lens, const uint8_t **values, const size_t *value_lens, size_t n, int32_t *out_codes);
        int32_t thunderstrike_mdelete(void *client, const uint8_t **keys, const size_t *key_lens, size_t n, int32_t *out_codes);

        void thunderstrike_buf_free(thunderstrike_buf *buf);
        int32_t thunderstrike_get_stats(void *client, thunderstrike_stats *out);
        int32_t thunderstrike_get_mux_stats(void *client, uint64_t *out_mux_connections, uint64_t *out_chunked_requests, uint64_t *out_chunked_replies);
        int32_t thunderstrike_shard_of(void *client, const char *key, size_t key_len, uint16_t *out_shard);
        int32_t thunderstrike_hash_shard(const char *key, size_t key_len, uint16_t shards, uint16_t *out_shard);
        int32_t thunderstrike_shard_count(void *client, uint16_t *out_shards);
        int32_t thunderstrike_endpoint_count(void *client, uint32_t *out_endpoints);
        const char *thunderstrike_strerror(int32_t code);
        const char *thunderstrike_version(void);
        CDEF;

    private static ?FFI $ffi = null;

    private function __construct()
    {
    }

    /** The loaded library, built once per process. */
    public static function get(): FFI
    {
        if (self::$ffi === null) {
            if (!\extension_loaded('FFI')) {
                throw new RuntimeException(
                    'thunderstrike: the FFI extension is not loaded. Enable it in php.ini '
                    . '(extension=ffi). It is on by default for the CLI SAPI; under '
                    . 'FPM you also need ffi.enable=true or an ffi.preload entry.'
                );
            }
            $path = self::locate();
            try {
                self::$ffi = FFI::cdef(self::DECLARATIONS, $path);
            } catch (\FFI\Exception $e) {
                throw new RuntimeException(
                    "thunderstrike: could not load the native library ({$path}): {$e->getMessage()}. "
                    . 'Run vendor/bin/thunderstrike-install to fetch it for this platform, '
                    . 'or set THUNDERSTRIKE_CLIENT_LIB to its path.',
                    0,
                    $e
                );
            }
        }

        return self::$ffi;
    }

    /**
     * Point at a specific build, before the first call.
     *
     * @throws RuntimeException if the library is already loaded
     */
    public static function useLibrary(string $path): void
    {
        if (self::$ffi !== null) {
            throw new RuntimeException('thunderstrike: the library is already loaded');
        }
        self::$ffi = FFI::cdef(self::DECLARATIONS, $path);
    }

    /**
     * Where to look, most specific first: THUNDERSTRIKE_CLIENT_LIB, then
     * what bin/thunderstrike-install put in lib/<rid>/, then a checkout's
     * build output.
     */
    private static function locate(): string
    {
        $tried = [];

        $explicit = \getenv('THUNDERSTRIKE_CLIENT_LIB');
        if (\is_string($explicit) && $explicit !== '') {
            if (\is_file($explicit)) {
                return $explicit;
            }
            $tried[] = $explicit;
        }

        $name = self::libraryFileName();

        $rid = self::rid();
        if ($rid !== null) {
            $installed = self::libDir($rid) . \DIRECTORY_SEPARATOR . $name;
            if (\is_file($installed)) {
                return $installed;
            }
            $tried[] = $installed;
        }

        // Walk up from this file: a checkout has the library under
        // target/{release,debug}.
        $dir = __DIR__;
        for ($i = 0; $i < 8; $i++) {
            foreach (['release', 'debug'] as $profile) {
                $candidate = $dir . \DIRECTORY_SEPARATOR . 'target'
                    . \DIRECTORY_SEPARATOR . $profile . \DIRECTORY_SEPARATOR . $name;
                if (\is_file($candidate)) {
                    return $candidate;
                }
                $tried[] = $candidate;
            }
            $parent = \dirname($dir);
            if ($parent === $dir) {
                break;
            }
            $dir = $parent;
        }

        // Let the platform loader search its own paths.
        return $name;
    }

    /** @internal The library's file name on this OS. */
    public static function libraryFileName(): string
    {
        return match (\PHP_OS_FAMILY) {
            'Windows' => 'thunderstrike_client.dll',
            'Darwin'  => 'libthunderstrike_client.dylib',
            default   => 'libthunderstrike_client.so',
        };
    }

    /**
     * @internal The release's runtime id for this machine (linux-x64,
     * osx-arm64, win-x64, ...), or null for one it does not build.
     * musl gets its own id: a glibc library would not load there.
     */
    public static function rid(): ?string
    {
        $arch = match (\strtolower(\php_uname('m'))) {
            'x86_64', 'amd64', 'x64' => 'x64',
            'aarch64', 'arm64' => 'arm64',
            default => null,
        };
        if ($arch === null) {
            return null;
        }

        return match (\PHP_OS_FAMILY) {
            'Windows' => $arch === 'x64' ? 'win-x64' : null,
            'Darwin' => "osx-{$arch}",
            'Linux' => "linux-{$arch}" . (\glob('/lib/ld-musl-*') ? '-musl' : ''),
            default => null,
        };
    }

    /** @internal Where bin/thunderstrike-install puts the library for $rid. */
    public static function libDir(string $rid): string
    {
        return \dirname(__DIR__) . \DIRECTORY_SEPARATOR . 'lib' . \DIRECTORY_SEPARATOR . $rid;
    }

    /**
     * Copy a PHP string into native memory.
     *
     * PHP strings are binary-safe but their buffers are not stable
     * addresses to hand to C, so every argument is copied into FFI
     * memory that lives for the duration of the call. The returned
     * CData must stay referenced until the call returns.
     *
     * @return \FFI\CData|null null for an empty string, which the ABI
     *                         reads no bytes from
     */
    public static function bytes(string $s): ?\FFI\CData
    {
        $len = \strlen($s);
        if ($len === 0) {
            return null;
        }
        // The instance form, not FFI::new(): calling new()/cast()
        // statically is deprecated from PHP 8.4.
        $buf = self::get()->new("uint8_t[$len]", false);
        FFI::memcpy($buf, $s, $len);

        return $buf;
    }

    /**
     * Read a `const char *` returned by the library.
     *
     * PHP's FFI already converts a `char *` return value into a PHP
     * string, but it hands back CData in some builds — accept both
     * rather than depend on which.
     */
    public static function str(mixed $v): string
    {
        if (\is_string($v)) {
            return $v;
        }
        if ($v instanceof \FFI\CData) {
            return FFI::string($v);
        }

        return '';
    }

    /** Free a buffer allocated by {@see bytes()}. */
    public static function freeBytes(?\FFI\CData $buf): void
    {
        if ($buf !== null) {
            FFI::free($buf);
        }
    }
}
