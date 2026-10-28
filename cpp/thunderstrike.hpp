// thunderstrike.hpp — a header-only C++ wrapper over the thunderstrike C ABI.
//
// C++17. No dependencies beyond the standard library and
// thunderstrike_client.h. Nothing to compile but your own code: include
// this, link libthunderstrike_client, done.
//
//     #include <thunderstrike.hpp>
//
//     auto db = thunderstrike::client::connect("127.0.0.1:7379");
//
//     db.set("user:1", "ada");
//     if (auto v = db.get("user:1")) {
//         std::cout << v->view() << "\n";   // ada
//     }
//     // a miss is an empty optional, not an exception
//
//     auto values = db.get_many(keys);      // one round trip, not N
//
// What this adds over calling the C API directly:
//
//   * every handle and buffer is released by a destructor, including
//     on the exception paths, which is where hand-written C leaks;
//   * a non-OK status becomes a typed exception carrying the numeric
//     code, so `catch (const thunderstrike::error&)` works;
//   * string_view in, so no length arguments to get wrong and no
//     assumption that keys are NUL-terminated.
//
// Everything here is binary-safe: string_view is a pointer and a
// length, so embedded NULs pass through untouched.

#ifndef THUNDERSTRIKE_HPP
#define THUNDERSTRIKE_HPP

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <optional>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include "thunderstrike_client.h"

#if __cplusplus < 201703L && !(defined(_MSVC_LANG) && _MSVC_LANG >= 201703L)
#error "thunderstrike.hpp requires C++17 or later"
#endif

namespace thunderstrike {

// ===========================================================================
// error
// ===========================================================================

/// Any non-OK status from the client or the server.
///
/// A miss is *not* an error: `get` returns an empty optional and
/// `remove`/`exists` return false.
class error : public std::runtime_error {
public:
    explicit error(std::int32_t code, const char* op = nullptr)
        : std::runtime_error(build(code, op)), code_(code) {}

    /// The numeric status. See the `THUNDERSTRIKE_*` constants.
    std::int32_t code() const noexcept { return code_; }

    /// True when the key was simply absent.
    bool not_found() const noexcept { return code_ == THUNDERSTRIKE_NOT_FOUND; }

    /// True when the deadline elapsed. The server may still have
    /// applied the request — thunderstrike has no cancel opcode.
    bool timed_out() const noexcept { return code_ == THUNDERSTRIKE_ERR_TIMEOUT; }

    /// True when the server answered Busy: it did NOT apply the
    /// request, and the same request may be sent again after a short
    /// backoff. The client already retried it (all but remove).
    bool busy() const noexcept { return code_ == THUNDERSTRIKE_BUSY; }

    /// `THUNDERSTRIKE_RETRY_NEVER`, `THUNDERSTRIKE_RETRY_IF_IDEMPOTENT` (the outcome
    /// is unknown: repeat get/set/exists/ping only) or
    /// `THUNDERSTRIKE_RETRY_ALWAYS` (Busy: not applied).
    std::int32_t retry_class() const noexcept { return thunderstrike_retry_class(code_); }

private:
    static std::string build(std::int32_t code, const char* op) {
        std::string s = "thunderstrike: ";
        if (op != nullptr) {
            s += op;
            s += ": ";
        }
        s += thunderstrike_strerror(code);
        s += " (code " + std::to_string(code) + ")";
        return s;
    }

    std::int32_t code_;
};

namespace detail {

inline void check(std::int32_t code, const char* op) {
    if (code != THUNDERSTRIKE_OK) {
        throw error(code, op);
    }
}

inline const std::uint8_t* bytes(std::string_view s) noexcept {
    // A zero-length view may hold a null pointer; the ABI reads no
    // bytes for length 0, so passing it through is fine.
    return reinterpret_cast<const std::uint8_t*>(s.data());
}

} // namespace detail

// ===========================================================================
// buffer
// ===========================================================================

/// An owned value returned by the client.
///
/// Move-only, and releases the underlying `thunderstrike_buf` in its
/// destructor — which is the whole point, because the C API's buffers
/// hold a reference into the connection's read buffer until freed.
class buffer {
public:
    buffer() noexcept : buf_{nullptr, 0, nullptr} {}

