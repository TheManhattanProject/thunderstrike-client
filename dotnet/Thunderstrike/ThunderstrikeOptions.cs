using System;

namespace Thunderstrike;

/// <summary>
/// Connection and pool settings. Every nullable property means "leave
/// the client's default", so an options object only has to name what
/// it wants changed.
/// </summary>
public sealed class ThunderstrikeOptions
{
    /// <summary>
    /// Endpoints, in shard order when <see cref="Routing"/> is
    /// <see cref="ThunderstrikeRouting.ShardByKey"/>. One is enough: the
    /// client asks it for the deployment's routing table and connects
    /// to every port that names.
    /// </summary>
    public System.Collections.Generic.List<string> Endpoints { get; } = new();

    /// <summary>
    /// STATES the layout of a thunderstrike started with <c>--shards n
    /// --cores m</c>: core <c>c</c> listens on <c>BasePort + c</c> and
    /// owns a contiguous run of shards.
    /// <para>
    /// Optional, and usually unnecessary — one endpoint plus discovery
    /// finds the same thing. What this adds is a check: the stated
    /// layout is compared against the one the server serves, and the
    /// connect fails if they differ. Set all four together.
    /// </para>
    /// </summary>
    public string? Host { get; set; }

    /// <inheritdoc cref="Host"/>
    public ushort BasePort { get; set; }

    /// <inheritdoc cref="Host"/>
    public ushort Shards { get; set; }

    /// <inheritdoc cref="Host"/>
    public ushort Cores { get; set; }

    /// <summary>
    /// TLS server name — sent as SNI and matched against the
    /// certificate's subject alternative names, so it must be a name
    /// the certificate was issued for, not necessarily the host in the
    /// endpoint. Set together with <see cref="TlsCaPem"/>.
    /// </summary>
    public string? TlsServerName { get; set; }

    /// <summary>
    /// PEM trust anchor: the CA certificate (ca.crt) that issued the server's.
    /// Set together with <see cref="TlsServerName"/>. There is no way
    /// to disable verification.
    /// </summary>
    public byte[]? TlsCaPem { get; set; }

    /// <summary>TCP connections per endpoint.</summary>
    public uint? ConnectionsPerEndpoint { get; set; }

    /// <summary>Pipeline depth per connection, 1..65535.</summary>
    public uint? MaxInflight { get; set; }

    /// <summary>Encoded-but-unwritten bytes allowed per connection.</summary>
    public ulong? MaxBufferedBytes { get; set; }

    /// <summary>Largest response frame the client will buffer.</summary>
    public ulong? MaxResponseBytes { get; set; }

    /// <summary>Per-request deadline. <see cref="TimeSpan.Zero"/> disables it.</summary>
    public TimeSpan? RequestTimeout { get; set; }

    /// <summary>Connect + AUTH deadline.</summary>
    public TimeSpan? ConnectTimeout { get; set; }

    /// <summary>TCP keepalive idle period. Zero leaves the OS default.</summary>
    public TimeSpan? TcpKeepalive { get; set; }

    /// <summary>TCP_NODELAY. On by default.</summary>
    public bool? TcpNoDelay { get; set; }

    /// <summary>
    /// Frame multiplexing, on by default: a value larger than
    /// <see cref="MuxChunkBytes"/> travels in chunks with other requests
    /// and replies between them, so a small operation never waits behind
    /// a whole large one on its connection. <c>false</c> forces the
    /// legacy framing (no HELLO is sent).
    /// </summary>
    public bool? Multiplexing { get; set; }

    /// <summary>
    /// Chunk size for large values on a multiplexed connection. Default
    /// 1 MiB; held to the server's grant. Must be non-zero.
    /// </summary>
    public ulong? MuxChunkBytes { get; set; }

    /// <summary>
    /// Retries for an idempotent request whose connection dropped.
    /// Get, Set, Exists and Ping retry; Delete and Append never do,
    /// and a timeout never does.
    /// </summary>
    public uint? MaxRetries { get; set; }

    /// <summary>
    /// mTLS: the client certificate chain (leaf first), PEM, for a
    /// server started with THUNDERSTRIKE_TLS_CLIENT_CA. Set with
    /// <see cref="TlsClientKeyPem"/>.
    /// </summary>
    public byte[]? TlsClientCertPem { get; set; }

    /// <summary>mTLS: the client's PKCS#8 Ed25519 key, PEM.</summary>
    public byte[]? TlsClientKeyPem { get; set; }

    /// <summary>
    /// Connect WITHOUT TLS. Only the in-repo mock server answers that;
    /// the thunderstrike server is TLS-only.
    /// </summary>
    public bool InsecurePlaintext { get; set; }

