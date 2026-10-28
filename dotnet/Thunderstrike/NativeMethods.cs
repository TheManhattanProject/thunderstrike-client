// P/Invoke declarations for the thunderstrike client's C ABI.
//
// Mirrors include/thunderstrike_client.h. Two
// things here are not cosmetic:
//
//   * CallingConvention.Cdecl. On x86-64 there is only one convention
//     so it makes no difference, but this library also builds for
//     32-bit Windows, where the default (StdCall) would corrupt the
//     stack on every call.
//
//   * UIntPtr for size_t. `int`/`uint` would silently truncate a
//     length on 64-bit.

using System;
using System.Runtime.InteropServices;

namespace Thunderstrike;

/// <summary>An owned block of bytes returned by the native library.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct NativeBuf
{
    public IntPtr Data;
    public UIntPtr Len;
    public IntPtr Owner; // opaque
}

/// <summary>Pool counters, laid out as <c>thunderstrike_stats</c>.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct NativeStats
{
    public ulong Connections;
    public ulong ConnectionsReady;
    public ulong Inflight;
    public ulong Capacity;
    public ulong Requests;
    public ulong Responses;
    public ulong BytesWritten;
    public ulong BytesRead;
    public ulong WriteBatches;
    public ulong Reconnects;
    public ulong OrphanResponses;

    // The I/O pool. Present even if unread: the native side writes the
    // whole struct, so a short one is an out-of-bounds write.
    public ulong IOThreads;
    public ulong IOThreadsMin;
    public ulong IOThreadsMax;
    public ulong IOGrew;
    public ulong IOShrank;
    public ulong IOMoved;
}

/// <summary>
/// The native callback. Must be kept alive for as long as native code
/// can invoke it — see <see cref="ThunderstrikeClient"/>, which holds a
/// single static instance rather than creating one per call.
/// </summary>
[UnmanagedFunctionPointer(CallingConvention.Cdecl)]
internal delegate void NativeCallback(IntPtr userData, int code, IntPtr data, UIntPtr len);

internal static class NativeMethods
{
    /// <summary>
    /// The runtime resolves this to <c>thunderstrike_client.dll</c>,
    /// <c>libthunderstrike_client.so</c> or <c>libthunderstrike_client.dylib</c>
    /// depending on the platform.
    /// </summary>
    internal const string Lib = "thunderstrike_client";

    private const CallingConvention Cdecl = CallingConvention.Cdecl;

    // ── Configuration ──────────────────────────────────────────────

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern IntPtr thunderstrike_config_new();

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern void thunderstrike_config_free(IntPtr cfg);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_add_endpoint(IntPtr cfg, byte[] addrUtf8);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_cluster(
        IntPtr cfg, byte[] hostUtf8, ushort basePort, ushort shards, ushort cores);

    [DllImport(Lib, CallingConvention = Cdecl)]
    // `serverName` is a NUL-terminated UTF-8 byte[] rather than a
    // marshalled string: `UnmanagedType.LPUTF8Str` does not exist in
    // netstandard2.0, which this targets alongside net8.0.
    internal static extern int thunderstrike_config_set_tls(
        IntPtr cfg,
        byte[] serverNameUtf8,
        byte[] caPem,
        UIntPtr caPemLen);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_client_identity(
        IntPtr cfg,
        byte[] certPem,
        UIntPtr certPemLen,
        byte[] keyPem,
        UIntPtr keyPemLen);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_insecure_plaintext(IntPtr cfg, int on);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_busy_retries(IntPtr cfg, uint n);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_multiplexing(IntPtr cfg, int on);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_mux_chunk_bytes(IntPtr cfg, ulong n);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_busy_backoff_ms(IntPtr cfg, ulong minMs, ulong maxMs);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_busy_retry_window_ms(IntPtr cfg, ulong ms);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_timeout_per_mib_ms(IntPtr cfg, ulong ms);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_retry_class(int code);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_connections_per_endpoint(IntPtr cfg, uint n);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_max_inflight(IntPtr cfg, uint n);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_max_buffered_bytes(IntPtr cfg, ulong n);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_max_response_bytes(IntPtr cfg, ulong n);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_request_timeout_ms(IntPtr cfg, ulong ms);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_connect_timeout_ms(IntPtr cfg, ulong ms);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_tcp_keepalive_ms(IntPtr cfg, ulong ms);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_tcp_nodelay(IntPtr cfg, int on);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_max_retries(IntPtr cfg, uint n);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_routing(IntPtr cfg, int routing);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_config_set_worker_threads(IntPtr cfg, uint n);

    // ── Lifecycle ──────────────────────────────────────────────────

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_connect(IntPtr cfg, out IntPtr client);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern void thunderstrike_client_free(IntPtr client);

    // ── Blocking operations ────────────────────────────────────────

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_get(
        IntPtr client, byte[] key, UIntPtr keyLen, out NativeBuf value);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_get_into(
        IntPtr client, byte[] key, UIntPtr keyLen,
        byte[] dst, UIntPtr dstCap, out UIntPtr outLen);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_set_ttl(
        IntPtr client, byte[] key, UIntPtr keyLen,
        byte[] value, UIntPtr valueLen, uint ttlMs);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_delete(IntPtr client, byte[] key, UIntPtr keyLen);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_exists(IntPtr client, byte[] key, UIntPtr keyLen);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_append(
        IntPtr client, byte[] key, UIntPtr keyLen,
        byte[] value, UIntPtr valueLen, out ulong outTotal);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_ping(IntPtr client);

    // ── Batches ────────────────────────────────────────────────────

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_mget(
        IntPtr client, IntPtr[] keys, UIntPtr[] keyLens, UIntPtr n,
        [Out] NativeBuf[] outValues, [Out] int[] outCodes);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_mset(
        IntPtr client, IntPtr[] keys, UIntPtr[] keyLens,
        IntPtr[] values, UIntPtr[] valueLens, UIntPtr n, [Out] int[] outCodes);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_mdelete(
        IntPtr client, IntPtr[] keys, UIntPtr[] keyLens, UIntPtr n, [Out] int[] outCodes);

    // ── Callback operations ────────────────────────────────────────

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_get_async(
        IntPtr client, byte[] key, UIntPtr keyLen, NativeCallback cb, IntPtr userData);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_set_async(
        IntPtr client, byte[] key, UIntPtr keyLen,
        byte[] value, UIntPtr valueLen, NativeCallback cb, IntPtr userData);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_delete_async(
        IntPtr client, byte[] key, UIntPtr keyLen, NativeCallback cb, IntPtr userData);

    // ── Buffers and diagnostics ────────────────────────────────────

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern void thunderstrike_buf_free(ref NativeBuf buf);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_get_stats(IntPtr client, out NativeStats stats);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_get_mux_stats(
        IntPtr client, out ulong muxConnections, out ulong chunkedRequests, out ulong chunkedReplies);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_shard_of(
        IntPtr client, byte[] key, UIntPtr keyLen, out ushort outShard);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_hash_shard(
        byte[] key, UIntPtr keyLen, ushort shards, out ushort outShard);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_shard_count(IntPtr client, out ushort outShards);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern int thunderstrike_endpoint_count(IntPtr client, out uint outEndpoints);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern IntPtr thunderstrike_strerror(int code);

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern IntPtr thunderstrike_last_error_message();

    [DllImport(Lib, CallingConvention = Cdecl)]
    internal static extern IntPtr thunderstrike_version();
}
