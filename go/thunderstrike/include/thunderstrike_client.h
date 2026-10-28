/*
 * thunderstrike_client.h — C ABI for the thunderstrike key-value database client.
 *
 * Link against:
 *   Windows : thunderstrike_client.dll  + thunderstrike_client.dll.lib
 *             (or thunderstrike_client.lib for the static build)
 *   Linux   : libthunderstrike_client.so  / libthunderstrike_client.a
 *   macOS   : libthunderstrike_client.dylib / libthunderstrike_client.a
 *
 * ---------------------------------------------------------------------------
 * Threading
 * ---------------------------------------------------------------------------
 * A thunderstrike_client is internally synchronised. Every function except
 * thunderstrike_client_free may be called from any thread, concurrently, on the
 * same handle. Call thunderstrike_client_free exactly once, after every other
 * call on that handle has returned.
 *
 * The blocking calls park the calling thread on the client's own runtime.
 * Do NOT call them from inside a callback — that thread is already a
 * runtime thread, and the call returns THUNDERSTRIKE_ERR_RUNTIME rather than
 * deadlocking.
 *
 * ---------------------------------------------------------------------------
 * Memory
 * ---------------------------------------------------------------------------
 * A thunderstrike_buf returned by this library is owned by the caller and must be
 * released with thunderstrike_buf_free. Pointers passed in are read only for the
 * duration of the call, except in the *_async family, which copies them
 * before returning.
 *
 * ---------------------------------------------------------------------------
 * Errors
 * ---------------------------------------------------------------------------
 * Every call returns an int32_t. 0 is success. Positive values are statuses
 * the server reported (the wire status byte); negative values are
 * conditions the client detected. thunderstrike_strerror turns any of them into a
 * static string, thunderstrike_last_error_message gives this thread's detail, and
 * thunderstrike_retry_class says whether repeating the request is sound:
 *
 *   THUNDERSTRIKE_RETRY_ALWAYS        THUNDERSTRIKE_BUSY — not applied; retry anything
 *   THUNDERSTRIKE_RETRY_IF_IDEMPOTENT connection lost / timeout — outcome unknown;
 *                               retry GET, SET, EXISTS, PING only
 *   THUNDERSTRIKE_RETRY_NEVER         everything else
 *
 * The client applies this itself before returning: Busy is retried with a
 * jittered backoff (busy_retries), a dropped connection is retried for the
 * idempotent operations (max_retries).
 *
 * ---------------------------------------------------------------------------
 * TLS
 * ---------------------------------------------------------------------------
 * The thunderstrike server speaks TLS 1.3 only. Configure it with
 * thunderstrike_config_set_tls before thunderstrike_connect.
 */

#ifndef THUNDERSTRIKE_CLIENT_H
#define THUNDERSTRIKE_CLIENT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ===========================================================================
 * Status codes
 * ======================================================================== */

#define THUNDERSTRIKE_OK                    0

/* ---- Statuses the SERVER reported: the wire status byte, widened. ---- */
#define THUNDERSTRIKE_NOT_FOUND             1
/* The answering core does not own the key's shard: the deployment's
 * layout changed under a running client. Reconnect to re-read it. */
#define THUNDERSTRIKE_WRONG_SHARD           2
/* Never sent by the current server (it has no CAS opcode). */
#define THUNDERSTRIKE_CAS_CONFLICT          3
/* The server's MEMORY is genuinely exhausted: its arena could not stage
 * the request after a whole admission window. Not applied. Back off for
 * seconds or shed load; an immediate retry meets the same arena. */
#define THUNDERSTRIKE_SERVER_OOM            4
/* Never sent by the current server: thunderstrike authenticates with TLS and
 * closes an unauthenticated connection instead of answering. */
#define THUNDERSTRIKE_NOT_AUTHED            5
/* The server does not implement the opcode. */
#define THUNDERSTRIKE_UNKNOWN_OP            6
/* The server will not serve the request as sent: an empty value in a
 * SET or APPEND, a value over its ceiling, an APPEND past the extent cap. */
