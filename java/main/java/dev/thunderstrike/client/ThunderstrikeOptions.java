package dev.thunderstrike.client;

import java.time.Duration;
import java.util.ArrayList;
import java.util.List;

/**
 * Connection and pool settings.
 *
 * <p>Every setter is optional — an unset value keeps the client's own
 * default — so an options object only names what it wants changed.
 *
 * <pre>{@code
 * var opts = new ThunderstrikeOptions()
 *     .endpoint("127.0.0.1:7379")
 *     .connectionsPerEndpoint(4)
 *     .tls("db.internal", caPem);
 * }</pre>
 */
public final class ThunderstrikeOptions {

    final List<String> endpoints = new ArrayList<>();
    String host;
    int basePort;
    int shards;
    int cores;
    String tlsServerName;
    byte[] tlsCaPem;
    byte[] tlsClientCertPem;
    byte[] tlsClientKeyPem;
    boolean insecurePlaintext;
    Integer busyRetries;
    Duration busyBackoffMin;
    Duration busyBackoffMax;
    Duration busyRetryWindow;
    Duration timeoutPerMib;
    Integer connectionsPerEndpoint;
    Integer maxInflight;
    Long maxBufferedBytes;
    Long maxResponseBytes;
    Duration requestTimeout;
    Duration connectTimeout;
    Duration tcpKeepalive;
    Boolean tcpNoDelay;
    Integer maxRetries;
    ThunderstrikeRouting routing;
    Integer workerThreads;
    Boolean multiplexing;
    Long muxChunkBytes;

    /**
     * Add an endpoint, e.g. {@code "127.0.0.1:7379"}. With
     * shard-by-key routing the order <em>is</em> the shard numbering:
     * the first is shard 0.
     */
    public ThunderstrikeOptions endpoint(String addr) {
        if (addr == null || addr.isBlank()) {
            throw new IllegalArgumentException("endpoint must not be empty");
        }
        endpoints.add(addr);
        return this;
    }

    /**
     * <em>State</em> the layout of a thunderstrike started with
     * {@code --shards n --cores m}: core {@code c} listens on
     * {@code basePort + c} and owns a contiguous run of shards.
     *
     * <p>Optional, and usually unnecessary - one endpoint plus
     * discovery finds the same thing. What this adds is a check: the
     * stated layout is compared against the one the server serves,
     * and connecting fails if they differ.
     */
    public ThunderstrikeOptions cluster(String host, int basePort, int shards, int cores) {
        if (host == null || host.isBlank()) {
            throw new IllegalArgumentException("host must not be empty");
        }
        if (basePort <= 0 || basePort > 65535) {
            throw new IllegalArgumentException("basePort out of range: " + basePort);
        }
        if (shards <= 0 || shards > 65535) {
            throw new IllegalArgumentException("shards out of range: " + shards);
        }
        if (cores <= 0 || cores > shards) {
            throw new IllegalArgumentException(
                    cores + " cores cannot each own one of " + shards + " shards");
        }
        if (basePort + cores - 1 > 65535) {
            throw new IllegalArgumentException("the port range runs past 65535");
        }
        this.host = host;
        this.basePort = basePort;
        this.shards = shards;
        this.cores = cores;
        return this;
    }

    /**
     * Speak TLS 1.3, verifying the server against {@code caPem}.
     *
     * <p>{@code serverName} is sent as SNI <em>and</em> matched against
     * the certificate's subject alternative names, so it must be a name
     * the certificate was issued for — not necessarily the host in the
     * endpoint. {@code caPem} is PEM text: the CA certificate ({@code ca.crt})
     * that issued the server's. There is no way to disable
     * verification.
     */
    public ThunderstrikeOptions tls(String serverName, byte[] caPem) {
        this.tlsServerName = serverName;
        this.tlsCaPem = caPem;
        return this;
    }

    /**
     * mTLS: present this client certificate chain (leaf first) and its
     * PKCS#8 Ed25519 key, both PEM, to a server started with {@code
     * THUNDERSTRIKE_TLS_CLIENT_CA}.
     */
    public ThunderstrikeOptions clientIdentity(byte[] certPem, byte[] keyPem) {
        this.tlsClientCertPem = certPem;
        this.tlsClientKeyPem = keyPem;
        return this;
    }

