package dev.thunderstrike.client;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executor;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicLong;

/**
 * A pooled, pipelined client for the thunderstrike key-value database.
 *
 * <pre>{@code
 * try (var db = ThunderstrikeClient.connect("127.0.0.1:7379")) {
 *     db.set("user:1", "ada");
 *     System.out.println(db.getString("user:1"));   // ada
 *     System.out.println(db.getString("nope"));     // null
 *
 *     // One round trip, not 1000.
 *     var values = db.getMany(keys);
 * }
 * }</pre>
 *
 * <h2>How it talks to the native code</h2>
 *
 * <p>Through the FFM API (Project Panama, stable since JDK 22) —
 * plain Java calling the same C ABI the C, Go, Python and .NET
 * bindings use. There is no JNI and no Java-specific native library:
 * the only artifact is {@code libthunderstrike_client}, shared with every
 * other binding.
 *
 * <h2>Threading</h2>
 *
 * <p>Thread-safe: share one instance across the process. The
 * synchronous methods block the calling thread. The {@code *Async}
 * methods do not — they hand the request to the native runtime and
 * complete a {@link CompletableFuture} when the answer arrives, so no
 * thread is held while waiting.
 *
 * <h2>Batches</h2>
 *
 * <p>For more than one key prefer {@link #getMany}, {@link #setMany}
 * and {@link #deleteMany}: they put the whole batch on the wire before
 * awaiting any answer, so n keys cost one round trip instead of n.
 */
public final class ThunderstrikeClient implements AutoCloseable {

    private final MemorySegment handle;
    private volatile boolean closed;

    // ── Async plumbing ─────────────────────────────────────────────
    //
    // One upcall stub for the process, in the global arena so it
    // outlives every request. Per-request state travels through the
    // C ABI's user_data as an opaque token, looked up here — the same
    // shape the .NET binding uses, and for the same reason: a stub
    // created per call could be freed while native code still holds
    // its address.

    private static final AtomicLong NEXT_TOKEN = new AtomicLong(1);
    private static final ConcurrentHashMap<Long, Pending> PENDING = new ConcurrentHashMap<>();
    private static final MemorySegment CALLBACK_STUB = makeCallbackStub();

    /**
     * Where a future's dependents run.
     *
     * <p>Load-bearing, not decoration: a plain {@code complete()} runs
     * every dependent stage inline on the thread that completed it —
     * here, one of the native client's I/O threads. Blocking or slow
     * work there would stall the client for every other caller.
     */
    private static final Executor COMPLETION_EXECUTOR =
            Executors.newCachedThreadPool(r -> {
                Thread t = new Thread(r, "thunderstrike-completion");
                t.setDaemon(true);
                return t;
            });

    private record Pending(CompletableFuture<byte[]> future, int kind, String operation) {
        static final int VALUE = 0; // resolve to the bytes, or null on a miss
        static final int UNIT = 1;  // resolve to null
        static final int FOUND = 2; // resolve to a 1-byte flag
    }

    private static MemorySegment makeCallbackStub() {
        try {
            MethodHandle target = MethodHandles.lookup()
                    .findStatic(
                            ThunderstrikeClient.class,
                            "onCompletion",
                            MethodType.methodType(
                                    void.class, MemorySegment.class, int.class,
                                    MemorySegment.class, long.class));
            return Native.upcall(target, Arena.global());
        } catch (ReflectiveOperationException e) {
            throw new ExceptionInInitializerError(e);
        }
    }

    /**
     * Invoked by native code on one of its I/O threads.
     *
     * <p>Must not throw: an exception crossing back into native frames
     * is undefined behaviour. Must not block: the thread belongs to
     * the client.
     */
    @SuppressWarnings("unused") // called from native code via the upcall stub
    private static void onCompletion(MemorySegment userData, int code, MemorySegment data, long len) {
        try {
            Pending p = PENDING.remove(userData.address());
            if (p == null) {
                return; // already completed or abandoned
            }
            if (code == ThunderstrikeCode.OK) {
                byte[] value = switch (p.kind()) {
                    case Pending.VALUE -> Native.bytes(data, len);
                    case Pending.FOUND -> new byte[] {1};
                    default -> null;
                };
                COMPLETION_EXECUTOR.execute(() -> p.future().complete(value));
            } else if (code == ThunderstrikeCode.NOT_FOUND
                    && (p.kind() == Pending.VALUE || p.kind() == Pending.FOUND)) {
                byte[] value = p.kind() == Pending.FOUND ? new byte[] {0} : null;
                COMPLETION_EXECUTOR.execute(() -> p.future().complete(value));
            } else {
                ThunderstrikeException e = ThunderstrikeException.of(code, p.operation());
                COMPLETION_EXECUTOR.execute(() -> p.future().completeExceptionally(e));
            }
        } catch (Throwable ignored) {
            // Nothing useful to do, and letting it propagate would
            // unwind into native code.
        }
    }