    buffer(buffer&& other) noexcept : buf_(other.buf_) {
        other.buf_ = thunderstrike_buf{nullptr, 0, nullptr};
    }

    buffer& operator=(buffer&& other) noexcept {
        if (this != &other) {
            reset();
            buf_ = other.buf_;
            other.buf_ = thunderstrike_buf{nullptr, 0, nullptr};
        }
        return *this;
    }

    buffer(const buffer&) = delete;
    buffer& operator=(const buffer&) = delete;

    ~buffer() { reset(); }

    const std::uint8_t* data() const noexcept { return buf_.data; }
    std::size_t size() const noexcept { return buf_.len; }
    bool empty() const noexcept { return buf_.len == 0; }

    /// A view over the bytes. Valid until this buffer is destroyed or
    /// moved from.
    std::string_view view() const noexcept {
        return std::string_view(reinterpret_cast<const char*>(buf_.data), buf_.len);
    }

    /// A copy that outlives this buffer.
    std::string str() const { return std::string(view()); }

    /// Release the bytes early. Freeing promptly lets the client reuse
    /// that part of its read buffer.
    void reset() noexcept {
        if (buf_.data != nullptr || buf_._owner != nullptr) {
            thunderstrike_buf_free(&buf_);
        }
        buf_ = thunderstrike_buf{nullptr, 0, nullptr};
    }

    /// The raw struct, for handing to the C API.
    thunderstrike_buf* raw() noexcept { return &buf_; }

private:
    thunderstrike_buf buf_;
};

// ===========================================================================
// stats
// ===========================================================================

/// Frame multiplexing counters.
struct mux_stats {
    /// Connections that negotiated multiplexing and are up now.
    std::uint64_t connections = 0;
    /// Requests whose value went in chunks.
    std::uint64_t chunked_requests = 0;
    /// Replies that came in chunks.
    std::uint64_t chunked_replies = 0;
};

/// Pool counters.
struct stats {
    std::uint64_t connections = 0;
    std::uint64_t connections_ready = 0;
    /// Requests on the wire right now.
    std::uint64_t inflight = 0;
    /// Requests that could be on the wire at once.
    std::uint64_t capacity = 0;
    std::uint64_t requests = 0;
    std::uint64_t responses = 0;
    std::uint64_t bytes_written = 0;
    std::uint64_t bytes_read = 0;
    /// `write` syscalls.
    std::uint64_t write_batches = 0;
    std::uint64_t reconnects = 0;
    /// Answers whose waiter had already given up.
    std::uint64_t orphan_responses = 0;

    /// The I/O pool: threads running now, the range they may move in,
    /// and how often it has reshaped. These are the numbers that say
    /// how many cores the client can occupy — the connection count is
    /// not.
    std::uint64_t io_threads = 0;
    std::uint64_t io_threads_min = 0;
    std::uint64_t io_threads_max = 0;
    std::uint64_t io_grew = 0;
    std::uint64_t io_shrank = 0;
    std::uint64_t io_moved = 0;

    /// Requests per `write` syscall — the payoff from coalescing.
    double batching_factor() const noexcept {
        return write_batches == 0
                   ? 0.0
                   : static_cast<double>(requests) / static_cast<double>(write_batches);
    }
};

// ===========================================================================
// options
// ===========================================================================

/// Connection and pool settings.
///
/// Owns a `thunderstrike_config` and frees it in its destructor, so an
/// exception thrown midway through configuration cannot leak it.
class options {
public:
    options() : cfg_(thunderstrike_config_new()) {
        if (cfg_ == nullptr) {
            throw std::bad_alloc();
        }
    }

