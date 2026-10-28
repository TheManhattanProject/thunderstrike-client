<?php

declare(strict_types=1);

namespace Thunderstrike;

use FFI;
use InvalidArgumentException;

/**
 * Connection and pool settings.
 *
 * Every setter is optional — an unset value keeps the client's own
 * default — so an Options only names what it wants changed.
 *
 * ```php
 * $db = Thunderstrike\Client::connect(
 *     (new Thunderstrike\Options())
 *         ->endpoint('127.0.0.1:7379')
 *         ->connectionsPerEndpoint(4)
 *         ->requestTimeoutMs(5000)
 * );
 * ```
 */
final class Options
{
    /** @var list<string> */
    private array $endpoints = [];

    private ?string $host = null;
    private int $basePort = 0;
    private int $shards = 0;
    private int $cores = 0;
    private ?string $tlsServerName = null;
    private ?string $tlsCaPem = null;
    private ?string $tlsClientCertPem = null;
    private ?string $tlsClientKeyPem = null;
    private bool $insecurePlaintext = false;
    private ?int $busyRetries = null;
    private ?int $busyBackoffMinMs = null;
    private ?int $busyBackoffMaxMs = null;
    private ?int $busyRetryWindowMs = null;
    private ?int $timeoutPerMibMs = null;
    private ?int $connectionsPerEndpoint = null;
    private ?int $maxInflight = null;
    private ?int $maxBufferedBytes = null;
    private ?int $maxResponseBytes = null;
    private ?int $requestTimeoutMs = null;
    private ?int $connectTimeoutMs = null;
    private ?int $tcpKeepaliveMs = null;
    private ?bool $tcpNoDelay = null;
    private ?int $maxRetries = null;
    private ?int $routing = null;
    private ?int $workerThreads = null;
    private ?bool $multiplexing = null;
    private ?int $muxChunkBytes = null;

    /**
     * Add a bootstrap address, e.g. `"127.0.0.1:7379"`. ONE is enough:
     * the client asks it for the deployment's routing table and
     * connects to every port that names.
     */
    public function endpoint(string $addr): self
    {
        if ($addr === '') {
            throw new InvalidArgumentException('thunderstrike: endpoint must not be empty');
        }
        $this->endpoints[] = $addr;

        return $this;
    }

    /**
     * STATE the layout of a thunderstrike started with `--shards n --cores
     * m`: core `c` listens on `$basePort + c` and owns a contiguous
     * run of shards.
     *
     * Optional, and usually unnecessary - one endpoint plus discovery
     * finds the same thing. What this adds is a check: the stated
     * layout is compared against the one the server serves, and
     * connecting fails if they differ.
     */
    public function cluster(string $host, int $basePort, int $shards, int $cores): self
    {
        if ($host === '') {
            throw new InvalidArgumentException('thunderstrike: host must not be empty');
        }
        if ($basePort <= 0 || $basePort > 65535) {
            throw new InvalidArgumentException("thunderstrike: basePort out of range: {$basePort}");
        }
        if ($shards <= 0 || $shards > 65535) {
            throw new InvalidArgumentException("thunderstrike: shards out of range: {$shards}");
        }
        if ($cores <= 0 || $cores > $shards) {
            throw new InvalidArgumentException(
                "thunderstrike: {$cores} cores cannot each own one of {$shards} shards",
            );
        }
        if ($basePort + $cores - 1 > 65535) {
            throw new InvalidArgumentException('thunderstrike: the port range runs past 65535');
        }
        $this->host = $host;
        $this->basePort = $basePort;
        $this->shards = $shards;
        $this->cores = $cores;

        return $this;
    }

    /** Shared secret, sent as an AUTH op once per connection. */
    /**
     * Speak TLS 1.3, verifying the server against $caPem.
     *
     * $serverName is sent as SNI AND matched against the certificate's
     * subject alternative names, so it must be a name the certificate
     * was issued for - not necessarily the host in the endpoint.
     * $caPem is PEM text: the CA certificate (ca.crt) that issued the server's.
     * There is no way to disable verification.
     */
    public function tls(string $serverName, string $caPem): self
    {
        $this->tlsServerName = $serverName;
        $this->tlsCaPem = $caPem;
        return $this;
    }

    /** TCP connections per endpoint. */
    public function connectionsPerEndpoint(int $n): self
    {
        $this->connectionsPerEndpoint = $n;

        return $this;
    }

    /** Pipeline depth per connection, 1..65535. */
    public function maxInflight(int $n): self
    {
        $this->maxInflight = $n;

        return $this;
    }

