<?php

declare(strict_types=1);

namespace Thunderstrike;

/** Status codes, mirroring thunderstrike_client.h. */
final class Code
{
    /** Success. */
    public const OK = 0;

    // ── Statuses the server reported (positive) ────────────────────

    /** The key is not there. */
    public const NOT_FOUND = 1;
    /** This shard does not own the key; rehash and retry. */
    public const WRONG_SHARD = 2;
    /** A compare-and-set comparison failed. */
    public const CAS_CONFLICT = 3;
    /** The server's memory is genuinely exhausted. Not applied. */
    public const SERVER_OOM = 4;
    /** Not authenticated. Never sent today: the server closes instead. */
    public const NOT_AUTHED = 5;
    /** The server does not know this opcode. */
    public const UNKNOWN_OP = 6;
    /** The server rejected the request as malformed. */
    public const MALFORMED = 7;
    /** The server speaks a different protocol version. */
    public const UNSUPPORTED_VERSION = 9;
    /** This one response is too large for the server to serve. */
    public const RESPONSE_TOO_LARGE = 11;
    /** The shard's disk is full, or a structural ceiling. Not applied. */
    public const STORAGE_EXHAUSTED = 12;
    /** Not applied; the one retriable status (already retried, all but delete). */
    public const BUSY = 13;
    /** The server failed the operation: integrity, device or invariant. */
    public const SERVER_ERROR = 14;
    /** A status byte this client version does not know. */
    public const UNKNOWN_STATUS = 64;

    // ── Conditions the client detected (negative) ──────────────────

    /** A null or otherwise unusable argument. */
    public const INVALID_ARG = -1;
    /** The configuration cannot work. */
    public const CONFIG = -2;
    /** Could not establish a connection. */
    public const CONNECT = -3;
    /** A socket-level failure. */
    public const IO = -4;
    /** The deadline elapsed. The server may still apply the request. */
    public const TIMEOUT = -5;
    /** The connection dropped mid-flight; the outcome is unknown. */
    public const DISCONNECTED = -6;
    /** The peer sent bytes this client cannot interpret. */
    public const PROTOCOL = -7;
    /** The key is longer than MAX_KEY_LENGTH. */
    public const KEY_TOO_LONG = -8;
    /** The client has been closed. */
    public const CLOSED = -9;
    /** The destination buffer was too small. */
    public const BUFFER_TOO_SMALL = -10;
    /** A runtime failure. */
    public const RUNTIME = -11;
    /** A panic was caught at the native boundary. */
    public const PANIC = -12;
    /** The TLS handshake failed or a record did not decrypt. (Was 13.) */
    public const TLS = -13;
    /** The value is longer than the server takes in one request. */
    public const VALUE_TOO_LARGE = -14;

    /** retryClass(): the same request fails the same way. */
    public const RETRY_NEVER = 0;
    /** retryClass(): outcome unknown; repeat get/set/exists/ping only. */
    public const RETRY_IF_IDEMPOTENT = 1;
    /** retryClass(): Busy, not applied; repeat anything after a backoff. */
    public const RETRY_ALWAYS = 2;

    /** The wire's key_len is a uint16. */
    public const MAX_KEY_LENGTH = 65535;

    /** Hash the key to a shard. */
    public const ROUTING_SHARD_BY_KEY = 0;
    /** Treat every endpoint as interchangeable. */
    public const ROUTING_ROUND_ROBIN = 1;

    private function __construct()
    {
    }
}
