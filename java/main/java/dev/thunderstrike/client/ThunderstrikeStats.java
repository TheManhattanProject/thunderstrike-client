package dev.thunderstrike.client;

import java.lang.foreign.MemorySegment;

/**
 * Pool counters.
 *
 * @param connections connections in the pool
 * @param connectionsReady connections currently usable
 * @param inflight requests on the wire right now
 * @param capacity requests that could be on the wire at once
 * @param requests requests submitted
 * @param responses responses received
 * @param bytesWritten request bytes written to sockets
 * @param bytesRead response bytes read from sockets
 * @param writeBatches {@code write} syscalls
 * @param reconnects times a connection was rebuilt
 * @param orphanResponses answers whose waiter had already given up
 * @param ioThreads I/O threads running now — the cores this client can
 *     be using. Connections are sockets; their readers and writers are
 *     tasks, so the connection count is not this number.
 * @param ioThreadsMin the floor the pool may narrow to
 * @param ioThreadsMax the ceiling the pool may widen to
 * @param ioGrew times the pool widened
 * @param ioShrank times the pool narrowed
 * @param ioMoved connections moved between threads to keep the split even
 */
public record ThunderstrikeStats(
        long connections,
        long connectionsReady,
        long inflight,
        long capacity,
        long requests,
        long responses,
        long bytesWritten,
        long bytesRead,
        long writeBatches,
        long reconnects,
        long orphanResponses,
        long ioThreads,
        long ioThreadsMin,
        long ioThreadsMax,
        long ioGrew,
        long ioShrank,
        long ioMoved) {

    static ThunderstrikeStats from(MemorySegment s) {
        // Read every field the layout declares. A count short of the
        // struct is not a missing field — the native side writes the
        // whole thing, so the tail would simply be unread.
        long[] v = new long[17];
        for (int i = 0; i < v.length; i++) {
            v[i] = s.getAtIndex(Native.C_LONG_LONG, i);
        }
        return new ThunderstrikeStats(
                v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8], v[9], v[10],
                v[11], v[12], v[13], v[14], v[15], v[16]);
    }

    /**
     * Requests per {@code write} syscall — the payoff from coalescing.
     * 1.0 means no batching happened; higher is better.
     */
    public double batchingFactor() {
        return writeBatches == 0 ? 0 : (double) requests / writeBatches;
    }

    @Override
    public String toString() {
        return String.format(
                "connections=%d (%d ready), requests=%d, writes=%d (%.1fx), inflight=%d, reconnects=%d",
                connections, connectionsReady, requests, writeBatches,
                batchingFactor(), inflight, reconnects);
    }
}