#define THUNDERSTRIKE_MALFORMED             7
#define THUNDERSTRIKE_UNSUPPORTED_VERSION   9
#define THUNDERSTRIKE_RESPONSE_TOO_LARGE   11
/* The shard's disk is full, or it reached a structural ceiling — not
 * THUNDERSTRIKE_SERVER_OOM, which is memory. The write did not land; free or
 * grow storage first. */
#define THUNDERSTRIKE_STORAGE_EXHAUSTED    12
/* The server was momentarily out of queue and did NOT apply the request.
 * The one retriable status: the client has already retried it
 * busy_retries times (every operation but DELETE) before returning it. */
#define THUNDERSTRIKE_BUSY                 13
/* The server FAILED the operation: integrity, device or invariant
 * failure. It logged the cause. Not retriable. */
#define THUNDERSTRIKE_SERVER_ERROR         14
#define THUNDERSTRIKE_UNKNOWN_STATUS       64

/* ---- Conditions the CLIENT detected. ---- */
#define THUNDERSTRIKE_ERR_INVALID_ARG      (-1)
#define THUNDERSTRIKE_ERR_CONFIG           (-2)
#define THUNDERSTRIKE_ERR_CONNECT          (-3)
#define THUNDERSTRIKE_ERR_IO               (-4)
#define THUNDERSTRIKE_ERR_TIMEOUT          (-5)
#define THUNDERSTRIKE_ERR_DISCONNECTED     (-6)
#define THUNDERSTRIKE_ERR_PROTOCOL         (-7)
#define THUNDERSTRIKE_ERR_KEY_TOO_LONG     (-8)
#define THUNDERSTRIKE_ERR_CLOSED           (-9)
#define THUNDERSTRIKE_ERR_BUFFER_TOO_SMALL (-10)
#define THUNDERSTRIKE_ERR_RUNTIME          (-11)
#define THUNDERSTRIKE_ERR_PANIC            (-12)
/* The TLS handshake failed or a record did not decrypt. Not retriable.
 * (It was 13 until the server assigned that byte to THUNDERSTRIKE_BUSY.) */
#define THUNDERSTRIKE_ERR_TLS              (-13)
/* The value is longer than the server takes in one request (4 GiB - 1);
 * refused before anything was sent. */
#define THUNDERSTRIKE_ERR_VALUE_TOO_LARGE  (-14)

/* ---- thunderstrike_retry_class(code) ---- */
/* The same request fails the same way, or must not be repeated. */
#define THUNDERSTRIKE_RETRY_NEVER          0
/* The outcome is unknown (the connection dropped, or the deadline passed
 * with the request in flight): repeat only GET, SET, EXISTS and PING. */
#define THUNDERSTRIKE_RETRY_IF_IDEMPOTENT  1
/* The server did not apply the request (THUNDERSTRIKE_BUSY): repeat any
 * operation after a short backoff. */
#define THUNDERSTRIKE_RETRY_ALWAYS         2

#define THUNDERSTRIKE_ROUTING_SHARD_BY_KEY 0
#define THUNDERSTRIKE_ROUTING_ROUND_ROBIN  1

/* The wire caps key length at 65535: key_len is a uint16 in the header. */
#define THUNDERSTRIKE_MAX_KEY_LEN 65535

/* ===========================================================================
 * Types
 * ======================================================================== */

typedef struct thunderstrike_config thunderstrike_config;
typedef struct thunderstrike_client thunderstrike_client;

/*
 * An owned block of bytes. `data` is valid until thunderstrike_buf_free.
 * A zero-length value comes back as { NULL, 0, NULL }, which is still
 * safe to pass to thunderstrike_buf_free.
 */
typedef struct {
    const uint8_t *data;
    size_t         len;
    void          *_owner; /* opaque; do not touch */
} thunderstrike_buf;

