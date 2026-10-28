package dev.thunderstrike.client;

/** The key is absent. Only thrown by members that promise a value; {@code get} returns null instead. */
public final class ThunderstrikeKeyNotFoundException extends ThunderstrikeException {

    private static final long serialVersionUID = 1L;

    /**
     * @param operation the operation that failed, if known
     */
    public ThunderstrikeKeyNotFoundException(String operation) {
        super(ThunderstrikeCode.NOT_FOUND, operation);
    }
}