    options(options&& other) noexcept : cfg_(other.cfg_) { other.cfg_ = nullptr; }

    options& operator=(options&& other) noexcept {
        if (this != &other) {
            if (cfg_ != nullptr) {
                thunderstrike_config_free(cfg_);
            }
            cfg_ = other.cfg_;
            other.cfg_ = nullptr;
        }
        return *this;
    }

    options(const options&) = delete;
    options& operator=(const options&) = delete;

    ~options() {
        if (cfg_ != nullptr) {
            thunderstrike_config_free(cfg_);
        }
    }

    /// Add an endpoint. With shard-by-key routing the order IS the
    /// bootstrap address. ONE is enough: the client asks it for the
    /// deployment's routing table and connects to every port it names.
    options& endpoint(const std::string& addr) {
        detail::check(thunderstrike_config_add_endpoint(cfg_, addr.c_str()), "endpoint");
        return *this;
    }

    /// State the layout of a thunderstrike started with `--shards n
    /// --cores m`: core `c` listens on `base_port + c` and owns a
    /// contiguous run of shards.
    ///
    /// Optional, and usually unnecessary -- one endpoint plus
    /// discovery finds the same thing. What this adds is a check: the
    /// stated layout is compared against the one the server serves,
    /// and connecting fails if they differ.
    options& cluster(const std::string& host, std::uint16_t base_port,
                     std::uint16_t shards, std::uint16_t cores) {
        detail::check(
            thunderstrike_config_set_cluster(cfg_, host.c_str(), base_port, shards, cores),
            "cluster");
        return *this;
    }

    /// Speak TLS 1.3, verifying the server against `ca_pem`. The
    /// thunderstrike server is TLS-only, so this is required (see
    /// `insecure_plaintext` for the one exception).
    ///
    /// `server_name` is sent as SNI **and** matched against the
    /// certificate's subject alternative names, so it must be a name
    /// the certificate was issued for — not necessarily the host in
    /// the endpoint. `ca_pem` is PEM text, the server's CA certificate
    /// (`ca.crt` in its release kit). There is no way to disable verification.
    options& tls(const std::string& server_name, std::string_view ca_pem) {
        detail::check(thunderstrike_config_set_tls(cfg_, server_name.c_str(),
                                             detail::bytes(ca_pem), ca_pem.size()),
                      "tls");
        return *this;
    }

    /// mTLS: the client certificate chain (leaf first) and its PKCS#8
    /// Ed25519 key, both PEM, for a server started with
    /// `THUNDERSTRIKE_TLS_CLIENT_CA`. Either order with `tls` works.
    options& client_identity(std::string_view cert_pem, std::string_view key_pem) {
        detail::check(thunderstrike_config_set_client_identity(cfg_, detail::bytes(cert_pem),
                                                         cert_pem.size(), detail::bytes(key_pem),
                                                         key_pem.size()),
                      "client_identity");
        return *this;
    }

    /// Connect WITHOUT TLS. Only the in-repo mock server answers that;
    /// the thunderstrike server is TLS-only.
    options& insecure_plaintext(bool on = true) {
        detail::check(thunderstrike_config_set_insecure_plaintext(cfg_, on ? 1 : 0),
                      "insecure_plaintext");
        return *this;
    }

    /// Retries for a request the server answered Busy (not applied),
    /// every operation but remove. Default 4.
    options& busy_retries(std::uint32_t n) {
        detail::check(thunderstrike_config_set_busy_retries(cfg_, n), "busy_retries");
        return *this;
    }

    /// The Busy backoff, first wait and ceiling. Defaults 2 / 200 ms.
    options& busy_backoff_ms(std::uint64_t min_ms, std::uint64_t max_ms) {
        detail::check(thunderstrike_config_set_busy_backoff_ms(cfg_, min_ms, max_ms), "busy_backoff_ms");
        return *this;
    }