/* Pool counters. */
typedef struct {
    uint64_t connections;
    uint64_t connections_ready;
    uint64_t inflight;          /* requests on the wire right now */
    uint64_t capacity;          /* requests that could be, at once */
    uint64_t requests;
    uint64_t responses;
    uint64_t bytes_written;
    uint64_t bytes_read;
    uint64_t write_batches;     /* requests/write_batches = batching factor */
    uint64_t reconnects;
    uint64_t orphan_responses;  /* answers whose waiter had given up */

    /* The I/O pool. Connections are sockets; their readers and writers
     * are tasks, so these are the numbers that say how many cores the
     * client can occupy — not the connection count. */
    uint64_t io_threads;        /* running right now */
    uint64_t io_threads_min;    /* the range it may move in */
    uint64_t io_threads_max;
    uint64_t io_grew;           /* times it widened */
    uint64_t io_shrank;         /* times it narrowed */
    uint64_t io_moved;          /* connections moved between threads */
} thunderstrike_stats;

/*
 * Delivered on a client worker thread when an *_async call finishes.
 *
 * `data` is valid ONLY for the duration of the call — copy what you need.
 * It carries the value for get, and nothing for set/delete.
 *
 * The callback must not block and must not call a blocking thunderstrike_*
 * function.
 */
typedef void (*thunderstrike_callback)(void *user_data,
                                 int32_t code,
                                 const uint8_t *data,
                                 size_t len);

/* ===========================================================================
 * Configuration
 * ======================================================================== */

/* Returns NULL only on allocation failure. Free with thunderstrike_config_free. */
thunderstrike_config *thunderstrike_config_new(void);
void            thunderstrike_config_free(thunderstrike_config *cfg);

/*
 * Add an endpoint, e.g. "127.0.0.1:7379".
 *
 * Under shard-by-key routing ONE is enough: the client asks that address
 * for the deployment's routing table and opens a connection to every port
 * it names. Adding more only widens where it may bootstrap from.
 */
int32_t thunderstrike_config_add_endpoint(thunderstrike_config *cfg, const char *addr);

/*
 * State the layout of a thunderstrike started with --shards n --cores m: m
 * endpoints at base_port + core, each owning a contiguous run of shards.
 *
 * Optional, and usually unnecessary -- one endpoint plus discovery finds
 * the same thing. What this adds is a CHECK: the stated layout is compared
 * against the one the server hands back, and thunderstrike_connect fails if they
 * differ. Numbers that are merely plausible are the dangerous kind.
 *
 * Replaces the endpoint list rather than adding to it.
 */
int32_t thunderstrike_config_set_cluster(thunderstrike_config *cfg,
                                   const char *host,
                                   uint16_t base_port,
                                   uint16_t shards,
                                   uint16_t cores);

/* REMOVED in the key-routing release. A shard is no longer a port, so
 * one endpoint per shard is not a layout any server has:
 *
 *   int32_t thunderstrike_config_add_shard_range(thunderstrike_config *, const char *,
 *                                          uint16_t base_port,
 *                                          uint16_t shards);
 *
 * Use thunderstrike_config_add_endpoint with one address, or
 * thunderstrike_config_set_cluster to state the layout and have it verified.
 */


/* TCP connections per endpoint. Default: 4 for a narrow pool, 1 for >= 8
 * endpoints. */
int32_t thunderstrike_config_set_connections_per_endpoint(thunderstrike_config *cfg,
                                                    uint32_t n);

/* Pipeline depth per connection, 1..65535. Default 1024. */
int32_t thunderstrike_config_set_max_inflight(thunderstrike_config *cfg, uint32_t n);

/* Encoded-but-unwritten bytes allowed per connection. Default 8 MiB. */
int32_t thunderstrike_config_set_max_buffered_bytes(thunderstrike_config *cfg, uint64_t n);