    // ── Construction ───────────────────────────────────────────────

    private ThunderstrikeClient(MemorySegment handle) {
        this.handle = handle;
    }

    /** Connect to a single endpoint with default settings. */
    public static ThunderstrikeClient connect(String endpoint) {
        return connect(new ThunderstrikeOptions().endpoint(endpoint));
    }

    /**
     * Connect with explicit settings.
     *
     * <p>Every connection in the pool is established (and
     * authenticated) before this returns, so a wrong address or a bad
     * token throws here rather than on first use.
     */
    public static ThunderstrikeClient connect(ThunderstrikeOptions options) {
        if (options == null) {
            throw new IllegalArgumentException("options must not be null");
        }
        if (options.endpoints.isEmpty() && options.host == null) {
            throw new IllegalArgumentException(
                    "give at least one endpoint, or a cluster(...)");
        }
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment cfg = (MemorySegment) Native.CONFIG_NEW.invokeExact();
            if (cfg.equals(MemorySegment.NULL)) {
                throw new OutOfMemoryError("thunderstrike_config_new");
            }
            try {
                for (String addr : options.endpoints) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_ADD_ENDPOINT.invokeExact(
                                    cfg, arena.allocateFrom(addr)),
                            "addEndpoint");
                }
                if (options.host != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_CLUSTER.invokeExact(
                                    cfg,
                                    arena.allocateFrom(options.host),
                                    (short) options.basePort,
                                    (short) options.shards,
                                    (short) options.cores),
                            "cluster");
                }
                if (options.tlsServerName != null || options.tlsCaPem != null) {
                    if (options.tlsServerName == null || options.tlsCaPem == null) {
                        throw new IllegalArgumentException(
                                "tlsServerName and tlsCaPem must be set together - a trust "
                                        + "anchor with no name to check it against verifies nothing");
                    }
                    MemorySegment name = arena.allocateFrom(options.tlsServerName);
                    MemorySegment pem = arena.allocateFrom(Native.C_CHAR, options.tlsCaPem);
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_TLS.invokeExact(
                                    cfg, name, pem, (long) options.tlsCaPem.length),
                            "tls");
                }
                if (options.tlsClientCertPem != null || options.tlsClientKeyPem != null) {
                    if (options.tlsClientCertPem == null || options.tlsClientKeyPem == null) {
                        throw new IllegalArgumentException(
                                "the client certificate and key go together");
                    }
                    MemorySegment cert = arena.allocateFrom(Native.C_CHAR, options.tlsClientCertPem);
                    MemorySegment key = arena.allocateFrom(Native.C_CHAR, options.tlsClientKeyPem);
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_CLIENT_IDENTITY.invokeExact(
                                    cfg, cert, (long) options.tlsClientCertPem.length,
                                    key, (long) options.tlsClientKeyPem.length),
                            "clientIdentity");
                }
                if (options.insecurePlaintext) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_INSECURE_PLAINTEXT.invokeExact(cfg, 1),
                            "insecurePlaintext");
                }
                if (options.busyRetries != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_BUSY_RETRIES.invokeExact(
                                    cfg, options.busyRetries.intValue()),
                            "busyRetries");
                }
                if (options.busyBackoffMin != null && options.busyBackoffMax != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_BUSY_BACKOFF.invokeExact(
                                    cfg, options.busyBackoffMin.toMillis(),
                                    options.busyBackoffMax.toMillis()),
                            "busyBackoff");
                }
                if (options.busyRetryWindow != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_BUSY_RETRY_WINDOW.invokeExact(
                                    cfg, options.busyRetryWindow.toMillis()),
                            "busyRetryWindow");
                }
                if (options.timeoutPerMib != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_TIMEOUT_PER_MIB.invokeExact(
                                    cfg, options.timeoutPerMib.toMillis()),
                            "timeoutPerMib");
                }
                if (options.connectionsPerEndpoint != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_CONNECTIONS.invokeExact(
                                    cfg, options.connectionsPerEndpoint.intValue()),
                            "connectionsPerEndpoint");
                }
                if (options.maxInflight != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_MAX_INFLIGHT.invokeExact(
                                    cfg, options.maxInflight.intValue()),
                            "maxInflight");
                }
                if (options.maxBufferedBytes != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_MAX_BUFFERED.invokeExact(
                                    cfg, options.maxBufferedBytes.longValue()),
                            "maxBufferedBytes");
                }
                if (options.maxResponseBytes != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_MAX_RESPONSE.invokeExact(
                                    cfg, options.maxResponseBytes.longValue()),
                            "maxResponseBytes");
                }
                if (options.requestTimeout != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_REQUEST_TIMEOUT.invokeExact(
                                    cfg, millis(options.requestTimeout)),
                            "requestTimeout");
                }
                if (options.connectTimeout != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_CONNECT_TIMEOUT.invokeExact(
                                    cfg, millis(options.connectTimeout)),
                            "connectTimeout");
                }
                if (options.tcpKeepalive != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_KEEPALIVE.invokeExact(
                                    cfg, millis(options.tcpKeepalive)),
                            "tcpKeepalive");
                }
                if (options.tcpNoDelay != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_NODELAY.invokeExact(
                                    cfg, options.tcpNoDelay ? 1 : 0),
                            "tcpNoDelay");
                }
                if (options.maxRetries != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_MAX_RETRIES.invokeExact(
                                    cfg, options.maxRetries.intValue()),
                            "maxRetries");
                }
                if (options.routing != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_ROUTING.invokeExact(
                                    cfg, options.routing.code()),
                            "routing");
                }
                if (options.workerThreads != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_WORKER_THREADS.invokeExact(
                                    cfg, options.workerThreads.intValue()),
                            "workerThreads");
                }
                if (options.multiplexing != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_MULTIPLEXING.invokeExact(
                                    cfg, options.multiplexing ? 1 : 0),
                            "multiplexing");
                }
                if (options.muxChunkBytes != null) {
                    ThunderstrikeException.check(
                            (int) Native.CONFIG_SET_MUX_CHUNK_BYTES.invokeExact(
                                    cfg, options.muxChunkBytes.longValue()),
                            "muxChunkBytes");
                }

                MemorySegment out = arena.allocate(Native.C_POINTER);
                ThunderstrikeException.check(
                        (int) Native.CONNECT.invokeExact(cfg, out), "connect");
                return new ThunderstrikeClient(out.get(Native.C_POINTER, 0));
            } finally {
                Native.CONFIG_FREE.invokeExact(cfg);
            }
        } catch (ThunderstrikeException | IllegalArgumentException | OutOfMemoryError e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "connect: " + t);
        }
    }

    /** Shut the pool down. Idempotent. */
    @Override
    public void close() {
        if (closed) {
            return;
        }
        synchronized (this) {
            if (closed) {
                return;
            }
            closed = true;
        }
        try {
            Native.CLIENT_FREE.invokeExact(handle);
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "close: " + t);
        }
    }

    private MemorySegment handle() {
        if (closed) {
            throw new ThunderstrikeClosedException(null);
        }
        return handle;
    }

    // ── Single keys ────────────────────────────────────────────────

    /** Read a key. {@code null} is a clean miss. */
    public byte[] get(byte[] key) {
        checkKey(key);
        try {
            Scratch sc = Scratch.get();
            MemorySegment k = sc.key(key);
            MemorySegment buf = sc.bufOut;
            int code = (int) Native.GET.invokeExact(handle(), k, (long) key.length, buf);
            try {
                return switch (code) {
                    case ThunderstrikeCode.OK -> readBuf(buf);
                    case ThunderstrikeCode.NOT_FOUND -> null;
                    default -> throw ThunderstrikeException.of(code, "get");
                };
            } finally {
                // Always: the buffer holds a reference into the
                // client's read buffer, and it is initialised even on
                // a miss.
                Native.BUF_FREE.invokeExact(buf);
            }
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "get: " + t);
        }
    }

    /** Read a key as UTF-8 text. {@code null} is a clean miss. */
    public String getString(String key) {
        byte[] v = get(utf8(key));
        return v == null ? null : new String(v, StandardCharsets.UTF_8);
    }

    /**
     * Read a key straight into {@code destination} — no allocation.
     *
     * @return the value's length, or {@code -1} on a miss. A length
     *     greater than {@code destination.length} means nothing was
     *     copied and that is how large a buffer to bring back.
     */
    public int getInto(byte[] key, byte[] destination) {
        checkKey(key);
        if (destination == null) {
            throw new IllegalArgumentException("destination must not be null");
        }
        try {
            Scratch sc = Scratch.get();
            MemorySegment k = sc.key(key);
            MemorySegment dst = sc.sized(Math.max(destination.length, 1));
            MemorySegment outLen = sc.sizeOut;
            int code = (int) Native.GET_INTO.invokeExact(
                    handle(), k, (long) key.length, dst, (long) destination.length, outLen);
            long len = outLen.get(Native.C_SIZE_T, 0);
            return switch (code) {
                case ThunderstrikeCode.OK -> {
                    MemorySegment.copy(dst, Native.C_CHAR, 0, destination, 0, (int) len);
                    yield (int) len;
                }
                case ThunderstrikeCode.NOT_FOUND -> -1;
                case ThunderstrikeCode.BUFFER_TOO_SMALL -> (int) len;
                default -> throw ThunderstrikeException.of(code, "getInto");
            };
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "getInto: " + t);
        }
    }

    /** Write a key. */
    public void set(byte[] key, byte[] value) {
        checkKey(key);
        if (value == null) {
            throw new IllegalArgumentException("value must not be null");
        }
        try {
            Scratch sc = Scratch.get();
            MemorySegment k = sc.key(key);
            MemorySegment v = sc.value(value);
            ThunderstrikeException.check(
                    (int) Native.SET.invokeExact(
                            handle(), k, (long) key.length, v, (long) value.length),
                    "set");
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "set: " + t);
        }
    }

    /** Write a key from UTF-8 text. */
    public void set(String key, String value) {
        set(utf8(key), utf8(value));
    }

    /** Delete a key. {@code false} if it was not there. */
    public boolean delete(byte[] key) {
        checkKey(key);
        try {
            MemorySegment k = Scratch.get().key(key);
            int code = (int) Native.DELETE.invokeExact(handle(), k, (long) key.length);
            return switch (code) {
                case ThunderstrikeCode.OK -> true;
                case ThunderstrikeCode.NOT_FOUND -> false;
                default -> throw ThunderstrikeException.of(code, "delete");
            };
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "delete: " + t);
        }
    }

    /** Test for a key without transferring its value. */
    public boolean exists(byte[] key) {
        checkKey(key);
        try {
            MemorySegment k = Scratch.get().key(key);
            int code = (int) Native.EXISTS.invokeExact(handle(), k, (long) key.length);
            return switch (code) {
                case ThunderstrikeCode.OK -> true;
                case ThunderstrikeCode.NOT_FOUND -> false;
                default -> throw ThunderstrikeException.of(code, "exists");
            };
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "exists: " + t);
        }
    }

    /**
     * Append to a key's value, creating it when absent. Returns the
     * value's total length afterwards.
     */
    public long append(byte[] key, byte[] value) {
        checkKey(key);
        if (value == null) {
            throw new IllegalArgumentException("value must not be null");
        }
        try {
            Scratch sc = Scratch.get();
            MemorySegment k = sc.key(key);
            MemorySegment v = sc.value(value);
            MemorySegment total = sc.u64Out;
            ThunderstrikeException.check(
                    (int) Native.APPEND.invokeExact(
                            handle(), k, (long) key.length, v, (long) value.length, total),
                    "append");
            return total.get(Native.C_LONG_LONG, 0);
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "append: " + t);
        }
    }

    /** Round-trip a liveness probe. */
    public void ping() {
        try {
            ThunderstrikeException.check((int) Native.PING.invokeExact(handle()), "ping");
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "ping: " + t);
        }
    }

    // ── Batches ────────────────────────────────────────────────────

    /**
     * Read many keys in ONE round trip. A {@code null} entry is a
     * miss.
     */
    public byte[][] getMany(List<byte[]> keys) {
        if (keys == null) {
            throw new IllegalArgumentException("keys must not be null");
        }
        int n = keys.size();
        if (n == 0) {
            return new byte[0][];
        }
        try (Arena arena = Arena.ofConfined()) {
            Batch b = new Batch(arena, keys);
            MemorySegment bufs = arena.allocate(Native.BUF_LAYOUT, n);
            MemorySegment codes = arena.allocate(Native.C_INT, n);

            int rc = (int) Native.MGET.invokeExact(
                    handle(), b.pointers, b.lengths, (long) n, bufs, codes);
            try {
                ThunderstrikeException.check(rc, "getMany");
                byte[][] out = new byte[n][];
                for (int i = 0; i < n; i++) {
                    int code = codes.getAtIndex(Native.C_INT, i);
                    MemorySegment one = bufs.asSlice(i * Native.BUF_LAYOUT.byteSize(),
                            Native.BUF_LAYOUT.byteSize());
                    out[i] = switch (code) {
                        case ThunderstrikeCode.OK -> readBuf(one);
                        case ThunderstrikeCode.NOT_FOUND -> null;
                        default -> throw ThunderstrikeException.of(code, "getMany[" + i + "]");
                    };
                }
                return out;
            } finally {
                for (int i = 0; i < n; i++) {
                    MemorySegment one = bufs.asSlice(i * Native.BUF_LAYOUT.byteSize(),
                            Native.BUF_LAYOUT.byteSize());
                    Native.BUF_FREE.invokeExact(one);
                }
            }
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "getMany: " + t);
        }
    }

    /** Write many pairs in ONE round trip. */
    public void setMany(List<byte[]> keys, List<byte[]> values) {
        if (keys == null || values == null) {
            throw new IllegalArgumentException("keys and values must not be null");
        }
        if (keys.size() != values.size()) {
            throw new IllegalArgumentException("as many values as keys");
        }
        int n = keys.size();
        if (n == 0) {
            return;
        }
        try (Arena arena = Arena.ofConfined()) {
            Batch k = new Batch(arena, keys);
            Batch v = new Batch(arena, values);
            MemorySegment codes = arena.allocate(Native.C_INT, n);

            ThunderstrikeException.check(
                    (int) Native.MSET.invokeExact(
                            handle(), k.pointers, k.lengths, v.pointers, v.lengths,
                            (long) n, codes),
                    "setMany");
            for (int i = 0; i < n; i++) {
                int code = codes.getAtIndex(Native.C_INT, i);
                if (code != ThunderstrikeCode.OK) {
                    throw ThunderstrikeException.of(code, "setMany[" + i + "]");
                }
            }
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "setMany: " + t);
        }
    }

    /** Delete many keys in ONE round trip. */
    public boolean[] deleteMany(List<byte[]> keys) {
        if (keys == null) {
            throw new IllegalArgumentException("keys must not be null");
        }
        int n = keys.size();
        if (n == 0) {
            return new boolean[0];
        }
        try (Arena arena = Arena.ofConfined()) {
            Batch b = new Batch(arena, keys);
            MemorySegment codes = arena.allocate(Native.C_INT, n);

            ThunderstrikeException.check(
                    (int) Native.MDELETE.invokeExact(
                            handle(), b.pointers, b.lengths, (long) n, codes),
                    "deleteMany");
            boolean[] out = new boolean[n];
            for (int i = 0; i < n; i++) {
                int code = codes.getAtIndex(Native.C_INT, i);
                out[i] = switch (code) {
                    case ThunderstrikeCode.OK -> true;
                    case ThunderstrikeCode.NOT_FOUND -> false;
                    default -> throw ThunderstrikeException.of(code, "deleteMany[" + i + "]");
                };
            }
            return out;
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "deleteMany: " + t);
        }
    }

    // ── Async ──────────────────────────────────────────────────────

    /**
     * Read a key without holding a thread while waiting. Completes
     * with {@code null} on a clean miss.
     */
    public CompletableFuture<byte[]> getAsync(byte[] key) {
        checkKey(key);
        return submit(Pending.VALUE, "getAsync", (arena, token) -> {
            MemorySegment k = copy(arena, key);
            return (int) Native.GET_ASYNC.invokeExact(
                    handle(), k, (long) key.length, CALLBACK_STUB, token);
        });
    }

    /** Write a key without holding a thread while waiting. */
    public CompletableFuture<Void> setAsync(byte[] key, byte[] value) {
        checkKey(key);
        if (value == null) {
            throw new IllegalArgumentException("value must not be null");
        }
        return submit(Pending.UNIT, "setAsync", (arena, token) -> {
            MemorySegment k = copy(arena, key);
            MemorySegment v = copy(arena, value);
            return (int) Native.SET_ASYNC.invokeExact(
                    handle(), k, (long) key.length, v, (long) value.length,
                    CALLBACK_STUB, token);
        }).thenApply(ignored -> null);
    }

    /** Delete a key without holding a thread while waiting. */
    public CompletableFuture<Boolean> deleteAsync(byte[] key) {
        checkKey(key);
        return submit(Pending.FOUND, "deleteAsync", (arena, token) -> {
            MemorySegment k = copy(arena, key);
            return (int) Native.DELETE_ASYNC.invokeExact(
                    handle(), k, (long) key.length, CALLBACK_STUB, token);
        }).thenApply(flag -> flag != null && flag.length > 0 && flag[0] == 1);
    }

    @FunctionalInterface
    private interface Submission {
        int submit(Arena arena, MemorySegment token) throws Throwable;
    }

    private CompletableFuture<byte[]> submit(int kind, String operation, Submission call) {
        CompletableFuture<byte[]> future = new CompletableFuture<>();
        long id = NEXT_TOKEN.getAndIncrement();
        PENDING.put(id, new Pending(future, kind, operation));

        // The key and value are copied into native memory here and
        // the C ABI copies them again before returning, so this arena
        // can close as soon as the call does.
        try (Arena arena = Arena.ofConfined()) {
            int rc = call.submit(arena, MemorySegment.ofAddress(id));
            if (rc != ThunderstrikeCode.OK) {
                // No callback will arrive; drop the entry ourselves or
                // it pins the future forever.
                PENDING.remove(id);
                future.completeExceptionally(ThunderstrikeException.of(rc, operation));
            }
        } catch (Throwable t) {
            PENDING.remove(id);
            future.completeExceptionally(
                    new ThunderstrikeException(ThunderstrikeCode.RUNTIME, operation + ": " + t));
        }
        return future;
    }

    // ── Introspection ──────────────────────────────────────────────

    /** The pool's counters. */
    public ThunderstrikeStats stats() {
        try {
            MemorySegment s = Scratch.get().statsOut;
            ThunderstrikeException.check((int) Native.GET_STATS.invokeExact(handle(), s), "stats");
            return ThunderstrikeStats.from(s);
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "stats: " + t);
        }
    }

    /** Frame multiplexing counters. */
    public ThunderstrikeMuxStats muxStats() {
        try (java.lang.foreign.Arena a = java.lang.foreign.Arena.ofConfined()) {
            MemorySegment out = a.allocate(Native.C_LONG_LONG, 3);
            ThunderstrikeException.check(
                    (int) Native.GET_MUX_STATS.invokeExact(
                            handle(),
                            out,
                            out.asSlice(8),
                            out.asSlice(16)),
                    "muxStats");
            return new ThunderstrikeMuxStats(
                    out.getAtIndex(Native.C_LONG_LONG, 0),
                    out.getAtIndex(Native.C_LONG_LONG, 1),
                    out.getAtIndex(Native.C_LONG_LONG, 2));
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "muxStats: " + t);
        }
    }

    /**
     * Which shard this client routes {@code key} to, under the routing
     * table it read from the server.
     */
    public int shardOf(byte[] key) {
        checkKey(key);
        try {
            Scratch sc = Scratch.get();
            MemorySegment k = sc.key(key);
            MemorySegment out = sc.u16Out;
            ThunderstrikeException.check(
                    (int) Native.SHARD_OF.invokeExact(handle(), k, (long) key.length, out),
                    "shardOf");
            return Short.toUnsignedInt(out.get(Native.C_SHORT, 0));
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "shardOf: " + t);
        }
    }

    /**
     * Which of {@code shards} shards owns {@code key}:
     * {@code CRC32C(key) % shards}, the same function the server
     * places data with. Needs no client.
     */
    public static int shardOf(byte[] key, int shards) {
        if (key == null) {
            throw new IllegalArgumentException("key must not be null");
        }
        try {
            Scratch sc = Scratch.get();
            MemorySegment k = sc.key(key);
            MemorySegment out = sc.u16Out;
            ThunderstrikeException.check(
                    (int) Native.HASH_SHARD.invokeExact(
                            k, (long) key.length, (short) shards, out),
                    "shardOf");
            return Short.toUnsignedInt(out.get(Native.C_SHORT, 0));
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "shardOf: " + t);
        }
    }

    /**
     * How many shards the deployment has, as the server stated it.
     * 0 under round-robin routing, which has no shards.
     */
    public int shards() {
        try {
            MemorySegment out = Scratch.get().u16Out;
            ThunderstrikeException.check(
                    (int) Native.SHARD_COUNT.invokeExact(handle(), out), "shards");
            return Short.toUnsignedInt(out.get(Native.C_SHORT, 0));
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "shards: " + t);
        }
    }

    /**
     * How many endpoints this client holds - one per server core,
     * discovered rather than configured.
     */
    public int endpoints() {
        try {
            MemorySegment out = Scratch.get().u32Out;
            ThunderstrikeException.check(
                    (int) Native.ENDPOINT_COUNT.invokeExact(handle(), out), "endpoints");
            return out.get(Native.C_INT, 0);
        } catch (ThunderstrikeException e) {
            throw e;
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "endpoints: " + t);
        }
    }

    /** The native library's version. */
    public static String version() {
        try {
            MemorySegment p = (MemorySegment) Native.VERSION.invokeExact();
            return Native.cString(p);
        } catch (Throwable t) {
            throw new ThunderstrikeException(ThunderstrikeCode.RUNTIME, "version: " + t);
        }
    }

    // ── Plumbing ───────────────────────────────────────────────────

    /**
     * Parallel pointer/length arrays for the batch entry points.
     *
     * <p>All the payloads go into <em>one</em> contiguous allocation
     * and the pointer array indexes into it. The obvious version —
     * one {@code arena.allocateFrom} per item — costs n allocations
     * per batch, which is exactly the cost a batch exists to avoid,
     * and it showed up as a measurable gap against the Rust baseline
     * in the batched column of the benchmark.
     */
    private static final class Batch {
        final MemorySegment pointers;
        final MemorySegment lengths;

        Batch(Arena arena, List<byte[]> items) {
            int n = items.size();
            pointers = arena.allocate(Native.C_POINTER, n);
            lengths = arena.allocate(Native.C_SIZE_T, n);

            long total = 0;
            for (int i = 0; i < n; i++) {
                byte[] item = items.get(i);
                if (item == null) {
                    throw new IllegalArgumentException("item " + i + " is null");
                }
                total += item.length;
            }

            // One allocation for every payload in the batch.
            MemorySegment blob = total == 0 ? MemorySegment.NULL : arena.allocate(total);
            long offset = 0;
            for (int i = 0; i < n; i++) {
                byte[] item = items.get(i);
                if (item.length == 0) {
                    // An empty key is legal on the wire, and the ABI
                    // reads no bytes for length 0.
                    pointers.setAtIndex(Native.C_POINTER, i, MemorySegment.NULL);
                } else {
                    MemorySegment slice = blob.asSlice(offset, item.length);
                    MemorySegment.copy(item, 0, slice, Native.C_CHAR, 0, item.length);
                    pointers.setAtIndex(Native.C_POINTER, i, slice);
                    offset += item.length;
                }
                lengths.setAtIndex(Native.C_SIZE_T, i, item.length);
            }
        }
    }

    /**
     * Copy a Java array into native memory.
     *
     * <p>Not optional: the GC moves heap arrays, so their addresses
     * cannot be handed to native code. The arena's memory does not
     * move, and the C ABI only reads it for the duration of the call.
     */
    private static MemorySegment copy(Arena arena, byte[] data) {
        if (data.length == 0) {
            // An empty key is legal on the wire, and the ABI reads no
            // bytes for length 0.
            return MemorySegment.NULL;
        }
        return arena.allocateFrom(Native.C_CHAR, data);
    }

    private static byte[] readBuf(MemorySegment buf) {
        MemorySegment data = buf.get(Native.C_POINTER, Native.BUF_DATA_OFFSET);
        long len = buf.get(Native.C_SIZE_T, Native.BUF_LEN_OFFSET);
        return Native.bytes(data, len);
    }

    private static void checkKey(byte[] key) {
        if (key == null) {
            throw new IllegalArgumentException("key must not be null");
        }
        if (key.length > ThunderstrikeCode.MAX_KEY_LENGTH) {
            throw new ThunderstrikeException(
                    ThunderstrikeCode.KEY_TOO_LONG,
                    "key is " + key.length + " bytes; the wire's key_len field is 16 bits");
        }
    }

    private static byte[] utf8(String s) {
        if (s == null) {
            throw new IllegalArgumentException("must not be null");
        }
        return s.getBytes(StandardCharsets.UTF_8);
    }

    private static long millis(Duration d) {
        return d.isNegative() || d.isZero() ? 0L : d.toMillis();
    }
}
