/** Connection and pool settings. Everything is optional but an endpoint. */
export interface ThunderstrikeOptions {
  /**
   * Bootstrap addresses. ONE is enough: the client asks it for the
   * deployment's routing table and connects to every port it names.
   */
  endpoints?: string[];
  /**
   * STATES the layout of a server started with `--shards n --cores m`:
   * core `c` listens on `basePort + c`. Optional, and checked against
   * what the server serves rather than trusted. All four together.
   */
  host?: string;
  basePort?: number;
  shards?: number;
  cores?: number;
  /** TCP connections per endpoint. */
  connectionsPerEndpoint?: number;
  /** Pipeline depth per connection, 1..65535. */
  maxInflight?: number;
  /**
   * Per-request deadline in ms, covering every retry. 0 disables it.
   * Default 60000 — longer than the server's own bounded waits, so its
   * genuine Busy/Oom arrives before the client gives up.
   */
  requestTimeoutMs?: number;
  /** Connect + TLS handshake deadline in ms. */
  connectTimeoutMs?: number;
  /**
   * TLS 1.3, which the thunderstrike server REQUIRES. `tlsServerName` is sent
   * as SNI and matched against the certificate's SANs; `tlsCaPem` is
   * the CA certificate to verify it against. Both together.
   */
  tlsServerName?: string;
  tlsCaPem?: string | Buffer;
  /**
   * mTLS, for a server started with THUNDERSTRIKE_TLS_CLIENT_CA: the client
   * certificate chain (leaf first) and its PKCS#8 Ed25519 key, PEM.
   */
  tlsClientCertPem?: string | Buffer;
  tlsClientKeyPem?: string | Buffer;
  /** Connect WITHOUT TLS. Only the in-repo mock server answers that. */
  insecurePlaintext?: boolean;
  /** Retries for a Busy (not applied) request, all but delete. Default 4. */
  busyRetries?: number;
  /** The Busy backoff: first wait and ceiling. Defaults 2 / 200. */
  busyBackoffMinMs?: number;
  busyBackoffMaxMs?: number;
  /** No Busy retry starts once the operation has run this long. Default 5000. */
  busyRetryWindowMs?: number;
  /** Extra deadline per MiB of a set/append value. Default 100. */
  timeoutPerMibMs?: number;
  /** Largest response frame the client will buffer. */
  maxResponseBytes?: number;
  /** Retries for an idempotent request whose connection dropped. */
  maxRetries?: number;
  /** `"shardByKey"` (default) or `"roundRobin"`. */
  routing?: 'shardByKey' | 'roundRobin';
  /**
   * Frame multiplexing, on by default: a value larger than `muxChunkBytes`
   * travels in chunks with other requests and replies between them, so a
   * small operation never waits behind a whole large one on its connection.
   * `false` forces the legacy framing (no HELLO is sent).
   */
  multiplexing?: boolean;
  /** Chunk size for large values on a multiplexed connection. Default 1 MiB. */
  muxChunkBytes?: number;
}

/** Pool counters. */
export interface ThunderstrikeStats {
  connections: number;
  connectionsReady: number;
  /** Requests on the wire right now. */
  inflight: number;
  /** Requests that could be on the wire at once. */
  capacity: number;
  requests: number;
  responses: number;
  bytesWritten: number;
  bytesRead: number;
  /** `write` syscalls. `requests / writeBatches` is the batching factor. */
  writeBatches: number;
  reconnects: number;
  /** Answers whose waiter had already given up. */
  orphanResponses: number;
  /** Requests re-sent because the server answered Busy. */
  busyRetries: number;
  /** Idempotent requests re-sent because their connection dropped. */
  reconnectRetries: number;
  /** Connections that negotiated frame multiplexing and are up now. */
  muxConnections: number;
  /** Requests whose value went in chunks. */
  chunkedRequests: number;
  /** Replies that came in chunks. */
  chunkedReplies: number;
}

/**
 * Every rejection from this client is an `Error` carrying these. Branch
 * on `thunderstrikeCode` (see `codes`), and on `retryClass` to decide whether
 * repeating the request is sound.
 */
