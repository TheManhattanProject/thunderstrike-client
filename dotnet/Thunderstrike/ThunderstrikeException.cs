using System;
using System.Runtime.InteropServices;

namespace Thunderstrike;

/// <summary>Status codes, mirroring <c>thunderstrike_client.h</c>.</summary>
public static class ThunderstrikeCode
{
    /// <summary>Success.</summary>
    public const int Ok = 0;

    // ── Statuses the server reported (positive) ────────────────────

    /// <summary>The key is not there.</summary>
    public const int NotFound = 1;

    /// <summary>This shard does not own the key; rehash and retry.</summary>
    public const int WrongShard = 2;

    /// <summary>A compare-and-set comparison failed.</summary>
    public const int CasConflict = 3;

    /// <summary>The server's memory is genuinely exhausted. Not applied.</summary>
    public const int ServerOom = 4;

    /// <summary>
    /// Not authenticated. Never sent by the current server: it
    /// authenticates with TLS and closes instead.
    /// </summary>
    public const int NotAuthed = 5;

    /// <summary>The server does not know this opcode.</summary>
    public const int UnknownOp = 6;

    /// <summary>The server rejected the request as malformed.</summary>
    public const int Malformed = 7;

    /// <summary>The server speaks a different protocol version.</summary>
    public const int UnsupportedVersion = 9;

    /// <summary>
    /// This one response is too large for the server to serve
    /// contiguously. Distinct from <see cref="ServerOom"/>.
    /// </summary>
    public const int ResponseTooLarge = 11;

    /// <summary>The shard's disk is full, or a structural ceiling. Not applied.</summary>
    public const int StorageExhausted = 12;

    /// <summary>
    /// The server was momentarily out of queue and did NOT apply the
    /// request: the one retriable status. The client already retried it
    /// (every operation but Delete).
    /// </summary>
    public const int Busy = 13;

    /// <summary>The server failed the operation: integrity, device or invariant.</summary>
    public const int ServerError = 14;

    /// <summary>A status byte this client version does not know.</summary>
    public const int UnknownStatus = 64;

    // ── Conditions the client detected (negative) ──────────────────

    /// <summary>A null or otherwise unusable argument.</summary>
    public const int InvalidArg = -1;

    /// <summary>The configuration cannot work.</summary>
    public const int Config = -2;

    /// <summary>Could not establish a connection.</summary>
    public const int Connect = -3;

    /// <summary>A socket-level failure.</summary>
    public const int Io = -4;

    /// <summary>
    /// The deadline elapsed. The server may still apply the request.
    /// </summary>
    public const int Timeout = -5;

    /// <summary>
    /// The connection dropped mid-flight; the outcome is unknown.
    /// </summary>
    public const int Disconnected = -6;

    /// <summary>The peer sent bytes this client cannot interpret.</summary>
    public const int Protocol = -7;

    /// <summary>The key is longer than <see cref="MaxKeyLength"/>.</summary>
    public const int KeyTooLong = -8;

    /// <summary>The client has been disposed.</summary>
    public const int Closed = -9;

    /// <summary>
    /// The destination buffer was too small; the required length is
    /// reported alongside.
    /// </summary>
    public const int BufferTooSmall = -10;

    /// <summary>
    /// A runtime failure — most often a blocking call made from
    /// inside a completion callback.
    /// </summary>
    public const int Runtime = -11;

    /// <summary>A panic was caught at the native boundary.</summary>
    public const int Panic = -12;

    /// <summary>The TLS handshake failed or a record did not decrypt. (Was 13.)</summary>
    public const int Tls = -13;

    /// <summary>The value is longer than the server takes in one request.</summary>
    public const int ValueTooLarge = -14;

    /// <summary>RetryClass: the same request fails the same way.</summary>
    public const int RetryNever = 0;

    /// <summary>RetryClass: outcome unknown; repeat Get/Set/Exists/Ping only.</summary>
    public const int RetryIfIdempotent = 1;

    /// <summary>RetryClass: Busy, not applied; repeat anything after a backoff.</summary>
    public const int RetryAlways = 2;

    /// <summary>The wire's <c>key_len</c> is a <c>uint16</c>.</summary>
    public const int MaxKeyLength = 65535;
}

/// <summary>How the client picks a connection for a key.</summary>
public enum ThunderstrikeRouting
{
    /// <summary>Hash the key to a shard. A thunderstrike shard is a
    /// listening port that owns its own store, so this is addressing,
    /// not load balancing.</summary>
    ShardByKey = 0,

    /// <summary>Treat every endpoint as interchangeable.</summary>
    RoundRobin = 1,
}

/// <summary>Any non-OK status from the client or the server.</summary>
public class ThunderstrikeException : Exception
{
    /// <summary>The numeric status. See <see cref="ThunderstrikeCode"/>.</summary>
    public int Code { get; }

    /// <summary>The operation that failed, when known.</summary>
    public string? Operation { get; }

    /// <summary>
    /// Whether repeating the request is sound: <see cref="ThunderstrikeCode.RetryAlways"/>
    /// (Busy — not applied), <see cref="ThunderstrikeCode.RetryIfIdempotent"/> (the
    /// outcome is unknown) or <see cref="ThunderstrikeCode.RetryNever"/>.
    /// </summary>
    public int RetryClass => RetryClassOf(Code);

    /// <summary>The server answered Busy: not applied, retriable.</summary>
    public bool IsBusy => Code == ThunderstrikeCode.Busy;