    /// No Busy retry starts once the operation has run this long.
    /// Default 5000.
    options& busy_retry_window_ms(std::uint64_t ms) {
        detail::check(thunderstrike_config_set_busy_retry_window_ms(cfg_, ms), "busy_retry_window_ms");
        return *this;
    }

    /// Extra deadline per MiB of a request's own value. Default 100.
    options& timeout_per_mib_ms(std::uint64_t ms) {
        detail::check(thunderstrike_config_set_timeout_per_mib_ms(cfg_, ms), "timeout_per_mib_ms");
        return *this;
    }

    options& connections_per_endpoint(std::uint32_t n) {
        detail::check(thunderstrike_config_set_connections_per_endpoint(cfg_, n),
                      "connections_per_endpoint");
        return *this;
    }

    /// Pipeline depth per connection, 1..65535.
    options& max_inflight(std::uint32_t n) {
        detail::check(thunderstrike_config_set_max_inflight(cfg_, n), "max_inflight");
        return *this;
    }

    options& max_buffered_bytes(std::uint64_t n) {
        detail::check(thunderstrike_config_set_max_buffered_bytes(cfg_, n), "max_buffered_bytes");
        return *this;
    }

    /// Largest response frame the client will buffer.
    options& max_response_bytes(std::uint64_t n) {
        detail::check(thunderstrike_config_set_max_response_bytes(cfg_, n), "max_response_bytes");
        return *this;
    }

    /// Per-request deadline, covering every retry. 0 disables it.
    /// Default 60000 — longer than the server's own bounded waits, so
    /// its genuine Busy/Oom arrives before the client gives up.
    options& request_timeout_ms(std::uint64_t ms) {
        detail::check(thunderstrike_config_set_request_timeout_ms(cfg_, ms), "request_timeout_ms");
        return *this;
    }

    options& connect_timeout_ms(std::uint64_t ms) {
        detail::check(thunderstrike_config_set_connect_timeout_ms(cfg_, ms), "connect_timeout_ms");
        return *this;
    }

    /// TCP keepalive idle period. 0 leaves the OS default.
    options& tcp_keepalive_ms(std::uint64_t ms) {
        detail::check(thunderstrike_config_set_tcp_keepalive_ms(cfg_, ms), "tcp_keepalive_ms");
        return *this;
    }

    options& tcp_nodelay(bool on) {
        detail::check(thunderstrike_config_set_tcp_nodelay(cfg_, on ? 1 : 0), "tcp_nodelay");
        return *this;
    }

    /// Frame multiplexing, on by default: a value larger than the chunk
    /// size travels in chunks with other requests and replies between
    /// them, so a small operation never waits behind a whole large one
    /// on its connection. `false` forces the legacy framing (no HELLO).
    options& multiplexing(bool on) {
        detail::check(thunderstrike_config_set_multiplexing(cfg_, on ? 1 : 0), "multiplexing");
        return *this;
    }

    /// Chunk size for large values on a multiplexed connection. Default
    /// 1 MiB; held to the server's grant. Must be non-zero.
    options& mux_chunk_bytes(std::uint64_t n) {
        detail::check(thunderstrike_config_set_mux_chunk_bytes(cfg_, n), "mux_chunk_bytes");
        return *this;
    }

    /// Chunked request bodies open at once on a multiplexed connection.
    /// Default 1; held to the server's grant. At least 1. More than one
    /// is unsafe against the server at 36db4921 under arena pressure.
    options& mux_max_open_bodies(std::uint32_t n) {
        detail::check(thunderstrike_config_set_mux_max_open_bodies(cfg_, n), "mux_max_open_bodies");
        return *this;
    }

    /// Retries for an idempotent request whose connection dropped.
    /// get, set, exists and ping retry; remove and append never do,
    /// and a timeout never does.
    options& max_retries(std::uint32_t n) {
        detail::check(thunderstrike_config_set_max_retries(cfg_, n), "max_retries");
        return *this;
    }