    /**
     * Connect WITHOUT TLS. Only the in-repo mock server answers that;
     * the thunderstrike server is TLS-only, and connecting with neither TLS
     * nor this is refused.
     */
    public ThunderstrikeOptions insecurePlaintext() {
        this.insecurePlaintext = true;
        return this;
    }

    /**
     * Retries for a request the server answered Busy (not applied),
     * every operation but delete. Default 4; 0 surfaces every Busy.
     */
    public ThunderstrikeOptions busyRetries(int n) {
        this.busyRetries = n;
        return this;
    }

    /** The Busy backoff: first wait and ceiling. Defaults 2 ms / 200 ms. */
    public ThunderstrikeOptions busyBackoff(Duration min, Duration max) {
        this.busyBackoffMin = min;
        this.busyBackoffMax = max;
        return this;
    }

    /** No Busy retry starts once the operation has run this long. Default 5 s. */
    public ThunderstrikeOptions busyRetryWindow(Duration d) {
        this.busyRetryWindow = d;
        return this;
    }

    /** Extra deadline per MiB of a set/append value. Default 100 ms. */
    public ThunderstrikeOptions timeoutPerMib(Duration d) {
        this.timeoutPerMib = d;
        return this;
    }

    /** TCP connections per endpoint. */
    public ThunderstrikeOptions connectionsPerEndpoint(int n) {
        this.connectionsPerEndpoint = n;
        return this;
    }

    /** Pipeline depth per connection, 1..65535. */
    public ThunderstrikeOptions maxInflight(int n) {
        this.maxInflight = n;
        return this;
    }

    /** Encoded-but-unwritten bytes allowed per connection. */
    public ThunderstrikeOptions maxBufferedBytes(long n) {
        this.maxBufferedBytes = n;
        return this;
    }

    /** Largest response frame the client will buffer. */
    public ThunderstrikeOptions maxResponseBytes(long n) {
        this.maxResponseBytes = n;
        return this;
    }

    /**
     * Per-request deadline, covering every retry. {@link Duration#ZERO}
     * disables it. Default 60 s — longer than the server's own bounded
     * waits, so its genuine Busy/Oom arrives before the client gives up.
     */
    public ThunderstrikeOptions requestTimeout(Duration d) {
        this.requestTimeout = d;
        return this;
    }

    /** Connect + TLS handshake deadline. */
    public ThunderstrikeOptions connectTimeout(Duration d) {
        this.connectTimeout = d;
        return this;
    }

    /** TCP keepalive idle period. {@link Duration#ZERO} leaves the OS default. */
    public ThunderstrikeOptions tcpKeepalive(Duration d) {
        this.tcpKeepalive = d;
        return this;
    }

    /** TCP_NODELAY. On by default. */
    public ThunderstrikeOptions tcpNoDelay(boolean on) {
        this.tcpNoDelay = on;
        return this;
    }

    /**
     * Retries for an idempotent request whose connection dropped.
     * Get, set, exists and ping retry; delete and append never do, and
     * a timeout never does.
     */
    public ThunderstrikeOptions maxRetries(int n) {
        this.maxRetries = n;
        return this;
    }

    /** How a key maps to an endpoint. */
    public ThunderstrikeOptions routing(ThunderstrikeRouting r) {
        this.routing = r;
        return this;
    }

    /** Worker threads for the client's own runtime. */
    public ThunderstrikeOptions workerThreads(int n) {
        this.workerThreads = n;
        return this;
    }

    /**
     * Frame multiplexing, on by default: a value larger than
     * {@link #muxChunkBytes(long)} travels in chunks with other requests
     * and replies between them, so a small operation never waits behind
     * a whole large one on its connection. {@code false} forces the
     * legacy framing (no HELLO is sent). A server that does not grant it
     * gets the legacy framing either way.
     */
    public ThunderstrikeOptions multiplexing(boolean on) {
        this.multiplexing = on;
        return this;
    }

    /**
     * Chunk size for large values on a multiplexed connection. Default
     * 1 MiB; held to the server's grant. Must be positive.
     */
    public ThunderstrikeOptions muxChunkBytes(long n) {
        if (n <= 0) {
            throw new IllegalArgumentException("muxChunkBytes must be positive");
        }
        this.muxChunkBytes = n;
        return this;
    }
}
