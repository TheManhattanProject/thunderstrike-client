package dev.thunderstrike.client;

/**
 * Frame multiplexing counters.
 *
 * @param connections connections that negotiated multiplexing and are up now
 * @param chunkedRequests requests whose value went in chunks
 * @param chunkedReplies replies that came in chunks
 */
public record ThunderstrikeMuxStats(long connections, long chunkedRequests, long chunkedReplies) {}