/*
 * Largest response frame the client will buffer. Default: the largest the
 * server can produce (a 4 GiB - 1 value plus the 28-byte prefix).
 *
 * value_len is a uint64 on the wire and the server has no frame-size
 * cap, so without this a corrupt length prefix would become an
 * arbitrary allocation. A frame over the ceiling is a protocol error:
 * the connection is dropped and rebuilt. Raise it if the deployment
 * really serves values this large.
 */
int32_t thunderstrike_config_set_max_response_bytes(thunderstrike_config *cfg, uint64_t n);

/*
 * Per-request deadline, covering queueing and every retry. 0 disables it.
 * Default 60000: longer than the server's own bounded waits (5 s before an
 * Oom, 30 s in total before a Busy), so its genuine answer arrives before
 * the client gives up. A request whose deadline passes may still execute.
 */
int32_t thunderstrike_config_set_request_timeout_ms(thunderstrike_config *cfg, uint64_t ms);

/* Extra deadline per MiB of a SET/APPEND's own value. Default 100 (a
 * 10 MiB/s floor: a 512 MiB value earns 51.2 s more). 0 disables it. */
int32_t thunderstrike_config_set_timeout_per_mib_ms(thunderstrike_config *cfg, uint64_t ms);

/* Connect + TLS handshake deadline. Must be non-zero. Default 5000. */
int32_t thunderstrike_config_set_connect_timeout_ms(thunderstrike_config *cfg, uint64_t ms);

/* TCP keepalive idle time. 0 leaves the OS default. Default 15000. */
int32_t thunderstrike_config_set_tcp_keepalive_ms(thunderstrike_config *cfg, uint64_t ms);

/* TCP_NODELAY. Non-zero enables it. Default: on. */
int32_t thunderstrike_config_set_tcp_nodelay(thunderstrike_config *cfg, int32_t on);

/*
 * Retries for an IDEMPOTENT request whose connection dropped. get, set,
 * exists and ping retry; delete and append never do; a timeout never does.
 * Default 1.
 */
int32_t thunderstrike_config_set_max_retries(thunderstrike_config *cfg, uint32_t n);

/*
 * Retries for a request the server answered THUNDERSTRIKE_BUSY (its contract: not
 * applied). Every operation but DELETE — a DELETE may already have taken
 * effect before its metadata flush was refused, so it is returned to you.
 * Default 4. 0 returns every Busy.
 */
int32_t thunderstrike_config_set_busy_retries(thunderstrike_config *cfg, uint32_t n);

/*
 * Frame multiplexing. Non-zero (the default) sends a HELLO on every
 * connection asking the server for it: values larger than the chunk size
 * then go as OPEN + DATA chunks, interleaved with the other frames on the
 * connection, and the server sends large replies the same way — so a small
 * request or reply waits behind at most one chunk of a large transfer, not
 * the whole value. A server that does not grant it (THUNDERSTRIKE_MUX=0), or does
 * not know HELLO, gets the legacy framing. 0 forces the legacy framing and
 * sends no HELLO.
 */
int32_t thunderstrike_config_set_multiplexing(thunderstrike_config *cfg, int32_t on);

/* Chunk size for values on a multiplexed connection: larger values go in
 * chunks of this many bytes. Default 1048576 (1 MiB); held to the server's
 * grant (4 MiB today). Must be non-zero. */
int32_t thunderstrike_config_set_mux_chunk_bytes(thunderstrike_config *cfg, uint64_t n);

/* Chunked request bodies open on the wire at once on a multiplexed
 * connection. Default 1: one body's chunks run to its last before the next
 * opens (small frames still go between chunks). Held to the server's grant
 * (4 today). Must be at least 1. More than one is unsafe against the server
 * at 36db4921 under arena pressure: two bodies on one connection can wait on
 * each other. */
int32_t thunderstrike_config_set_mux_max_open_bodies(thunderstrike_config *cfg, uint32_t n);

