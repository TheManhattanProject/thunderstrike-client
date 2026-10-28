package dev.thunderstrike.client;

/** How the client picks a connection for a key. */
public enum ThunderstrikeRouting {

    /**
     * Hash the key to a shard. A thunderstrike shard <em>is</em> a listening
     * port that owns its own store, so this is addressing, not load
     * balancing — the wrong port is a different database.
     */
    SHARD_BY_KEY(0),

    /** Treat every endpoint as interchangeable. */
    ROUND_ROBIN(1);

    private final int code;

    ThunderstrikeRouting(int code) {
        this.code = code;
    }

    int code() {
        return code;
    }
}