    /// `THUNDERSTRIKE_ROUTING_SHARD_BY_KEY` (default) or
    /// `THUNDERSTRIKE_ROUTING_ROUND_ROBIN`.
    options& routing(std::int32_t r) {
        detail::check(thunderstrike_config_set_routing(cfg_, r), "routing");
        return *this;
    }

    options& worker_threads(std::uint32_t n) {
        detail::check(thunderstrike_config_set_worker_threads(cfg_, n), "worker_threads");
        return *this;
    }

    const thunderstrike_config* raw() const noexcept { return cfg_; }

private:
    thunderstrike_config* cfg_;
};

// ===========================================================================
// client
// ===========================================================================

/// A pooled, pipelined thunderstrike client.
///
/// Move-only, and closes the pool in its destructor. Thread-safe:
/// every method may be called concurrently from any thread, so share
/// one instance across the process.
///
/// For more than one key prefer `get_many` / `set_many` /
/// `remove_many`: they put the whole batch on the wire before
/// awaiting any answer, so n keys cost one round trip instead of n.
class client {
public:
    /// Connect to a single endpoint with default settings.
    static client connect(const std::string& endpoint) {
        options o;
        o.endpoint(endpoint);
        return connect(o);
    }

    /// Connect with explicit settings.
    ///
    /// Every connection in the pool is established (and
    /// authenticated) before this returns, so a wrong address or a
    /// bad token throws here rather than on first use.
    static client connect(const options& opts) {
        thunderstrike_client* handle = nullptr;
        detail::check(thunderstrike_connect(opts.raw(), &handle), "connect");
        return client(handle);
    }

    client(client&& other) noexcept : handle_(other.handle_) { other.handle_ = nullptr; }

    client& operator=(client&& other) noexcept {
        if (this != &other) {
            close();
            handle_ = other.handle_;
            other.handle_ = nullptr;
        }
        return *this;
    }

    client(const client&) = delete;
    client& operator=(const client&) = delete;

    ~client() { close(); }

    /// Shut the pool down. Idempotent; later calls throw.
    void close() noexcept {
        if (handle_ != nullptr) {
            thunderstrike_client_free(handle_);
            handle_ = nullptr;
        }
    }

    // ── Single keys ────────────────────────────────────────────────

    /// Read a key. An empty optional is a clean miss.
    std::optional<buffer> get(std::string_view key) const {
        buffer out;
        const std::int32_t code =
            thunderstrike_get(handle(), detail::bytes(key), key.size(), out.raw());
        if (code == THUNDERSTRIKE_NOT_FOUND) {
            return std::nullopt;
        }
        detail::check(code, "get");
        return std::optional<buffer>(std::move(out));
    }

    /// Read a key into memory you already own — no allocation and
    /// nothing to free.
    ///
    /// Returns the value's length, which may exceed `capacity`: that
    /// means nothing was copied and this is how large a buffer to
    /// bring back. An absent key yields an empty optional.
    std::optional<std::size_t> get_into(std::string_view key, void* dst,
                                        std::size_t capacity) const {
        std::size_t len = 0;
        const std::int32_t code =
            thunderstrike_get_into(handle(), detail::bytes(key), key.size(),
                             static_cast<std::uint8_t*>(dst), capacity, &len);
        if (code == THUNDERSTRIKE_NOT_FOUND) {
            return std::nullopt;
        }
        if (code == THUNDERSTRIKE_ERR_BUFFER_TOO_SMALL) {
            return len;
        }
        detail::check(code, "get_into");
        return len;
    }

    /// Write a key. `ttl_ms` is a hint carried in the request header.
    void set(std::string_view key, std::string_view value, std::uint32_t ttl_ms = 0) const {
        detail::check(thunderstrike_set_ttl(handle(), detail::bytes(key), key.size(),
                                      detail::bytes(value), value.size(), ttl_ms),
                      "set");
    }

