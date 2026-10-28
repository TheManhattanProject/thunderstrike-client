package dev.thunderstrike.client;

/** Status codes, mirroring {@code thunderstrike_client.h}. */
public final class ThunderstrikeCode {

    private ThunderstrikeCode() {}

    /** Success. */
    public static final int OK = 0;

    // ── Statuses the server reported (positive) ────────────────────

    /** The key is not there. */
    public static final int NOT_FOUND = 1;
    /** This shard does not own the key; rehash and retry. */
    public static final int WRONG_SHARD = 2;
    /** A compare-and-set comparison failed. */
    public static final int CAS_CONFLICT = 3;
    /** The server's memory is genuinely exhausted. Not applied. */
    public static final int SERVER_OOM = 4;
    /**
     * The connection is not authenticated. Never sent by the current
     * server: it authenticates with TLS and closes instead.
     */
    public static final int NOT_AUTHED = 5;
    /** The server does not know this opcode. */
    public static final int UNKNOWN_OP = 6;
    /** The server rejected the request as malformed. */
    public static final int MALFORMED = 7;
    /** The server speaks a different protocol version. */
    public static final int UNSUPPORTED_VERSION = 9;
    /** This one response is too large for the server to serve. */
    public static final int RESPONSE_TOO_LARGE = 11;
    /** The shard's disk is full, or a structural ceiling. Not applied. */
    public static final int STORAGE_EXHAUSTED = 12;
    /**
     * The server was momentarily out of queue and did NOT apply the
     * request: the one retriable status. The client already retried it
     * (every operation but delete).
     */
    public static final int BUSY = 13;
    /** The server failed the operation: integrity, device or invariant. */
    public static final int SERVER_ERROR = 14;
    /** A status byte this client version does not know. */
    public static final int UNKNOWN_STATUS = 64;

    // ── Conditions the client detected (negative) ──────────────────

    /** A null or otherwise unusable argument. */
    public static final int INVALID_ARG = -1;
    /** The configuration cannot work. */
    public static final int CONFIG = -2;
    /** Could not establish a connection. */
    public static final int CONNECT = -3;
    /** A socket-level failure. */
    public static final int IO = -4;
    /** The deadline elapsed. The server may still apply the request. */
    public static final int TIMEOUT = -5;
    /** The connection dropped mid-flight; the outcome is unknown. */
    public static final int DISCONNECTED = -6;
    /** The peer sent bytes this client cannot interpret. */
    public static final int PROTOCOL = -7;
    /** The key is longer than {@link #MAX_KEY_LENGTH}. */
    public static final int KEY_TOO_LONG = -8;
    /** The client has been closed. */
    public static final int CLOSED = -9;
    /** The destination buffer was too small. */
    public static final int BUFFER_TOO_SMALL = -10;
    /** A runtime failure. */
    public static final int RUNTIME = -11;
    /** A panic was caught at the native boundary. */
    public static final int PANIC = -12;
    /** The TLS handshake failed or a record did not decrypt. (Was 13.) */
    public static final int TLS = -13;
    /** The value is longer than the server takes in one request. */
    public static final int VALUE_TOO_LARGE = -14;

    // ── Retry classes ({@link ThunderstrikeException#retryClass()}) ──────

    /** The same request fails the same way, or must not be repeated. */
    public static final int RETRY_NEVER = 0;
    /** The outcome is unknown: repeat get, set, exists and ping only. */
    public static final int RETRY_IF_IDEMPOTENT = 1;
    /** Busy: the server did not apply it; repeat anything after a backoff. */
    public static final int RETRY_ALWAYS = 2;

    /** The wire's {@code key_len} is a {@code uint16}. */
    public static final int MAX_KEY_LENGTH = 65535;
}
