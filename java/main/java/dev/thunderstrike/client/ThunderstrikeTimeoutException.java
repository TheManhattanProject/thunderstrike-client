package dev.thunderstrike.client;

/** The deadline elapsed. The server may still apply the request — thunderstrike has no cancel opcode. */
public final class ThunderstrikeTimeoutException extends ThunderstrikeException {

    private static final long serialVersionUID = 1L;

    /**
     * @param operation the operation that failed, if known
     */
    public ThunderstrikeTimeoutException(String operation) {
        super(ThunderstrikeCode.TIMEOUT, operation);
    }
}