export interface ThunderstrikeError extends Error {
  /** The numeric code: positive from the server, negative client-side. */
  thunderstrikeCode: number;
  /** `RETRY_NEVER`, `RETRY_IF_IDEMPOTENT` or `RETRY_ALWAYS`. */
  retryClass: number;
  /** The server answered Busy: not applied, retriable. */
  busy: boolean;
}

/** Numeric codes, mirroring thunderstrike_client.h. */
export declare const codes: {
  readonly OK: 0;
  readonly NOT_FOUND: 1;
  readonly WRONG_SHARD: 2;
  readonly CAS_CONFLICT: 3;
  readonly SERVER_OOM: 4;
  readonly NOT_AUTHED: 5;
  readonly UNKNOWN_OP: 6;
  readonly MALFORMED: 7;
  readonly UNSUPPORTED_VERSION: 9;
  readonly RESPONSE_TOO_LARGE: 11;
  readonly STORAGE_EXHAUSTED: 12;
  readonly BUSY: 13;
  readonly SERVER_ERROR: 14;
  readonly UNKNOWN_STATUS: 64;
  readonly ERR_INVALID_ARG: -1;
  readonly ERR_CONFIG: -2;
  readonly ERR_CONNECT: -3;
  readonly ERR_IO: -4;
  readonly ERR_TIMEOUT: -5;
  readonly ERR_DISCONNECTED: -6;
  readonly ERR_PROTOCOL: -7;
  readonly ERR_KEY_TOO_LONG: -8;
  readonly ERR_CLOSED: -9;
  readonly ERR_TLS: -13;
  readonly ERR_VALUE_TOO_LARGE: -14;
};

/** The same request fails the same way, or must not be repeated. */
export declare const RETRY_NEVER: 0;
/** The outcome is unknown: repeat get/set/exists/ping only. */
export declare const RETRY_IF_IDEMPOTENT: 1;
/** Busy: the server did not apply it; repeat anything after a backoff. */
export declare const RETRY_ALWAYS: 2;

/** The retry class of a numeric code. */
export declare function retryClass(code: number): number;
/** A short, stable description of a numeric code. */
export declare function describe(code: number): string;

/**
 * A pooled, pipelined thunderstrike client.
 *
 * Every operation returns a Promise — nothing blocks the event loop.
 * One instance per process is the intended shape.
 */
export declare class ThunderstrikeClient {
  /**
   * Connect. Every connection is established (TLS included) before
   * this resolves, so a bad address or a certificate that does not
   * verify rejects here rather than on first use.
   */
  static connect(options: ThunderstrikeOptions): Promise<ThunderstrikeClient>;

  /** Read a key. Resolves to `null` on a clean miss. */
  get(key: Buffer): Promise<Buffer | null>;
  /** Read a key as UTF-8 text. */
  getString(key: string): Promise<string | null>;

  set(key: Buffer, value: Buffer): Promise<void>;
  setString(key: string, value: string): Promise<void>;

  /** Resolves `false` if the key was not there. */
  delete(key: Buffer): Promise<boolean>;
  exists(key: Buffer): Promise<boolean>;
  /** Resolves to the value's total length after the append. */
  append(key: Buffer, value: Buffer): Promise<number>;
  ping(): Promise<void>;

  /**
   * Read many keys in ONE round trip. A `null` entry is a miss.
   *
   * Prefer this to `Promise.all` over `get`: every request goes on the
   * wire before any answer is awaited, and it avoids n promises and n
   * trips across the JS boundary.
   */
  getMany(keys: Buffer[]): Promise<(Buffer | null)[]>;
  setMany(keys: Buffer[], values: Buffer[]): Promise<void>;
  deleteMany(keys: Buffer[]): Promise<boolean[]>;

  stats(): ThunderstrikeStats;
  /** Which shard this client routes `key` to, under the discovered table. */
  shardOf(key: Buffer): number;
  /** Shards the deployment has, as the server stated it. */
  shards(): number;
  /** Endpoints this client holds — one per server core. */
  endpoints(): number;
  /** Shut the pool down. Idempotent. */
  close(): void;
}

/**
 * Which of `shards` shards owns `key`: `CRC32C(key) % shards`, the same
 * function the server places data with. Needs no client.
 */
export declare function shardOf(key: Buffer, shards: number): number;

/** The client's version. */
export declare function version(): string;