    /// <summary>Retries for a Busy (not applied) request, all but Delete. Default 4.</summary>
    public uint? BusyRetries { get; set; }

    /// <summary>The first Busy backoff. Default 2 ms. Set with <see cref="BusyBackoffMax"/>.</summary>
    public TimeSpan? BusyBackoffMin { get; set; }

    /// <summary>The Busy backoff ceiling. Default 200 ms.</summary>
    public TimeSpan? BusyBackoffMax { get; set; }

    /// <summary>No Busy retry starts once the operation has run this long. Default 5 s.</summary>
    public TimeSpan? BusyRetryWindow { get; set; }

    /// <summary>Extra deadline per MiB of a Set/Append value. Default 100 ms.</summary>
    public TimeSpan? TimeoutPerMib { get; set; }

    /// <summary>How a key maps to an endpoint.</summary>
    public ThunderstrikeRouting? Routing { get; set; }

    /// <summary>Worker threads for the client's own runtime.</summary>
    public uint? WorkerThreads { get; set; }

    /// <summary>Add an endpoint and return this, for chaining.</summary>
    public ThunderstrikeOptions AddEndpoint(string endpoint)
    {
        if (string.IsNullOrWhiteSpace(endpoint))
        {
            throw new ArgumentException("endpoint must not be empty", nameof(endpoint));
        }
        Endpoints.Add(endpoint);
        return this;
    }
}

/// <summary>Pool counters.</summary>
public readonly struct ThunderstrikeStats
{
    /// <summary>Connections in the pool.</summary>
    public ulong Connections { get; }
    /// <summary>Connections currently usable.</summary>
    public ulong ConnectionsReady { get; }

    /// <summary>Requests on the wire right now.</summary>
    public ulong Inflight { get; }

    /// <summary>Requests that could be on the wire at once.</summary>
    public ulong Capacity { get; }

    /// <summary>Requests submitted.</summary>
    public ulong Requests { get; }
    /// <summary>Responses received.</summary>
    public ulong Responses { get; }
    /// <summary>Request bytes written to sockets.</summary>
    public ulong BytesWritten { get; }
    /// <summary>Response bytes read from sockets.</summary>
    public ulong BytesRead { get; }

    /// <summary><c>write</c> syscalls.</summary>
    public ulong WriteBatches { get; }

    /// <summary>Times a connection was rebuilt.</summary>
    public ulong Reconnects { get; }

    /// <summary>Answers whose waiter had already given up.</summary>
    public ulong OrphanResponses { get; }

    /// <summary>
    /// I/O threads running right now — the cores this client can be
    /// using. Connections are sockets; their readers and writers are
    /// tasks, so the connection count is not this number.
    /// </summary>
    public ulong IOThreads { get; }
    /// <summary>The floor the pool may narrow to.</summary>
    public ulong IOThreadsMin { get; }
    /// <summary>The ceiling the pool may widen to.</summary>
    public ulong IOThreadsMax { get; }
    /// <summary>Times the pool widened.</summary>
    public ulong IOGrew { get; }
    /// <summary>Times the pool narrowed.</summary>
    public ulong IOShrank { get; }
    /// <summary>Connections moved between threads to keep the split even.</summary>
    public ulong IOMoved { get; }

    internal ThunderstrikeStats(in NativeStats s)
    {
        Connections = s.Connections;
        ConnectionsReady = s.ConnectionsReady;
        Inflight = s.Inflight;
        Capacity = s.Capacity;
        Requests = s.Requests;
        Responses = s.Responses;
        BytesWritten = s.BytesWritten;
        BytesRead = s.BytesRead;
        WriteBatches = s.WriteBatches;
        Reconnects = s.Reconnects;
        OrphanResponses = s.OrphanResponses;
        IOThreads = s.IOThreads;
        IOThreadsMin = s.IOThreadsMin;
        IOThreadsMax = s.IOThreadsMax;
        IOGrew = s.IOGrew;
        IOShrank = s.IOShrank;
        IOMoved = s.IOMoved;
    }

    /// <summary>
    /// Requests per <c>write</c> syscall — the payoff from
    /// coalescing. 1.0 means no batching happened; higher is better.
    /// </summary>
    public double BatchingFactor => WriteBatches == 0 ? 0 : (double)Requests / WriteBatches;

    /// <summary>A one-line summary, for logs.</summary>
    public override string ToString() =>
        $"Connections={Connections} ({ConnectionsReady} ready), Requests={Requests}, " +
        $"Writes={WriteBatches} ({BatchingFactor:F1}x), Inflight={Inflight}, " +
        $"Reconnects={Reconnects}";
}
