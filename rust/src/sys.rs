//! Raw declarations of the `thunderstrike_client` C ABI.
//!
//! One to one with `thunderstrike_client.h`; the safe API in the crate root
//! is built on these. Every function here is `unsafe` and follows the
//! header's contract: a `thunderstrike_buf` returned to you is freed with
//! [`thunderstrike_buf_free`], and [`thunderstrike_client_free`] is called
//! exactly once, after every other call on that handle has returned.

#![allow(non_camel_case_types, missing_docs)]

use core::ffi::{c_char, c_void};

pub const THUNDERSTRIKE_OK: i32 = 0;

pub const THUNDERSTRIKE_NOT_FOUND: i32 = 1;
pub const THUNDERSTRIKE_WRONG_SHARD: i32 = 2;
pub const THUNDERSTRIKE_CAS_CONFLICT: i32 = 3;
pub const THUNDERSTRIKE_SERVER_OOM: i32 = 4;
pub const THUNDERSTRIKE_NOT_AUTHED: i32 = 5;
pub const THUNDERSTRIKE_UNKNOWN_OP: i32 = 6;
pub const THUNDERSTRIKE_MALFORMED: i32 = 7;
pub const THUNDERSTRIKE_UNSUPPORTED_VERSION: i32 = 9;
pub const THUNDERSTRIKE_RESPONSE_TOO_LARGE: i32 = 11;
pub const THUNDERSTRIKE_STORAGE_EXHAUSTED: i32 = 12;
pub const THUNDERSTRIKE_BUSY: i32 = 13;
pub const THUNDERSTRIKE_SERVER_ERROR: i32 = 14;
pub const THUNDERSTRIKE_UNKNOWN_STATUS: i32 = 64;

pub const THUNDERSTRIKE_ERR_INVALID_ARG: i32 = -1;
pub const THUNDERSTRIKE_ERR_CONFIG: i32 = -2;
pub const THUNDERSTRIKE_ERR_CONNECT: i32 = -3;
pub const THUNDERSTRIKE_ERR_IO: i32 = -4;
pub const THUNDERSTRIKE_ERR_TIMEOUT: i32 = -5;
pub const THUNDERSTRIKE_ERR_DISCONNECTED: i32 = -6;
pub const THUNDERSTRIKE_ERR_PROTOCOL: i32 = -7;
pub const THUNDERSTRIKE_ERR_KEY_TOO_LONG: i32 = -8;
pub const THUNDERSTRIKE_ERR_CLOSED: i32 = -9;
pub const THUNDERSTRIKE_ERR_BUFFER_TOO_SMALL: i32 = -10;
pub const THUNDERSTRIKE_ERR_RUNTIME: i32 = -11;
pub const THUNDERSTRIKE_ERR_PANIC: i32 = -12;
pub const THUNDERSTRIKE_ERR_TLS: i32 = -13;
pub const THUNDERSTRIKE_ERR_VALUE_TOO_LARGE: i32 = -14;

pub const THUNDERSTRIKE_RETRY_NEVER: i32 = 0;
pub const THUNDERSTRIKE_RETRY_IF_IDEMPOTENT: i32 = 1;
pub const THUNDERSTRIKE_RETRY_ALWAYS: i32 = 2;

pub const THUNDERSTRIKE_ROUTING_SHARD_BY_KEY: i32 = 0;
pub const THUNDERSTRIKE_ROUTING_ROUND_ROBIN: i32 = 1;

pub const THUNDERSTRIKE_MAX_KEY_LEN: usize = 65535;

/// Opaque configuration builder.
#[repr(C)]
pub struct thunderstrike_config {
    _private: [u8; 0],
}

/// Opaque client handle.
#[repr(C)]
pub struct thunderstrike_client {
    _private: [u8; 0],
}

/// An owned block of bytes; release with [`thunderstrike_buf_free`].
#[repr(C)]
#[derive(Debug)]
pub struct thunderstrike_buf {
    pub data: *const u8,
    pub len: usize,
    pub _owner: *mut c_void,
}