    /// Delete a key. False means it was not there.
    ///
    /// Named `remove` because `delete` is a keyword.
    bool remove(std::string_view key) const {
        const std::int32_t code = thunderstrike_delete(handle(), detail::bytes(key), key.size());
        if (code == THUNDERSTRIKE_NOT_FOUND) {
            return false;
        }
        detail::check(code, "remove");
        return true;
    }

    /// Test for a key without transferring its value.
    bool exists(std::string_view key) const {
        const std::int32_t code = thunderstrike_exists(handle(), detail::bytes(key), key.size());
        if (code == THUNDERSTRIKE_NOT_FOUND) {
            return false;
        }
        detail::check(code, "exists");
        return true;
    }

    /// Append to a key's value, creating it when absent. Returns the
    /// value's total length afterwards.
    std::uint64_t append(std::string_view key, std::string_view value) const {
        std::uint64_t total = 0;
        detail::check(thunderstrike_append(handle(), detail::bytes(key), key.size(),
                                     detail::bytes(value), value.size(), &total),
                      "append");
        return total;
    }

    /// Round-trip a liveness probe.
    void ping() const { detail::check(thunderstrike_ping(handle()), "ping"); }

    // ── Batches ────────────────────────────────────────────────────

    /// Read many keys in ONE round trip. An empty optional is a miss.
    std::vector<std::optional<buffer>> get_many(const std::vector<std::string_view>& keys) const {
        const std::size_t n = keys.size();
        std::vector<std::optional<buffer>> out;
        out.reserve(n);
        if (n == 0) {
            return out;
        }

        batch b(keys);
        // `buffer` owns exactly one thunderstrike_buf, so a vector of them
        // releases every element even if a code below throws.
        std::vector<buffer> bufs(n);
        std::vector<thunderstrike_buf> raw(n);
        std::vector<std::int32_t> codes(n);

        detail::check(thunderstrike_mget(handle(), b.ptrs.data(), b.lens.data(), n, raw.data(),
                                   codes.data()),
                      "get_many");

        // Take ownership of every returned buffer immediately, before
        // anything can throw.
        for (std::size_t i = 0; i < n; ++i) {
            *bufs[i].raw() = raw[i];
        }
        for (std::size_t i = 0; i < n; ++i) {
            if (codes[i] == THUNDERSTRIKE_OK) {
                out.emplace_back(std::move(bufs[i]));
            } else if (codes[i] == THUNDERSTRIKE_NOT_FOUND) {
                out.emplace_back(std::nullopt);
            } else {
                throw error(codes[i], "get_many");
            }
        }
        return out;
    }

    /// Write many pairs in ONE round trip.
    void set_many(const std::vector<std::string_view>& keys,
                  const std::vector<std::string_view>& values) const {
        if (keys.size() != values.size()) {
            throw std::invalid_argument("thunderstrike: set_many needs as many values as keys");
        }
        const std::size_t n = keys.size();
        if (n == 0) {
            return;
        }

        batch k(keys);
        batch v(values);
        std::vector<std::int32_t> codes(n);

        detail::check(thunderstrike_mset(handle(), k.ptrs.data(), k.lens.data(), v.ptrs.data(),
                                   v.lens.data(), n, codes.data()),
                      "set_many");
        for (std::size_t i = 0; i < n; ++i) {
            if (codes[i] != THUNDERSTRIKE_OK) {
                throw error(codes[i], "set_many");
            }
        }
    }

