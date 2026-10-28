package dev.thunderstrike.client;

/** The connection dropped mid-flight; the outcome is unknown. */
public final class ThunderstrikeDisconnectedException extends ThunderstrikeException {

    private static final long serialVersionUID = 1L;

    /**
     * @param operation the operation that failed, if known
     */
    public ThunderstrikeDisconnectedException(String operation) {
        super(ThunderstrikeCode.DISCONNECTED, operation);
    }
}