/* The Busy backoff: first wait and ceiling, in ms; it doubles with jitter.
 * Defaults 2 and 200. min must not exceed max. */
int32_t thunderstrike_config_set_busy_backoff_ms(thunderstrike_config *cfg,
                                           uint64_t min_ms, uint64_t max_ms);

/* No Busy retry starts once the operation has run this long. Default 5000.
 * A Busy the server sends at once (a full queue, a lost race) is retried;
 * one it sends after waiting out its own bound (up to 30 s for memory) is
 * returned then, not after several more such waits. */
int32_t thunderstrike_config_set_busy_retry_window_ms(thunderstrike_config *cfg, uint64_t ms);

/* THUNDERSTRIKE_ROUTING_SHARD_BY_KEY (default) or THUNDERSTRIKE_ROUTING_ROUND_ROBIN. */
int32_t thunderstrike_config_set_routing(thunderstrike_config *cfg, int32_t routing);

/* Worker threads for the client's runtime. 0 means one per core. */
int32_t thunderstrike_config_set_worker_threads(thunderstrike_config *cfg, uint32_t n);

/* The I/O thread pool.
 *
 * Connections are sockets; their readers and writers are tasks, so
 * 1024 connections are not 1024 threads. These two decide how many
 * threads — and so how many cores — the client can use. The pool
 * starts at the floor, divides the connections among the threads that
 * are running, and widens towards the ceiling when they saturate.
 *
 * Defaults: floor 1, ceiling the machine's cores, further clamped by
 * the number of connections there are to drive. */
/* Speak TLS 1.3, verifying the server against `ca_pem`.
 *
 * `server_name` is sent as SNI AND matched against the certificate's
 * subject alternative names, so it must be a name the certificate was
 * issued for -- not necessarily the host in the endpoint.
 *
 * `ca_pem` is PEM text: the server's CA certificate (ca.crt in its release kit).
 * Both arguments are copied before this returns.
 *
 * There is deliberately no way to disable verification. */
int32_t thunderstrike_config_set_tls(thunderstrike_config *cfg,
                               const char *server_name,
                               const uint8_t *ca_pem,
                               size_t ca_pem_len);

/* mTLS: present this client certificate when the server asks for one (a
 * server started with THUNDERSTRIKE_TLS_CLIENT_CA requires it of everyone).
 *
 * `cert_pem` is the client's PEM certificate chain, leaf first, chaining
 * to the server's client CA; `key_pem` is the leaf's PKCS#8 Ed25519 key.
 * A key that does not match the certificate is refused here with
 * THUNDERSTRIKE_ERR_CONFIG. Either order with thunderstrike_config_set_tls works. Both
 * are copied before this returns. */
int32_t thunderstrike_config_set_client_identity(thunderstrike_config *cfg,
                                           const uint8_t *cert_pem,
                                           size_t cert_pem_len,
                                           const uint8_t *key_pem,
                                           size_t key_pem_len);

/* Non-zero permits connecting WITHOUT TLS — for the in-repo mock server
 * only. The thunderstrike server is TLS-only, and thunderstrike_connect refuses a
 * config with neither TLS nor this (THUNDERSTRIKE_ERR_CONFIG). */
int32_t thunderstrike_config_set_insecure_plaintext(thunderstrike_config *cfg, int32_t on);

int32_t thunderstrike_config_set_min_io_threads(thunderstrike_config *cfg, uint32_t n);
int32_t thunderstrike_config_set_max_io_threads(thunderstrike_config *cfg, uint32_t n);
int32_t thunderstrike_config_set_io_sample_interval_ms(thunderstrike_config *cfg, uint64_t ms);
int32_t thunderstrike_config_set_io_idle_timeout_ms(thunderstrike_config *cfg, uint64_t ms);

/* ===========================================================================
 * Lifecycle
 * ======================================================================== */

/*
 * Connect and write the handle to *out.
 *
 * Every connection is established before this returns, so a bad address or
 * a certificate that does not verify fails here rather than on first use.
 * The config is not consumed; free it separately.
 */
