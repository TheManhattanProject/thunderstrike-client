package dev.thunderstrike.client;

import java.lang.foreign.MemorySegment;

/** Any non-OK status from the client or the server. */
public class ThunderstrikeException extends RuntimeException {

    private static final long serialVersionUID = 1L;

    private final int code;

    /**
     * @param code the numeric status; see {@link ThunderstrikeCode}
     * @param operation the operation that failed, if known
     */
    public ThunderstrikeException(int code, String operation) {
        super(format(code, operation));
        this.code = code;
    }

    /** The numeric status. */
    public int code() {
        return code;
    }

    /**
     * Whether repeating the request is sound: {@link
     * ThunderstrikeCode#RETRY_ALWAYS} (Busy — not applied), {@link
     * ThunderstrikeCode#RETRY_IF_IDEMPOTENT} (the outcome is unknown — get,
     * set, exists and ping only) or {@link ThunderstrikeCode#RETRY_NEVER}.
     */
    public int retryClass() {
        return retryClassOf(code);
    }

    /** The server answered Busy: not applied, retriable. */
    public boolean isBusy() {
        return code == ThunderstrikeCode.BUSY;
    }

    /** {@link #retryClass()} for a bare code. */
    public static int retryClassOf(int code) {
        try {
            return (int) Native.RETRY_CLASS.invokeExact(code);
        } catch (Throwable t) {
            return ThunderstrikeCode.RETRY_NEVER;
        }
    }

    private static String format(int code, String operation) {
        String text = describe(code);
        String where = operation == null ? "thunderstrike: " : "thunderstrike: " + operation + ": ";
        // The detail, when the native side recorded one, is the part
        // worth reading: "TLS failed" says nothing a caller can act on,
        // "X509(NameMismatch)" says the certificate is for another
        // host. Appended rather than substituted so the code stays
        // greppable.
        String detail = lastErrorDetail();
        String tail = detail.isEmpty() ? "" : ": " + detail;
        return where + text + " (code " + code + ")" + tail;
    }

    /**
     * The specific cause of this thread's most recent failure, or an
     * empty string.
     *
     * <p>{@link #describe} names the category ("TLS failed"); this
     * names the cause ("X509(NameMismatch)"). For TLS that difference
     * is the whole diagnosis — a certificate issued for another name,
     * an untrusted issuer and an expired one share a code and have
     * different fixes.
     */
    public static String lastErrorDetail() {
        try {
            MemorySegment p = (MemorySegment) Native.LAST_ERROR_MESSAGE.invokeExact();
            return Native.cString(p);
        } catch (Throwable t) {
            // Formatting an error must never fail for a second reason.
            return "";
        }
    }

    /** The native library's description of a status code. */
    public static String describe(int code) {
        try {
            MemorySegment p = (MemorySegment) Native.STRERROR.invokeExact(code);
            return Native.cString(p);
        } catch (Throwable t) {
            // Formatting an error must never fail for a second reason.
            return "unknown error";
        }
    }

    /** Throw unless {@code code} is OK. */
    static void check(int code, String operation) {
        if (code != ThunderstrikeCode.OK) {
            throw of(code, operation);
        }
    }

    static ThunderstrikeException of(int code, String operation) {
        return switch (code) {
            case ThunderstrikeCode.NOT_FOUND -> new ThunderstrikeKeyNotFoundException(operation);
            case ThunderstrikeCode.TIMEOUT -> new ThunderstrikeTimeoutException(operation);
            case ThunderstrikeCode.DISCONNECTED -> new ThunderstrikeDisconnectedException(operation);
            case ThunderstrikeCode.CLOSED -> new ThunderstrikeClosedException(operation);
            case ThunderstrikeCode.BUSY -> new ThunderstrikeBusyException(operation);
            default -> new ThunderstrikeException(code, operation);
        };
    }
}
