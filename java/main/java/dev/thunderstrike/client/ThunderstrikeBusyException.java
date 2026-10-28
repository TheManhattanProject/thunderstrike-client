package dev.thunderstrike.client;

/**
 * The server was momentarily out of queue and did NOT apply the request.
 *
 * <p>The one retriable status: the same request may be sent again after
 * a short backoff. The client has already retried it ({@code
 * busyRetries}, every operation but delete) before throwing this.
 */
public final class ThunderstrikeBusyException extends ThunderstrikeException {

    private static final long serialVersionUID = 1L;

    ThunderstrikeBusyException(String operation) {
        super(ThunderstrikeCode.BUSY, operation);
    }
}