int32_t thunderstrike_connect(const thunderstrike_config *cfg, thunderstrike_client **out);

/* Shut the pool down and release the handle. Safe on NULL. */
void thunderstrike_client_free(thunderstrike_client *client);

/* ===========================================================================
 * Blocking operations
 * ======================================================================== */

/*
 * Read a key into an owned buffer.
 *
 * THUNDERSTRIKE_OK fills *out (release it with thunderstrike_buf_free);
 * THUNDERSTRIKE_NOT_FOUND leaves it empty. *out is always initialised first, so
 * an unconditional thunderstrike_buf_free is safe on any return.
 */
int32_t thunderstrike_get(thunderstrike_client *client,
                    const uint8_t *key, size_t key_len,
                    thunderstrike_buf *out);

/*
 * Read a key straight into your own buffer — no allocation, nothing to free.
 *
 * *out_len is set to the value's true length whether or not it fit, so
 * THUNDERSTRIKE_ERR_BUFFER_TOO_SMALL tells you exactly how much to bring back.
 */
int32_t thunderstrike_get_into(thunderstrike_client *client,
                         const uint8_t *key, size_t key_len,
                         uint8_t *dst, size_t dst_cap, size_t *out_len);

int32_t thunderstrike_set(thunderstrike_client *client,
                    const uint8_t *key, size_t key_len,
                    const uint8_t *value, size_t value_len);

/* As thunderstrike_set, with a ttl_ms hint in the request header. */
int32_t thunderstrike_set_ttl(thunderstrike_client *client,
                        const uint8_t *key, size_t key_len,
                        const uint8_t *value, size_t value_len,
                        uint32_t ttl_ms);

/* THUNDERSTRIKE_OK removed it; THUNDERSTRIKE_NOT_FOUND means it was not there. */
int32_t thunderstrike_delete(thunderstrike_client *client,
                       const uint8_t *key, size_t key_len);

/* THUNDERSTRIKE_OK present; THUNDERSTRIKE_NOT_FOUND absent. */
int32_t thunderstrike_exists(thunderstrike_client *client,
                       const uint8_t *key, size_t key_len);

/* Append to a key's value; *out_total (may be NULL) gets the new length. */
int32_t thunderstrike_append(thunderstrike_client *client,
                       const uint8_t *key, size_t key_len,
                       const uint8_t *value, size_t value_len,
                       uint64_t *out_total);

int32_t thunderstrike_ping(thunderstrike_client *client);

/* ===========================================================================
 * Pipelined batches
 *
 * These put every request on the wire before awaiting any answer, so n keys
 * cost one round trip instead of n. Use them wherever you would otherwise
 * loop over the single-key calls.
 * ======================================================================== */

/*
 * Read n keys in one round trip.
 *
 * out_codes[i] is THUNDERSTRIKE_OK / THUNDERSTRIKE_NOT_FOUND / an error for key i.
 * out_values[i] is filled only for THUNDERSTRIKE_OK and must then be released.
 * Every out_values[i] is initialised, so freeing all n is always safe.
 * The return value covers argument validation only.
 */
int32_t thunderstrike_mget(thunderstrike_client *client,
                     const uint8_t *const *keys, const size_t *key_lens,
                     size_t n,
                     thunderstrike_buf *out_values, int32_t *out_codes);

/* Write n pairs in one round trip. out_codes may be NULL. */
int32_t thunderstrike_mset(thunderstrike_client *client,
                     const uint8_t *const *keys, const size_t *key_lens,
                     const uint8_t *const *values, const size_t *value_lens,
                     size_t n,
                     int32_t *out_codes);

/* Delete n keys in one round trip. out_codes may be NULL. */
int32_t thunderstrike_mdelete(thunderstrike_client *client,
                        const uint8_t *const *keys, const size_t *key_lens,
                        size_t n,
                        int32_t *out_codes);