    /// <summary><see cref="RetryClass"/> for a bare code.</summary>
    public static int RetryClassOf(int code)
    {
        try
        {
            return NativeMethods.thunderstrike_retry_class(code);
        }
        catch (DllNotFoundException)
        {
            return ThunderstrikeCode.RetryNever;
        }
    }

    /// <summary>Create an exception for a status code.</summary>
    /// <param name="code">The numeric status.</param>
    /// <param name="operation">The operation that failed, if known.</param>
    public ThunderstrikeException(int code, string? operation = null)
        : base(Format(code, operation))
    {
        Code = code;
        Operation = operation;
    }

    private static string Format(int code, string? operation)
    {
        var text = DescribeCode(code);
        var where = operation is null ? "thunderstrike" : $"thunderstrike: {operation}";

        // The detail, when the native side recorded one, is the part
        // worth reading: "TLS failed" says nothing a caller can act on,
        // "X509(NameMismatch)" says the certificate is for another
        // host. Appended rather than substituted so the code stays
        // greppable.
        var detail = LastErrorDetail();
        return detail.Length == 0
            ? $"{where}: {text} (code {code})"
            : $"{where}: {text} (code {code}): {detail}";
    }

    static ThunderstrikeException()
    {
#if NETCOREAPP3_0_OR_GREATER
        // DescribeCode can be the first native call a process makes.
        NativeLibraryResolver.EnsureInstalled();
#endif
    }

    /// <summary>The native library's description of a status code.</summary>
    public static string DescribeCode(int code)
    {
        try
        {
            var p = NativeMethods.thunderstrike_strerror(code);
            return p == IntPtr.Zero ? "unknown error" : Marshal.PtrToStringAnsi(p) ?? "unknown error";
        }
        catch (DllNotFoundException)
        {
            // Formatting an error must never fail for a second reason.
            return "unknown error";
        }
    }

    /// <summary>
    /// The detail behind this thread's most recent failure, or an empty
    /// string.
    /// </summary>
    /// <remarks>
    /// <see cref="DescribeCode"/> names the category ("TLS failed");
    /// this names the cause ("X509(NameMismatch)"). For TLS that
    /// difference is the diagnosis — a certificate for another name, an
    /// untrusted issuer and an expired one share a code and have
    /// different fixes.
    /// </remarks>
    public static string LastErrorDetail()
    {
        try
        {
            var p = NativeMethods.thunderstrike_last_error_message();
            return p == IntPtr.Zero ? "" : Marshal.PtrToStringAnsi(p) ?? "";
        }
        catch (DllNotFoundException)
        {
            return "";
        }
    }

    /// <summary>Throw unless <paramref name="code"/> is OK.</summary>
    internal static void ThrowIfError(int code, string operation)
    {
        if (code != ThunderstrikeCode.Ok)
        {
            throw Create(code, operation);
        }
    }

    internal static ThunderstrikeException Create(int code, string operation) => code switch
    {
        ThunderstrikeCode.NotFound => new ThunderstrikeKeyNotFoundException(operation),
        ThunderstrikeCode.Timeout => new ThunderstrikeTimeoutException(operation),
        ThunderstrikeCode.Disconnected => new ThunderstrikeDisconnectedException(operation),
        ThunderstrikeCode.Closed => new ThunderstrikeClosedException(operation),
        ThunderstrikeCode.Busy => new ThunderstrikeBusyException(operation),
        _ => new ThunderstrikeException(code, operation),
    };
}

/// <summary>The key is absent. Only thrown by members that promise a
/// value; <c>Get</c> returns <see langword="null"/> instead.</summary>
public sealed class ThunderstrikeKeyNotFoundException : ThunderstrikeException
{
    /// <summary>Create the exception. The key was absent.</summary>
    /// <param name="operation">The operation that failed, if known.</param>
    public ThunderstrikeKeyNotFoundException(string? operation = null)
        : base(ThunderstrikeCode.NotFound, operation) { }
}

/// <summary>The deadline elapsed. The server may still apply the
/// request — thunderstrike has no cancel opcode.</summary>
public sealed class ThunderstrikeTimeoutException : ThunderstrikeException
{
    /// <summary>Create the exception. The deadline elapsed.</summary>
    /// <param name="operation">The operation that failed, if known.</param>
    public ThunderstrikeTimeoutException(string? operation = null)
        : base(ThunderstrikeCode.Timeout, operation) { }
}

/// <summary>The connection dropped mid-flight; the outcome is unknown.</summary>
public sealed class ThunderstrikeDisconnectedException : ThunderstrikeException
{
    /// <summary>Create the exception. The connection dropped.</summary>
    /// <param name="operation">The operation that failed, if known.</param>
    public ThunderstrikeDisconnectedException(string? operation = null)
        : base(ThunderstrikeCode.Disconnected, operation) { }
}

/// <summary>The client has been disposed.</summary>
public sealed class ThunderstrikeClosedException : ThunderstrikeException
{
    /// <summary>Create the exception. The client was disposed.</summary>
    /// <param name="operation">The operation that failed, if known.</param>
    public ThunderstrikeClosedException(string? operation = null)
        : base(ThunderstrikeCode.Closed, operation) { }
}

/// <summary>The server was momentarily out of queue and did NOT apply
/// the request. Retriable after a short backoff; the client already
/// retried it (every operation but Delete).</summary>
public sealed class ThunderstrikeBusyException : ThunderstrikeException
{
    /// <summary>Create the exception. The server answered Busy.</summary>
    /// <param name="operation">The operation that failed, if known.</param>
    public ThunderstrikeBusyException(string? operation = null)
        : base(ThunderstrikeCode.Busy, operation) { }
}