    /** Encoded-but-unwritten bytes allowed per connection. */
    public function maxBufferedBytes(int $n): self
    {
        $this->maxBufferedBytes = $n;

        return $this;
    }

    /** Largest response frame the client will buffer. */
    public function maxResponseBytes(int $n): self
    {
        $this->maxResponseBytes = $n;

        return $this;
    }

    /** Per-request deadline. 0 disables it. */
    public function requestTimeoutMs(int $ms): self
    {
        $this->requestTimeoutMs = $ms;

        return $this;
    }

    /** Connect + AUTH deadline. */
    public function connectTimeoutMs(int $ms): self
    {
        $this->connectTimeoutMs = $ms;

        return $this;
    }

    /** TCP keepalive idle period. 0 leaves the OS default. */
    public function tcpKeepaliveMs(int $ms): self
    {
        $this->tcpKeepaliveMs = $ms;

        return $this;
    }

    /** TCP_NODELAY. On by default. */
    public function tcpNoDelay(bool $on): self
    {
        $this->tcpNoDelay = $on;

        return $this;
    }

    /**
     * Frame multiplexing, on by default: a value larger than the chunk
     * size travels in chunks with other requests and replies between
     * them, so a small operation never waits behind a whole large one
     * on its connection. `false` forces the legacy framing (no HELLO).
     */
    public function multiplexing(bool $on): self
    {
        $this->multiplexing = $on;

        return $this;
    }

    /** Chunk size for large values on a multiplexed connection. Default 1 MiB. */
    public function muxChunkBytes(int $n): self
    {
        if ($n <= 0) {
            throw new \InvalidArgumentException('muxChunkBytes must be positive');
        }
        $this->muxChunkBytes = $n;

        return $this;
    }

    /**
     * Retries for an idempotent request whose connection dropped.
     * Get, set, exists and ping retry; delete and append never do, and
     * a timeout never does.
     */
    public function maxRetries(int $n): self
    {
        $this->maxRetries = $n;

        return $this;
    }

    /** Code::ROUTING_SHARD_BY_KEY (default) or Code::ROUTING_ROUND_ROBIN. */
    public function routing(int $routing): self
    {
        $this->routing = $routing;

        return $this;
    }

    /**
     * mTLS: the client certificate chain (leaf first) and its PKCS#8
     * Ed25519 key, PEM, for a server started with THUNDERSTRIKE_TLS_CLIENT_CA.
     */
    public function clientIdentity(string $certPem, string $keyPem): self
    {
        $this->tlsClientCertPem = $certPem;
        $this->tlsClientKeyPem = $keyPem;

        return $this;
    }

    /**
     * Connect WITHOUT TLS. Only the in-repo mock server answers that; the
     * thunderstrike server is TLS-only, and a config with neither is refused.
     */
    public function insecurePlaintext(): self
    {
        $this->insecurePlaintext = true;

        return $this;
    }

    /** Retries for a Busy (not applied) request, all but delete. Default 4. */
    public function busyRetries(int $n): self
    {
        $this->busyRetries = $n;

        return $this;
    }

    /** The Busy backoff: first wait and ceiling. Defaults 2 / 200 ms. */
    public function busyBackoffMs(int $minMs, int $maxMs): self
    {
        $this->busyBackoffMinMs = $minMs;
        $this->busyBackoffMaxMs = $maxMs;

        return $this;
    }

    /** No Busy retry starts once the operation has run this long. Default 5000. */
    public function busyRetryWindowMs(int $ms): self
    {
        $this->busyRetryWindowMs = $ms;

        return $this;
    }

    /** Extra deadline per MiB of a set/append value. Default 100. */
    public function timeoutPerMibMs(int $ms): self
    {
        $this->timeoutPerMibMs = $ms;

        return $this;
    }

    /** Worker threads for the client's own runtime. */
    public function workerThreads(int $n): self
    {
        $this->workerThreads = $n;

        return $this;
    }

