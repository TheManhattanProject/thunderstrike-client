using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace Thunderstrike;

/// <summary>
/// A pooled, pipelined client for the thunderstrike key-value database.
/// </summary>
/// <remarks>
/// <para>
/// Thread-safe: share one instance across the whole process. Every
/// member except <see cref="Dispose"/> may be called concurrently.
/// </para>
/// <para>
/// The synchronous members block the calling thread on the native
/// runtime. The <c>*Async</c> members do not — they hand the request
/// to the native runtime and complete a <see cref="Task"/> when the
/// answer arrives, so they never occupy a thread-pool thread while
/// waiting. Prefer them in ASP.NET and anywhere else with a limited
/// thread pool.
/// </para>
/// <para>
/// For more than one key, prefer <see cref="GetMany"/> /
/// <see cref="SetMany(IReadOnlyList{byte[]}, IReadOnlyList{byte[]})"/> /
/// <see cref="DeleteMany"/>: they put the
/// whole batch on the wire before awaiting any answer, so n keys cost
/// one round trip instead of n.
/// </para>
/// </remarks>
public sealed class ThunderstrikeClient : IDisposable
{
    private IntPtr _handle;
    private int _disposed;

    /// <summary>
    /// One delegate instance for the process, rooted in a static
    /// field.
    /// </summary>
    /// <remarks>
    /// This is the classic P/Invoke callback bug: a delegate created
    /// per call is collectable as soon as the managed call returns,
    /// and the native side then jumps into freed memory when the
    /// answer arrives. Keeping exactly one rooted instance and
    /// passing the per-request state through <c>user_data</c> avoids
    /// it entirely, and allocates nothing per call.
    /// </remarks>
    private static readonly NativeCallback Trampoline = OnNativeCompletion;

    static ThunderstrikeClient()
    {
#if NETCOREAPP3_0_OR_GREATER
        // Runs before any member of this type is touched, which is
        // every entry point into the library.
        NativeLibraryResolver.EnsureInstalled();
#endif
    }

    // ── Construction ───────────────────────────────────────────────

    /// <summary>Connect to a single endpoint with default settings.</summary>
    public ThunderstrikeClient(string endpoint)
        : this(new ThunderstrikeOptions().AddEndpoint(endpoint))
    {
    }