impl thunderstrike_buf {
    /// `{ NULL, 0, NULL }`: what the library itself initialises out-buffers to.
    pub const fn empty() -> Self {
        Self { data: core::ptr::null(), len: 0, _owner: core::ptr::null_mut() }
    }
}

/// Pool counters.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct thunderstrike_stats {
    pub connections: u64,
    pub connections_ready: u64,
    pub inflight: u64,
    pub capacity: u64,
    pub requests: u64,
    pub responses: u64,
    pub bytes_written: u64,
    pub bytes_read: u64,
    pub write_batches: u64,
    pub reconnects: u64,
    pub orphan_responses: u64,
    pub io_threads: u64,
    pub io_threads_min: u64,
    pub io_threads_max: u64,
    pub io_grew: u64,
    pub io_shrank: u64,
    pub io_moved: u64,
}

/// Fired on a client worker thread when an `*_async` call finishes. `data`
/// is valid only for the duration of the call.
pub type thunderstrike_callback =
    Option<unsafe extern "C" fn(user_data: *mut c_void, code: i32, data: *const u8, len: usize)>;

extern "C" {
    // Configuration.
    pub fn thunderstrike_config_new() -> *mut thunderstrike_config;
    pub fn thunderstrike_config_free(cfg: *mut thunderstrike_config);
    pub fn thunderstrike_config_add_endpoint(cfg: *mut thunderstrike_config, addr: *const c_char) -> i32;
    pub fn thunderstrike_config_set_cluster(
        cfg: *mut thunderstrike_config,
        host: *const c_char,
        base_port: u16,
        shards: u16,
        cores: u16,
    ) -> i32;
    pub fn thunderstrike_config_set_connections_per_endpoint(cfg: *mut thunderstrike_config, n: u32) -> i32;
    pub fn thunderstrike_config_set_max_inflight(cfg: *mut thunderstrike_config, n: u32) -> i32;
    pub fn thunderstrike_config_set_max_buffered_bytes(cfg: *mut thunderstrike_config, n: u64) -> i32;
    pub fn thunderstrike_config_set_max_response_bytes(cfg: *mut thunderstrike_config, n: u64) -> i32;
    pub fn thunderstrike_config_set_request_timeout_ms(cfg: *mut thunderstrike_config, ms: u64) -> i32;
    pub fn thunderstrike_config_set_timeout_per_mib_ms(cfg: *mut thunderstrike_config, ms: u64) -> i32;
    pub fn thunderstrike_config_set_connect_timeout_ms(cfg: *mut thunderstrike_config, ms: u64) -> i32;
    pub fn thunderstrike_config_set_tcp_keepalive_ms(cfg: *mut thunderstrike_config, ms: u64) -> i32;
    pub fn thunderstrike_config_set_tcp_nodelay(cfg: *mut thunderstrike_config, on: i32) -> i32;
    pub fn thunderstrike_config_set_max_retries(cfg: *mut thunderstrike_config, n: u32) -> i32;
    pub fn thunderstrike_config_set_busy_retries(cfg: *mut thunderstrike_config, n: u32) -> i32;
    pub fn thunderstrike_config_set_multiplexing(cfg: *mut thunderstrike_config, on: i32) -> i32;
    pub fn thunderstrike_config_set_mux_chunk_bytes(cfg: *mut thunderstrike_config, n: u64) -> i32;
    pub fn thunderstrike_config_set_mux_max_open_bodies(cfg: *mut thunderstrike_config, n: u32) -> i32;
    pub fn thunderstrike_config_set_busy_backoff_ms(cfg: *mut thunderstrike_config, min_ms: u64, max_ms: u64) -> i32;
    pub fn thunderstrike_config_set_busy_retry_window_ms(cfg: *mut thunderstrike_config, ms: u64) -> i32;
    pub fn thunderstrike_config_set_routing(cfg: *mut thunderstrike_config, routing: i32) -> i32;
    pub fn thunderstrike_config_set_worker_threads(cfg: *mut thunderstrike_config, n: u32) -> i32;
    pub fn thunderstrike_config_set_tls(
        cfg: *mut thunderstrike_config,
        server_name: *const c_char,
        ca_pem: *const u8,
        ca_pem_len: usize,
    ) -> i32;
    pub fn thunderstrike_config_set_client_identity(
        cfg: *mut thunderstrike_config,
        cert_pem: *const u8,
        cert_pem_len: usize,
        key_pem: *const u8,
        key_pem_len: usize,
    ) -> i32;
    pub fn thunderstrike_config_set_insecure_plaintext(cfg: *mut thunderstrike_config, on: i32) -> i32;
    pub fn thunderstrike_config_set_min_io_threads(cfg: *mut thunderstrike_config, n: u32) -> i32;
    pub fn thunderstrike_config_set_max_io_threads(cfg: *mut thunderstrike_config, n: u32) -> i32;
    pub fn thunderstrike_config_set_io_sample_interval_ms(cfg: *mut thunderstrike_config, ms: u64) -> i32;
    pub fn thunderstrike_config_set_io_idle_timeout_ms(cfg: *mut thunderstrike_config, ms: u64) -> i32;

    // Lifecycle.
    pub fn thunderstrike_connect(cfg: *const thunderstrike_config, out: *mut *mut thunderstrike_client) -> i32;
    pub fn thunderstrike_client_free(client: *mut thunderstrike_client);

    // Blocking operations.
    pub fn thunderstrike_get(
        client: *mut thunderstrike_client,
        key: *const u8,
        key_len: usize,
        out: *mut thunderstrike_buf,
    ) -> i32;
    pub fn thunderstrike_get_into(
        client: *mut thunderstrike_client,
        key: *const u8,
        key_len: usize,
        dst: *mut u8,
        dst_cap: usize,
        out_len: *mut usize,
    ) -> i32;
    pub fn thunderstrike_set(
        client: *mut thunderstrike_client,
        key: *const u8,
        key_len: usize,
        value: *const u8,
        value_len: usize,
    ) -> i32;
    pub fn thunderstrike_delete(client: *mut thunderstrike_client, key: *const u8, key_len: usize) -> i32;
    pub fn thunderstrike_exists(client: *mut thunderstrike_client, key: *const u8, key_len: usize) -> i32;
    pub fn thunderstrike_append(
        client: *mut thunderstrike_client,
        key: *const u8,
        key_len: usize,
        value: *const u8,
        value_len: usize,
        out_total: *mut u64,
    ) -> i32;
    pub fn thunderstrike_ping(client: *mut thunderstrike_client) -> i32;

    // Pipelined batches.
    pub fn thunderstrike_mget(
        client: *mut thunderstrike_client,
        keys: *const *const u8,
        key_lens: *const usize,
        n: usize,
        out_values: *mut thunderstrike_buf,
        out_codes: *mut i32,
    ) -> i32;
    pub fn thunderstrike_mset(
        client: *mut thunderstrike_client,
        keys: *const *const u8,
        key_lens: *const usize,
        values: *const *const u8,
        value_lens: *const usize,
        n: usize,
        out_codes: *mut i32,
    ) -> i32;
    pub fn thunderstrike_mdelete(
        client: *mut thunderstrike_client,
        keys: *const *const u8,
        key_lens: *const usize,
        n: usize,
        out_codes: *mut i32,
    ) -> i32;

    // Callback operations.
    pub fn thunderstrike_get_async(
        client: *mut thunderstrike_client,
        key: *const u8,
        key_len: usize,
        cb: thunderstrike_callback,
        user_data: *mut c_void,
    ) -> i32;
    pub fn thunderstrike_set_async(
        client: *mut thunderstrike_client,
        key: *const u8,
        key_len: usize,
        value: *const u8,
        value_len: usize,
        cb: thunderstrike_callback,
        user_data: *mut c_void,
    ) -> i32;
    pub fn thunderstrike_delete_async(
        client: *mut thunderstrike_client,
        key: *const u8,
        key_len: usize,
        cb: thunderstrike_callback,
        user_data: *mut c_void,
    ) -> i32;

    // Buffers, introspection, diagnostics.
    pub fn thunderstrike_buf_free(buf: *mut thunderstrike_buf);
    pub fn thunderstrike_get_stats(client: *mut thunderstrike_client, out: *mut thunderstrike_stats) -> i32;
    pub fn thunderstrike_get_mux_stats(
        client: *mut thunderstrike_client,
        out_mux_connections: *mut u64,
        out_chunked_requests: *mut u64,
        out_chunked_replies: *mut u64,
    ) -> i32;
    pub fn thunderstrike_shard_of(
        client: *mut thunderstrike_client,
        key: *const u8,
        key_len: usize,
        out_shard: *mut u16,
    ) -> i32;
    pub fn thunderstrike_hash_shard(key: *const u8, key_len: usize, shards: u16, out_shard: *mut u16) -> i32;
    pub fn thunderstrike_shard_count(client: *mut thunderstrike_client, out_shards: *mut u16) -> i32;
    pub fn thunderstrike_endpoint_count(client: *mut thunderstrike_client, out_endpoints: *mut u32) -> i32;
    pub fn thunderstrike_strerror(code: i32) -> *const c_char;
    pub fn thunderstrike_retry_class(code: i32) -> i32;
    pub fn thunderstrike_last_error_message() -> *const c_char;
    pub fn thunderstrike_version() -> *const c_char;
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, offset_of, size_of};

    // What the header's C layout implies on every supported (LP64/LLP64,
    // 64-bit) target: no padding anywhere, pointer-sized fields.
    #[test]
    fn buf_layout_matches_header() {
        let p = size_of::<*const u8>();
        assert_eq!(size_of::<thunderstrike_buf>(), 3 * p);
        assert_eq!(align_of::<thunderstrike_buf>(), p);
        assert_eq!(offset_of!(thunderstrike_buf, data), 0);
        assert_eq!(offset_of!(thunderstrike_buf, len), p);
        assert_eq!(offset_of!(thunderstrike_buf, _owner), 2 * p);
    }

    #[test]
    fn stats_layout_matches_header() {
        assert_eq!(size_of::<thunderstrike_stats>(), 17 * 8);
        assert_eq!(align_of::<thunderstrike_stats>(), 8);
        let fields = [
            offset_of!(thunderstrike_stats, connections),
            offset_of!(thunderstrike_stats, connections_ready),
            offset_of!(thunderstrike_stats, inflight),
            offset_of!(thunderstrike_stats, capacity),
            offset_of!(thunderstrike_stats, requests),
            offset_of!(thunderstrike_stats, responses),
            offset_of!(thunderstrike_stats, bytes_written),
            offset_of!(thunderstrike_stats, bytes_read),
            offset_of!(thunderstrike_stats, write_batches),
            offset_of!(thunderstrike_stats, reconnects),
            offset_of!(thunderstrike_stats, orphan_responses),
            offset_of!(thunderstrike_stats, io_threads),
            offset_of!(thunderstrike_stats, io_threads_min),
            offset_of!(thunderstrike_stats, io_threads_max),
            offset_of!(thunderstrike_stats, io_grew),
            offset_of!(thunderstrike_stats, io_shrank),
            offset_of!(thunderstrike_stats, io_moved),
        ];
        for (i, off) in fields.into_iter().enumerate() {
            assert_eq!(off, i * 8, "field {i} of thunderstrike_stats");
        }
    }

    #[test]
    fn callback_is_one_nullable_pointer() {
        assert_eq!(size_of::<thunderstrike_callback>(), size_of::<*const u8>());
    }
}
