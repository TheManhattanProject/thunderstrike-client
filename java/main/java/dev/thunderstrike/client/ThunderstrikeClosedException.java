package dev.thunderstrike.client;

/** The client has been closed. */
public final class ThunderstrikeClosedException extends ThunderstrikeException {

    private static final long serialVersionUID = 1L;

    /**
     * @param operation the operation that failed, if known
     */
    public ThunderstrikeClosedException(String operation) {
        super(ThunderstrikeCode.CLOSED, operation);
    }
}