    /// <summary>Connect with explicit settings.</summary>
    /// <remarks>
    /// Every connection in the pool is established (and
    /// authenticated) before the constructor returns, so a wrong
    /// address or a bad token throws here rather than on first use.
    /// </remarks>
    public ThunderstrikeClient(ThunderstrikeOptions options)
    {
        if (options is null) throw new ArgumentNullException(nameof(options));
        if (options.Endpoints.Count == 0 && options.Host is null)
        {
            throw new ArgumentException(
                "set Endpoints, or Host/BasePort/Shards/Cores", nameof(options));
        }

        var cfg = NativeMethods.thunderstrike_config_new();
        if (cfg == IntPtr.Zero) throw new OutOfMemoryException("thunderstrike_config_new");

        try
        {
            foreach (var addr in options.Endpoints)
            {
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_add_endpoint(cfg, Utf8Z(addr)),
                    "AddEndpoint");
            }
            if (options.Host is not null)
            {
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_cluster(
                        cfg, Utf8Z(options.Host), options.BasePort,
                        options.Shards, options.Cores),
                    "SetCluster");
            }
            if (options.TlsServerName is not null || options.TlsCaPem is not null)
            {
                if (options.TlsServerName is null || options.TlsCaPem is null)
                {
                    throw new ArgumentException(
                        "TlsServerName and TlsCaPem must be set together — a trust " +
                        "anchor with no name to check it against verifies nothing.");
                }
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_tls(
                        cfg, Utf8Z(options.TlsServerName), options.TlsCaPem,
                        (UIntPtr)options.TlsCaPem.Length),
                    "Tls");
            }

            if (options.TlsClientCertPem is not null || options.TlsClientKeyPem is not null)
            {
                if (options.TlsClientCertPem is null || options.TlsClientKeyPem is null)
                {
                    throw new ArgumentException(
                        "TlsClientCertPem and TlsClientKeyPem must be set together.");
                }
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_client_identity(
                        cfg, options.TlsClientCertPem, (UIntPtr)options.TlsClientCertPem.Length,
                        options.TlsClientKeyPem, (UIntPtr)options.TlsClientKeyPem.Length),
                    "ClientIdentity");
            }
            if (options.InsecurePlaintext)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_insecure_plaintext(cfg, 1), "InsecurePlaintext");
            if (options.BusyRetries is { } br)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_busy_retries(cfg, br), "BusyRetries");
            if (options.BusyBackoffMin is { } bmin && options.BusyBackoffMax is { } bmax)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_busy_backoff_ms(cfg, Millis(bmin), Millis(bmax)),
                    "BusyBackoff");
            if (options.BusyRetryWindow is { } bw)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_busy_retry_window_ms(cfg, Millis(bw)),
                    "BusyRetryWindow");
            if (options.TimeoutPerMib is { } tpm)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_timeout_per_mib_ms(cfg, Millis(tpm)),
                    "TimeoutPerMib");

            if (options.ConnectionsPerEndpoint is { } cpe)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_connections_per_endpoint(cfg, cpe),
                    "ConnectionsPerEndpoint");
            if (options.MaxInflight is { } mi)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_max_inflight(cfg, mi), "MaxInflight");
            if (options.MaxBufferedBytes is { } mbb)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_max_buffered_bytes(cfg, mbb),
                    "MaxBufferedBytes");
            if (options.MaxResponseBytes is { } mrb)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_max_response_bytes(cfg, mrb),
                    "MaxResponseBytes");
            if (options.RequestTimeout is { } rt)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_request_timeout_ms(cfg, Millis(rt)),
                    "RequestTimeout");
            if (options.ConnectTimeout is { } ct)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_connect_timeout_ms(cfg, Millis(ct)),
                    "ConnectTimeout");
            if (options.TcpKeepalive is { } ka)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_tcp_keepalive_ms(cfg, Millis(ka)),
                    "TcpKeepalive");
            if (options.TcpNoDelay is { } nd)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_tcp_nodelay(cfg, nd ? 1 : 0), "TcpNoDelay");
            if (options.Multiplexing is { } mux)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_multiplexing(cfg, mux ? 1 : 0), "Multiplexing");
            if (options.MuxChunkBytes is { } chunk)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_mux_chunk_bytes(cfg, chunk), "MuxChunkBytes");
            if (options.MaxRetries is { } mr)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_max_retries(cfg, mr), "MaxRetries");
            if (options.Routing is { } routing)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_routing(cfg, (int)routing), "Routing");
            if (options.WorkerThreads is { } wt)
                ThunderstrikeException.ThrowIfError(
                    NativeMethods.thunderstrike_config_set_worker_threads(cfg, wt), "WorkerThreads");

            ThunderstrikeException.ThrowIfError(
                NativeMethods.thunderstrike_connect(cfg, out _handle), "Connect");
        }
        finally
        {
            NativeMethods.thunderstrike_config_free(cfg);
        }
    }

    /// <summary>Shut the pool down. Idempotent.</summary>
    public void Dispose()
    {
        // Interlocked, not a bool: Dispose racing itself would
        // otherwise free the same handle twice.
        if (Interlocked.Exchange(ref _disposed, 1) != 0) return;
        var h = Interlocked.Exchange(ref _handle, IntPtr.Zero);
        if (h != IntPtr.Zero) NativeMethods.thunderstrike_client_free(h);
        GC.SuppressFinalize(this);
    }

    /// <summary>
    /// Releases the native pool if <see cref="Dispose"/> was never
    /// called. The only resource held is the native handle, so
    /// freeing it from the finalizer thread is safe.
    /// </summary>
    ~ThunderstrikeClient() => Dispose();

    private IntPtr Handle
    {
        get
        {
            var h = _handle;
            if (h == IntPtr.Zero) throw new ThunderstrikeClosedException();
            return h;
        }
    }

    // ── Single keys ────────────────────────────────────────────────

    /// <summary>Read a key. <see langword="null"/> is a clean miss.</summary>
    public byte[]? Get(byte[] key)
    {
        CheckKey(key);
        var code = NativeMethods.thunderstrike_get(Handle, key, (UIntPtr)key.Length, out var buf);
        try
        {
            return code switch
            {
                ThunderstrikeCode.Ok => Copy(buf),
                ThunderstrikeCode.NotFound => null,
                _ => throw ThunderstrikeException.Create(code, "Get"),
            };
        }
        finally
        {
            // Always: the buffer is a hold on the client's read
            // buffer, and it is initialised even on a miss.
            NativeMethods.thunderstrike_buf_free(ref buf);
        }
    }

    /// <summary>Read a key as UTF-8 text.</summary>
    public string? GetString(string key)
    {
        var v = Get(Encoding.UTF8.GetBytes(key));
        return v is null ? null : Encoding.UTF8.GetString(v);
    }

    /// <summary>
    /// Read a key straight into <paramref name="destination"/> — no
    /// allocation.
    /// </summary>
    /// <param name="key">The key to read.</param>
    /// <param name="destination">Where to copy the value.</param>
    /// <param name="length">
    /// The value's true length, set whether or not it fit. When the
    /// result is <see langword="false"/> because the buffer was too
    /// small, this is how large a buffer to bring back.
    /// </param>
    /// <returns>
    /// <see langword="true"/> when the value was copied;
    /// <see langword="false"/> on a miss (with
    /// <paramref name="length"/> zero) or when the buffer was too
    /// small.
    /// </returns>
    public bool TryGetInto(byte[] key, byte[] destination, out int length)
    {
        CheckKey(key);
        if (destination is null) throw new ArgumentNullException(nameof(destination));

        var code = NativeMethods.thunderstrike_get_into(
            Handle, key, (UIntPtr)key.Length,
            destination, (UIntPtr)destination.Length, out var len);
        length = checked((int)len);

        return code switch
        {
            ThunderstrikeCode.Ok => true,
            ThunderstrikeCode.NotFound => false,
            ThunderstrikeCode.BufferTooSmall => false,
            _ => throw ThunderstrikeException.Create(code, "TryGetInto"),
        };
    }

    /// <summary>Write a key.</summary>
    /// <param name="key">The key to write.</param>
    /// <param name="value">The value to store.</param>
    public void Set(byte[] key, byte[] value)
    {
        CheckKey(key);
        if (value is null) throw new ArgumentNullException(nameof(value));
        ThunderstrikeException.ThrowIfError(
            NativeMethods.thunderstrike_set(
                Handle, key, (UIntPtr)key.Length, value, (UIntPtr)value.Length),
            "Set");
    }

    /// <summary>Write a key from UTF-8 text.</summary>
    public void SetString(string key, string value) =>
        Set(Encoding.UTF8.GetBytes(key), Encoding.UTF8.GetBytes(value));

    /// <summary>Delete a key. <see langword="false"/> if it was absent.</summary>
    public bool Delete(byte[] key)
    {
        CheckKey(key);
        var code = NativeMethods.thunderstrike_delete(Handle, key, (UIntPtr)key.Length);
        return code switch
        {
            ThunderstrikeCode.Ok => true,
            ThunderstrikeCode.NotFound => false,
            _ => throw ThunderstrikeException.Create(code, "Delete"),
        };
    }

    /// <summary>Test for a key without transferring its value.</summary>
    public bool Exists(byte[] key)
    {
        CheckKey(key);
        var code = NativeMethods.thunderstrike_exists(Handle, key, (UIntPtr)key.Length);
        return code switch
        {
            ThunderstrikeCode.Ok => true,
            ThunderstrikeCode.NotFound => false,
            _ => throw ThunderstrikeException.Create(code, "Exists"),
        };
    }

    /// <summary>
    /// Append to a key's value, creating it when absent. Returns the
    /// value's total length afterwards.
    /// </summary>
    public ulong Append(byte[] key, byte[] value)
    {
        CheckKey(key);
        if (value is null) throw new ArgumentNullException(nameof(value));
        ThunderstrikeException.ThrowIfError(
            NativeMethods.thunderstrike_append(
                Handle, key, (UIntPtr)key.Length, value, (UIntPtr)value.Length, out var total),
            "Append");
        return total;
    }

    /// <summary>Round-trip a liveness probe.</summary>
    public void Ping() => ThunderstrikeException.ThrowIfError(
        NativeMethods.thunderstrike_ping(Handle), "Ping");

    // ── Batches ────────────────────────────────────────────────────

    /// <summary>
    /// Read many keys in ONE round trip. A <see langword="null"/>
    /// entry is a miss.
    /// </summary>
    public byte[]?[] GetMany(IReadOnlyList<byte[]> keys)
    {
        if (keys is null) throw new ArgumentNullException(nameof(keys));
        var n = keys.Count;
        if (n == 0) return Array.Empty<byte[]?>();

        var pinned = new Pinned(keys);
        var bufs = new NativeBuf[n];
        var codes = new int[n];
        try
        {
            var rc = NativeMethods.thunderstrike_mget(
                Handle, pinned.Pointers, pinned.Lengths, (UIntPtr)n, bufs, codes);
            ThunderstrikeException.ThrowIfError(rc, "GetMany");

            var result = new byte[]?[n];
            for (var i = 0; i < n; i++)
            {
                result[i] = codes[i] switch
                {
                    ThunderstrikeCode.Ok => Copy(bufs[i]),
                    ThunderstrikeCode.NotFound => null,
                    _ => throw ThunderstrikeException.Create(codes[i], $"GetMany[{i}]"),
                };
            }
            return result;
        }
        finally
        {
            for (var i = 0; i < n; i++) NativeMethods.thunderstrike_buf_free(ref bufs[i]);
            pinned.Dispose();
        }
    }

    /// <summary>Write many pairs in ONE round trip.</summary>
    public void SetMany(IReadOnlyList<byte[]> keys, IReadOnlyList<byte[]> values)
    {
        if (keys is null) throw new ArgumentNullException(nameof(keys));
        if (values is null) throw new ArgumentNullException(nameof(values));
        if (keys.Count != values.Count)
        {
            throw new ArgumentException("as many values as keys", nameof(values));
        }
        var n = keys.Count;
        if (n == 0) return;

        var pk = new Pinned(keys);
        var pv = new Pinned(values);
        try
        {
            var codes = new int[n];
            ThunderstrikeException.ThrowIfError(
                NativeMethods.thunderstrike_mset(
                    Handle, pk.Pointers, pk.Lengths, pv.Pointers, pv.Lengths, (UIntPtr)n, codes),
                "SetMany");
            for (var i = 0; i < n; i++)
            {
                if (codes[i] != ThunderstrikeCode.Ok)
                {
                    throw ThunderstrikeException.Create(codes[i], $"SetMany[{i}]");
                }
            }
        }
        finally
        {
            pk.Dispose();
            pv.Dispose();
        }
    }

    /// <summary>
    /// Write many pairs in ONE round trip, from a dictionary.
    /// </summary>
    public void SetMany(IReadOnlyDictionary<string, string> items)
    {
        if (items is null) throw new ArgumentNullException(nameof(items));
        var keys = new List<byte[]>(items.Count);
        var values = new List<byte[]>(items.Count);
        foreach (var kv in items)
        {
            keys.Add(Encoding.UTF8.GetBytes(kv.Key));
            values.Add(Encoding.UTF8.GetBytes(kv.Value));
        }
        SetMany(keys, values);
    }

    /// <summary>Delete many keys in ONE round trip.</summary>
    public bool[] DeleteMany(IReadOnlyList<byte[]> keys)
    {
        if (keys is null) throw new ArgumentNullException(nameof(keys));
        var n = keys.Count;
        if (n == 0) return Array.Empty<bool>();

        var pinned = new Pinned(keys);
        try
        {
            var codes = new int[n];
            ThunderstrikeException.ThrowIfError(
                NativeMethods.thunderstrike_mdelete(
                    Handle, pinned.Pointers, pinned.Lengths, (UIntPtr)n, codes),
                "DeleteMany");

            var result = new bool[n];
            for (var i = 0; i < n; i++)
            {
                result[i] = codes[i] switch
                {
                    ThunderstrikeCode.Ok => true,
                    ThunderstrikeCode.NotFound => false,
                    _ => throw ThunderstrikeException.Create(codes[i], $"DeleteMany[{i}]"),
                };
            }
            return result;
        }
        finally
        {
            pinned.Dispose();
        }
    }

    // ── Async ──────────────────────────────────────────────────────

    /// <summary>
    /// Read a key without occupying a thread while waiting.
    /// <see langword="null"/> is a clean miss.
    /// </summary>
    /// <remarks>
    /// Cancelling detaches this task from the request; it does not
    /// cancel the request, because thunderstrike has no cancel opcode.
    /// </remarks>
    public Task<byte[]?> GetAsync(byte[] key, CancellationToken cancellationToken = default)
    {
        CheckKey(key);
        var state = new Completion<byte[]?>(cancellationToken);
        var rc = NativeMethods.thunderstrike_get_async(
            Handle, key, (UIntPtr)key.Length, Trampoline, state.ToPointer(OnGet));
        return state.Start(rc, "GetAsync");
    }

    /// <summary>Read a key as UTF-8 text, asynchronously.</summary>
    public async Task<string?> GetStringAsync(string key, CancellationToken cancellationToken = default)
    {
        var v = await GetAsync(Encoding.UTF8.GetBytes(key), cancellationToken).ConfigureAwait(false);
        return v is null ? null : Encoding.UTF8.GetString(v);
    }

    /// <summary>Write a key without occupying a thread while waiting.</summary>
    public Task SetAsync(byte[] key, byte[] value, CancellationToken cancellationToken = default)
    {
        CheckKey(key);
        if (value is null) throw new ArgumentNullException(nameof(value));
        var state = new Completion<bool>(cancellationToken);
        var rc = NativeMethods.thunderstrike_set_async(
            Handle, key, (UIntPtr)key.Length, value, (UIntPtr)value.Length,
            Trampoline, state.ToPointer(OnUnit));
        return state.Start(rc, "SetAsync");
    }

    /// <summary>Delete a key without occupying a thread while waiting.</summary>
    public Task<bool> DeleteAsync(byte[] key, CancellationToken cancellationToken = default)
    {
        CheckKey(key);
        var state = new Completion<bool>(cancellationToken);
        var rc = NativeMethods.thunderstrike_delete_async(
            Handle, key, (UIntPtr)key.Length, Trampoline, state.ToPointer(OnFound));
        return state.Start(rc, "DeleteAsync");
    }

    // ── Introspection ──────────────────────────────────────────────

    /// <summary>The pool's counters.</summary>
    public ThunderstrikeStats GetStats()
    {
        ThunderstrikeException.ThrowIfError(
            NativeMethods.thunderstrike_get_stats(Handle, out var s), "GetStats");
        return new ThunderstrikeStats(in s);
    }

    /// <summary>
    /// Frame multiplexing counters: connections multiplexed and up now,
    /// requests sent in chunks, replies received in chunks.
    /// </summary>
    public (ulong Connections, ulong ChunkedRequests, ulong ChunkedReplies) GetMuxStats()
    {
        ThunderstrikeException.ThrowIfError(
            NativeMethods.thunderstrike_get_mux_stats(Handle, out var c, out var q, out var p), "GetMuxStats");
        return (c, q, p);
    }

    /// <summary>
    /// Which shard this client routes <paramref name="key"/> to, under
    /// the routing table it read from the server.
    /// </summary>
    public ushort ShardOf(byte[] key)
    {
        CheckKey(key);
        ThunderstrikeException.ThrowIfError(
            NativeMethods.thunderstrike_shard_of(Handle, key, (UIntPtr)key.Length, out var shard),
            "ShardOf");
        return shard;
    }

    /// <summary>
    /// Which of <paramref name="shards"/> shards owns a key:
    /// <c>CRC32C(key) % shards</c>, the same function the server
    /// places data with. Needs no client.
    /// </summary>
    public static ushort ShardOf(byte[] key, ushort shards)
    {
        if (key is null) throw new ArgumentNullException(nameof(key));
        ThunderstrikeException.ThrowIfError(
            NativeMethods.thunderstrike_hash_shard(key, (UIntPtr)key.Length, shards, out var shard),
            "ShardOf");
        return shard;
    }

    /// <summary>
    /// How many shards the deployment has, as the server stated it.
    /// 0 under round-robin routing, which has no shards.
    /// </summary>
    public ushort Shards
    {
        get
        {
            ThunderstrikeException.ThrowIfError(
                NativeMethods.thunderstrike_shard_count(Handle, out var n), "Shards");
            return n;
        }
    }

    /// <summary>
    /// How many endpoints this client holds — one per server core,
    /// discovered rather than configured.
    /// </summary>
    public uint Endpoints
    {
        get
        {
            ThunderstrikeException.ThrowIfError(
                NativeMethods.thunderstrike_endpoint_count(Handle, out var n), "Endpoints");
            return n;
        }
    }

    /// <summary>The native library's version.</summary>
    public static string Version =>
        Marshal.PtrToStringAnsi(NativeMethods.thunderstrike_version()) ?? "unknown";

    // ── Plumbing ───────────────────────────────────────────────────

    private static void CheckKey(byte[] key)
    {
        if (key is null) throw new ArgumentNullException(nameof(key));
        if (key.Length > ThunderstrikeCode.MaxKeyLength)
        {
            throw new ArgumentException(
                $"key is {key.Length} bytes; the wire's key_len field is 16 bits " +
                $"(max {ThunderstrikeCode.MaxKeyLength})",
                nameof(key));
        }
    }

    private static byte[] Copy(in NativeBuf buf)
    {
        var len = (long)buf.Len;
        if (len == 0 || buf.Data == IntPtr.Zero) return Array.Empty<byte>();
        var managed = new byte[len];
        Marshal.Copy(buf.Data, managed, 0, checked((int)len));
        return managed;
    }

    private static byte[] Utf8Z(string s)
    {
        var bytes = Encoding.UTF8.GetBytes(s);
        var withNul = new byte[bytes.Length + 1];
        Buffer.BlockCopy(bytes, 0, withNul, 0, bytes.Length);
        return withNul; // the extra byte is already 0
    }

    private static ulong Millis(TimeSpan t) =>
        t <= TimeSpan.Zero ? 0UL : (ulong)t.TotalMilliseconds;

    /// <summary>
    /// Pins a list of byte arrays and exposes the parallel
    /// pointer/length arrays the batch ABI takes.
    /// </summary>
    /// <remarks>
    /// The GC may move a managed array at any time, so handing its
    /// address to native code without pinning is a use-after-move
    /// waiting to happen. These handles are released in a
    /// <c>finally</c>, after the native call has returned.
    /// </remarks>
    private sealed class Pinned : IDisposable
    {
        private readonly GCHandle[] _handles;
        public readonly IntPtr[] Pointers;
        public readonly UIntPtr[] Lengths;

        public Pinned(IReadOnlyList<byte[]> items)
        {
            var n = items.Count;
            _handles = new GCHandle[n];
            Pointers = new IntPtr[n];
            Lengths = new UIntPtr[n];
            for (var i = 0; i < n; i++)
            {
                var item = items[i] ?? throw new ArgumentNullException($"items[{i}]");
                _handles[i] = GCHandle.Alloc(item, GCHandleType.Pinned);
                // A zero-length array has no element to address; the
                // ABI reads no bytes for length 0.
                Pointers[i] = item.Length == 0
                    ? IntPtr.Zero
                    : _handles[i].AddrOfPinnedObject();
                Lengths[i] = (UIntPtr)item.Length;
            }
        }

        public void Dispose()
        {
            foreach (var h in _handles)
            {
                if (h.IsAllocated) h.Free();
            }
        }
    }

    // ── Callback plumbing ──────────────────────────────────────────

    /// <summary>What a completed native callback should produce.</summary>
    private delegate void Interpret(object completion, int code, IntPtr data, int len);

    private static readonly Interpret OnGet = static (c, code, data, len) =>
    {
        var tcs = (Completion<byte[]?>)c;
        switch (code)
        {
            case ThunderstrikeCode.Ok:
                var managed = new byte[len];
                if (len > 0) Marshal.Copy(data, managed, 0, len);
                tcs.Succeed(managed);
                break;
            case ThunderstrikeCode.NotFound:
                tcs.Succeed(null);
                break;
            default:
                tcs.Fail(code, "GetAsync");
                break;
        }
    };

    private static readonly Interpret OnUnit = static (c, code, _, _) =>
    {
        var tcs = (Completion<bool>)c;
        if (code == ThunderstrikeCode.Ok) tcs.Succeed(true);
        else tcs.Fail(code, "SetAsync");
    };

    private static readonly Interpret OnFound = static (c, code, _, _) =>
    {
        var tcs = (Completion<bool>)c;
        switch (code)
        {
            case ThunderstrikeCode.Ok: tcs.Succeed(true); break;
            case ThunderstrikeCode.NotFound: tcs.Succeed(false); break;
            default: tcs.Fail(code, "DeleteAsync"); break;
        }
    };

    /// <summary>
    /// The one function native code ever calls back into.
    /// </summary>
    /// <remarks>
    /// Runs on a native runtime thread. It must not throw — an
    /// exception escaping into native frames tears the process down —
    /// and it must not block, because the thread it is on is one of
    /// the client's I/O workers.
    /// </remarks>
    private static void OnNativeCompletion(IntPtr userData, int code, IntPtr data, UIntPtr len)
    {
        GCHandle handle = default;
        try
        {
            handle = GCHandle.FromIntPtr(userData);
            var completion = (ICompletion)handle.Target!;
            completion.Complete(code, data, checked((int)len));
        }
        catch
        {
            // Nothing useful to do here, and letting it propagate
            // would unwind into native code.
        }
        finally
        {
            if (handle.IsAllocated) handle.Free();
        }
    }

    private interface ICompletion
    {
        void Complete(int code, IntPtr data, int len);
    }

    /// <summary>
    /// The managed half of one in-flight async request.
    /// </summary>
    private sealed class Completion<T> : ICompletion
    {
        // RunContinuationsAsynchronously is load-bearing, not a
        // stylistic choice: without it the caller's continuation runs
        // inline on the native runtime's I/O thread, where blocking
        // or long work would stall the client for every other caller.
        private readonly TaskCompletionSource<T> _tcs =
            new(TaskCreationOptions.RunContinuationsAsynchronously);

        private readonly CancellationToken _token;
        private Interpret? _interpret;
        private CancellationTokenRegistration _registration;
        private GCHandle _self;

        public Completion(CancellationToken token) => _token = token;

        /// <summary>
        /// Root this object and hand native code a pointer to it.
        /// The handle is freed exactly once: by the callback when the
        /// request was accepted, or by <see cref="Start"/> when it
        /// was not.
        /// </summary>
        public IntPtr ToPointer(Interpret interpret)
        {
            _interpret = interpret;
            _self = GCHandle.Alloc(this);
            return GCHandle.ToIntPtr(_self);
        }

        /// <summary>
        /// Finish wiring up after submission. A submission that was
        /// refused outright never reaches the callback, so its handle
        /// has to be released here or it pins this object forever.
        /// </summary>
        public Task<T> Start(int submitCode, string operation)
        {
            if (submitCode != ThunderstrikeCode.Ok)
            {
                if (_self.IsAllocated) _self.Free();
                _tcs.TrySetException(ThunderstrikeException.Create(submitCode, operation));
                return _tcs.Task;
            }
            if (_token.CanBeCanceled)
            {
                _registration = _token.Register(
                    static s => ((Completion<T>)s!)._tcs.TrySetCanceled(), this);
            }
            return _tcs.Task;
        }

        public void Complete(int code, IntPtr data, int len) =>
            _interpret!(this, code, data, len);

        public void Succeed(T value)
        {
            _registration.Dispose();
            // Try*, not Set*: the caller may already have cancelled.
            _tcs.TrySetResult(value);
        }

        public void Fail(int code, string operation)
        {
            _registration.Dispose();
            _tcs.TrySetException(ThunderstrikeException.Create(code, operation));
        }
    }
}