    /// Delete many keys in ONE round trip.
    std::vector<bool> remove_many(const std::vector<std::string_view>& keys) const {
        const std::size_t n = keys.size();
        if (n == 0) {
            return {};
        }

        batch b(keys);
        std::vector<std::int32_t> codes(n);
        detail::check(thunderstrike_mdelete(handle(), b.ptrs.data(), b.lens.data(), n, codes.data()),
                      "remove_many");

        std::vector<bool> out;
        out.reserve(n);
        for (std::size_t i = 0; i < n; ++i) {
            if (codes[i] == THUNDERSTRIKE_OK) {
                out.push_back(true);
            } else if (codes[i] == THUNDERSTRIKE_NOT_FOUND) {
                out.push_back(false);
            } else {
                throw error(codes[i], "remove_many");
            }
        }
        return out;
    }

    // ── Introspection ──────────────────────────────────────────────

    /// The pool's counters.
    struct stats stats() const {
        thunderstrike_stats s{};
        detail::check(thunderstrike_get_stats(handle(), &s), "stats");
        return ::thunderstrike::stats{s.connections,   s.connections_ready, s.inflight,
                                s.capacity,      s.requests,          s.responses,
                                s.bytes_written, s.bytes_read,        s.write_batches,
                                s.reconnects,    s.orphan_responses,
                                s.io_threads,    s.io_threads_min,    s.io_threads_max,
                                s.io_grew,       s.io_shrank,         s.io_moved};
    }

    /// Frame multiplexing counters.
    struct mux_stats mux_stats() const {
        ::thunderstrike::mux_stats m{};
        detail::check(thunderstrike_get_mux_stats(handle(), &m.connections, &m.chunked_requests,
                                            &m.chunked_replies),
                      "mux_stats");
        return m;
    }

    /// Which shard this client routes `key` to, under the routing
    /// table it read from the server.
    std::uint16_t shard_of(std::string_view key) const {
        std::uint16_t shard = 0;
        detail::check(thunderstrike_shard_of(handle(), detail::bytes(key), key.size(), &shard),
                      "shard_of");
        return shard;
    }

    /// How many shards the deployment has, as the server stated it.
    /// 0 under round-robin routing, which has no shards.
    std::uint16_t shards() const {
        std::uint16_t n = 0;
        detail::check(thunderstrike_shard_count(handle(), &n), "shards");
        return n;
    }

    /// How many endpoints this client holds -- one per server core,
    /// discovered rather than configured.
    std::uint32_t endpoints() const {
        std::uint32_t n = 0;
        detail::check(thunderstrike_endpoint_count(handle(), &n), "endpoints");
        return n;
    }

    thunderstrike_client* raw() const noexcept { return handle_; }

private:
    explicit client(thunderstrike_client* handle) noexcept : handle_(handle) {}

    thunderstrike_client* handle() const {
        if (handle_ == nullptr) {
            throw error(THUNDERSTRIKE_ERR_CLOSED);
        }
        return handle_;
    }

    /// The parallel pointer/length arrays the batch ABI takes.
    ///
    /// The views point at the caller's own memory, which outlives the
    /// call — so unlike the managed-language bindings, there is no
    /// copy here at all.
    struct batch {
        std::vector<const std::uint8_t*> ptrs;
        std::vector<std::size_t> lens;

        explicit batch(const std::vector<std::string_view>& items) {
            ptrs.reserve(items.size());
            lens.reserve(items.size());
            for (const auto& item : items) {
                ptrs.push_back(detail::bytes(item));
                lens.push_back(item.size());
            }
        }
    };

    thunderstrike_client* handle_;
};

// ===========================================================================
// Free functions
// ===========================================================================

/// Which of `shards` shards owns `key`: `CRC32C(key) % shards`, the
/// same function the server places data with. Needs no client.
inline std::uint16_t shard_of(std::string_view key, std::uint16_t shards) {
    std::uint16_t shard = 0;
    detail::check(thunderstrike_hash_shard(detail::bytes(key), key.size(), shards, &shard), "shard_of");
    return shard;
}

/// The client library's version.
inline std::string version() { return std::string(thunderstrike_version()); }

} // namespace thunderstrike

#endif // THUNDERSTRIKE_HPP