    /**
     * Push these settings into a native config.
     *
     * @param list<FFI\CData|null> $keep buffers that must outlive the call
     *
     * @internal
     */
    public function applyTo(FFI $ffi, FFI\CData $cfg, array &$keep): void
    {
        if ($this->endpoints === [] && $this->host === null) {
            throw new InvalidArgumentException(
                'thunderstrike: give at least one endpoint, or a cluster(...)',
            );
        }

        foreach ($this->endpoints as $addr) {
            ThunderstrikeException::check($ffi->thunderstrike_config_add_endpoint($cfg, $addr), 'endpoint');
        }
        if ($this->host !== null) {
            ThunderstrikeException::check(
                $ffi->thunderstrike_config_set_cluster(
                    $cfg,
                    $this->host,
                    $this->basePort,
                    $this->shards,
                    $this->cores,
                ),
                'cluster',
            );
        }
        if ($this->tlsServerName !== null || $this->tlsCaPem !== null) {
            if ($this->tlsServerName === null || $this->tlsCaPem === null) {
                throw new InvalidArgumentException(
                    'thunderstrike: tlsServerName and tlsCaPem must be set together - a '
                    . 'trust anchor with no name to check it against verifies nothing',
                );
            }
            $pem = Lib::bytes($this->tlsCaPem);
            $keep[] = $pem;
            ThunderstrikeException::check(
                $ffi->thunderstrike_config_set_tls(
                    $cfg,
                    $this->tlsServerName,
                    $pem,
                    \strlen($this->tlsCaPem),
                ),
                'tls',
            );
        }

        if ($this->tlsClientCertPem !== null) {
            $cert = Lib::bytes($this->tlsClientCertPem);
            $key = Lib::bytes((string) $this->tlsClientKeyPem);
            $keep[] = $cert;
            $keep[] = $key;
            ThunderstrikeException::check(
                $ffi->thunderstrike_config_set_client_identity(
                    $cfg,
                    $cert,
                    \strlen($this->tlsClientCertPem),
                    $key,
                    \strlen((string) $this->tlsClientKeyPem),
                ),
                'clientIdentity',
            );
        }
        if ($this->insecurePlaintext) {
            ThunderstrikeException::check($ffi->thunderstrike_config_set_insecure_plaintext($cfg, 1), 'insecurePlaintext');
        }
        if ($this->busyBackoffMinMs !== null) {
            ThunderstrikeException::check(
                $ffi->thunderstrike_config_set_busy_backoff_ms($cfg, $this->busyBackoffMinMs, (int) $this->busyBackoffMaxMs),
                'busyBackoffMs',
            );
        }

        $simple = [
            ['thunderstrike_config_set_busy_retries', $this->busyRetries, 'busyRetries'],
            ['thunderstrike_config_set_busy_retry_window_ms', $this->busyRetryWindowMs, 'busyRetryWindowMs'],
            ['thunderstrike_config_set_timeout_per_mib_ms', $this->timeoutPerMibMs, 'timeoutPerMibMs'],
            ['thunderstrike_config_set_connections_per_endpoint', $this->connectionsPerEndpoint, 'connectionsPerEndpoint'],
            ['thunderstrike_config_set_max_inflight', $this->maxInflight, 'maxInflight'],
            ['thunderstrike_config_set_max_buffered_bytes', $this->maxBufferedBytes, 'maxBufferedBytes'],
            ['thunderstrike_config_set_max_response_bytes', $this->maxResponseBytes, 'maxResponseBytes'],
            ['thunderstrike_config_set_request_timeout_ms', $this->requestTimeoutMs, 'requestTimeoutMs'],
            ['thunderstrike_config_set_connect_timeout_ms', $this->connectTimeoutMs, 'connectTimeoutMs'],
            ['thunderstrike_config_set_tcp_keepalive_ms', $this->tcpKeepaliveMs, 'tcpKeepaliveMs'],
            ['thunderstrike_config_set_max_retries', $this->maxRetries, 'maxRetries'],
            ['thunderstrike_config_set_routing', $this->routing, 'routing'],
            ['thunderstrike_config_set_worker_threads', $this->workerThreads, 'workerThreads'],
            ['thunderstrike_config_set_mux_chunk_bytes', $this->muxChunkBytes, 'muxChunkBytes'],
        ];
        foreach ($simple as [$fn, $value, $name]) {
            if ($value !== null) {
                ThunderstrikeException::check($ffi->{$fn}($cfg, $value), $name);
            }
        }
        if ($this->tcpNoDelay !== null) {
            ThunderstrikeException::check(
                $ffi->thunderstrike_config_set_tcp_nodelay($cfg, $this->tcpNoDelay ? 1 : 0),
                'tcpNoDelay',
            );
        }
        if ($this->multiplexing !== null) {
            ThunderstrikeException::check(
                $ffi->thunderstrike_config_set_multiplexing($cfg, $this->multiplexing ? 1 : 0),
                'multiplexing',
            );
        }
    }
}