/* ===========================================================================
 * Callback operations
 *
 * These return as soon as the request is queued. The callback fires on a
 * client worker thread. Keys and values are copied before the call returns;
 * user_data is not — it must stay valid until the callback runs, and must
 * be safe to touch from another thread.
 * ======================================================================== */

int32_t thunderstrike_get_async(thunderstrike_client *client,
                          const uint8_t *key, size_t key_len,
                          thunderstrike_callback cb, void *user_data);

int32_t thunderstrike_set_async(thunderstrike_client *client,
                          const uint8_t *key, size_t key_len,
                          const uint8_t *value, size_t value_len,
                          thunderstrike_callback cb, void *user_data);

int32_t thunderstrike_delete_async(thunderstrike_client *client,
                             const uint8_t *key, size_t key_len,
                             thunderstrike_callback cb, void *user_data);

/* ===========================================================================
 * Buffers, introspection, diagnostics
 * ======================================================================== */

/*
 * Release a buffer. Safe on NULL, on a zeroed buffer, and on one already
 * freed — the fields are cleared, so a double free is a no-op.
 */
void thunderstrike_buf_free(thunderstrike_buf *buf);

int32_t thunderstrike_get_stats(thunderstrike_client *client, thunderstrike_stats *out);

/*
 * Frame multiplexing counters: connections that negotiated it and are up
 * now, requests sent in chunks, replies received in chunks. Any out pointer
 * may be NULL. (Kept apart from thunderstrike_stats so that struct's layout does
 * not change.)
 */
int32_t thunderstrike_get_mux_stats(thunderstrike_client *client,
                              uint64_t *out_mux_connections,
                              uint64_t *out_chunked_requests,
                              uint64_t *out_chunked_replies);

/*
 * Which shard owns `key`, under the routing table this client read from
 * the server. 0 under round-robin routing, which has no shards.
 */
int32_t thunderstrike_shard_of(thunderstrike_client *client,
                         const uint8_t *key, size_t key_len,
                         uint16_t *out_shard);

/*
 * The shard hash on its own, for callers that route themselves:
 * CRC32C(key) % shards, hardware-accelerated. The same function the server
 * places data with, so a caller doing its own routing lands where the
 * server expects. `shards` of 0 reads as 1.
 */
int32_t thunderstrike_hash_shard(const uint8_t *key, size_t key_len,
                           uint16_t shards, uint16_t *out_shard);

/* The deployment's shard count, as the server reported it. 0 under
 * round-robin routing. */
int32_t thunderstrike_shard_count(thunderstrike_client *client, uint16_t *out_shards);

/* Endpoints this client is connected to -- one per server core,
 * discovered rather than configured. */
int32_t thunderstrike_endpoint_count(thunderstrike_client *client, uint32_t *out_endpoints);

/* Static, NUL-terminated. Never NULL, never needs freeing. */
const char *thunderstrike_strerror(int32_t code);

/* THUNDERSTRIKE_RETRY_NEVER, THUNDERSTRIKE_RETRY_IF_IDEMPOTENT or THUNDERSTRIKE_RETRY_ALWAYS:
 * whether a request that failed with `code` may be sent again. */
int32_t thunderstrike_retry_class(int32_t code);

/* The detail behind THIS THREAD's most recent failure, or "" if there
 * is none. Never null; never needs freeing.
 *
 * thunderstrike_strerror names the category ("TLS failed"); this names the
 * cause ("tls: TLS handshake failed: X509(NameMismatch)"). For TLS
 * that difference is the whole diagnosis — a certificate issued for
 * another name, an untrusted issuer and an expired one all share a
 * code and have different fixes.
 *
 * Valid until the next failing call on this thread, so copy it before
 * making another. */
const char *thunderstrike_last_error_message(void);

/* This library's version, NUL-terminated. */
const char *thunderstrike_version(void);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* THUNDERSTRIKE_CLIENT_H */
