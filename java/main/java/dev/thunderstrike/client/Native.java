package dev.thunderstrike.client;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemoryLayout;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;

/**
 * The FFM (Project Panama) layer: method handles for every function in
 * {@code thunderstrike_client.h}.
 *
 * <p>No JNI, no glue library — this calls the same C ABI that the C,
 * Go, Python and .NET bindings use. The only native artifact is
 * {@code libthunderstrike_client}, which those bindings already ship.
 *
 * <p>Package-private on purpose: {@link ThunderstrikeClient} is the API.
 */
final class Native {

    private Native() {}

    // ── Layouts ────────────────────────────────────────────────────

    static final ValueLayout.OfInt C_INT = ValueLayout.JAVA_INT;
    static final ValueLayout.OfLong C_LONG_LONG = ValueLayout.JAVA_LONG;
    static final ValueLayout.OfShort C_SHORT = ValueLayout.JAVA_SHORT;
    static final ValueLayout.OfByte C_CHAR = ValueLayout.JAVA_BYTE;
    static final java.lang.foreign.AddressLayout C_POINTER = ValueLayout.ADDRESS;

    /**
     * {@code size_t}. Panama has no size_t layout, so this is the
     * 64-bit spelling — correct on every platform the client ships a
     * binary for. A 32-bit JVM would need JAVA_INT here; there is a
     * guard for that in {@link #checkPointerWidth()}.
     */
    static final ValueLayout.OfLong C_SIZE_T = ValueLayout.JAVA_LONG;

    /** {@code thunderstrike_buf { const uint8_t *data; size_t len; void *owner; }} */
    static final MemoryLayout BUF_LAYOUT = MemoryLayout.structLayout(
            C_POINTER.withName("data"),
            C_SIZE_T.withName("len"),
            C_POINTER.withName("owner"));

    static final long BUF_DATA_OFFSET = 0;
    static final long BUF_LEN_OFFSET = C_POINTER.byteSize();

    /** {@code thunderstrike_stats} — eleven uint64 fields. */
    static final MemoryLayout STATS_LAYOUT = MemoryLayout.structLayout(
            C_LONG_LONG.withName("connections"),
            C_LONG_LONG.withName("connections_ready"),
            C_LONG_LONG.withName("inflight"),
            C_LONG_LONG.withName("capacity"),
            C_LONG_LONG.withName("requests"),
            C_LONG_LONG.withName("responses"),
            C_LONG_LONG.withName("bytes_written"),
            C_LONG_LONG.withName("bytes_read"),
            C_LONG_LONG.withName("write_batches"),
            C_LONG_LONG.withName("reconnects"),
            C_LONG_LONG.withName("orphan_responses"),
            // The I/O pool. Present even if unread: the native side
            // writes the whole struct, and a short layout is an
            // out-of-bounds write, not a missing field.
            C_LONG_LONG.withName("io_threads"),
            C_LONG_LONG.withName("io_threads_min"),
            C_LONG_LONG.withName("io_threads_max"),
            C_LONG_LONG.withName("io_grew"),
            C_LONG_LONG.withName("io_shrank"),
            C_LONG_LONG.withName("io_moved"));

    /** Signature of {@code thunderstrike_callback}. */
    static final FunctionDescriptor CALLBACK_DESC = FunctionDescriptor.ofVoid(
            C_POINTER, C_INT, C_POINTER, C_SIZE_T);

    // ── Library loading ────────────────────────────────────────────

    private static final Linker LINKER = Linker.nativeLinker();
    private static final SymbolLookup LOOKUP = load();

    private static void checkPointerWidth() {
        if (C_POINTER.byteSize() != 8) {
            throw new UnsupportedOperationException(
                    "thunderstrike: this binding assumes 64-bit pointers (size_t is bound to "
                            + "JAVA_LONG); rebuild with C_SIZE_T = JAVA_INT for a 32-bit JVM");
        }
    }

    /**
     * Find and load {@code libthunderstrike_client}.
     *
     * <p>Order: an explicit {@code thunderstrike.client.lib} system property,
     * then {@code THUNDERSTRIKE_CLIENT_LIB}, then the java.library.path, then
     * a few build-output directories so a checkout works without any
     * setup. Same search the Python and .NET bindings do.
     */
    private static SymbolLookup load() {
        checkPointerWidth();
        // Arena.global(): the library stays mapped for the life of the
        // JVM, which is what every caller wants and what avoids a
        // use-after-unload if someone drops their last client.
        Arena arena = Arena.global();

        List<String> tried = new ArrayList<>();
        for (Path candidate : candidates()) {
            if (candidate == null || !Files.isRegularFile(candidate)) {
                if (candidate != null) {
                    tried.add(candidate.toString());
                }
                continue;
            }
            try {
                return SymbolLookup.libraryLookup(candidate, arena);
            } catch (IllegalArgumentException e) {
                tried.add(candidate + ": " + e.getMessage());
            }
        }
        // Packaged in the jar? Extract and load it. This is what
        // makes the deliverable a single artifact: `mvn dependency`
        // pulls one jar and the native library rides inside it.
        Path extracted = extractFromJar();
        if (extracted != null) {
            try {
                return SymbolLookup.libraryLookup(extracted, arena);
            } catch (IllegalArgumentException e) {
                tried.add(extracted + " (from the jar): " + e.getMessage());
            }
        } else {
            tried.add("/native/" + platformDir() + "/" + libraryFileName() + " (not in the jar)");
        }

        // Last resort: whatever the loader can find by name.
        try {
            System.loadLibrary("thunderstrike_client");
            return SymbolLookup.loaderLookup();
        } catch (UnsatisfiedLinkError e) {
            tried.add("java.library.path: " + e.getMessage());
        }
        throw new ExceptionInInitializerError(
                "thunderstrike: could not load the native library; this jar carries none for "
                        + platformDir() + ".\n"
                        + "Point -Dthunderstrike.client.lib=... or THUNDERSTRIKE_CLIENT_LIB at one.\n"
                        + "Tried:\n  " + String.join("\n  ", tried));
    }

    private static List<Path> candidates() {
        List<Path> out = new ArrayList<>();
        String explicit = System.getProperty("thunderstrike.client.lib");
        if (explicit == null) {
            explicit = System.getenv("THUNDERSTRIKE_CLIENT_LIB");
        }
        if (explicit != null && !explicit.isBlank()) {
            out.add(Path.of(explicit));
        }

        String name = libraryFileName();
        // Walk up from the working directory: a checkout has the
        // library under target/{release,debug}.
        Path dir = Path.of("").toAbsolutePath();
        for (int i = 0; i < 6 && dir != null; i++) {
            out.add(dir.resolve("target").resolve("release").resolve(name));
            out.add(dir.resolve("target").resolve("debug").resolve(name));
            dir = dir.getParent();
        }
        return out;
    }

    /**
     * Unpack the bundled native library to a temp file.
     *
     * <p>A shared library has to be a real file for the loader to map
     * it, so a jar-packaged one must be written out first. Returns
     * {@code null} when the jar carries no library for this platform.
     */
    private static Path extractFromJar() {
        String resource = "/native/" + platformDir() + "/" + libraryFileName();
        try (java.io.InputStream in = Native.class.getResourceAsStream(resource)) {
            if (in == null) {
                return null;
            }
            Path dir = Files.createTempDirectory("thunderstrike-client");
            Path out = dir.resolve(libraryFileName());
            Files.copy(in, out);
            // Best effort: Windows will not delete a mapped library,
            // so the directory may outlive the JVM. It is a few MB in
            // the temp directory, not a leak worth failing over.
            out.toFile().deleteOnExit();
            dir.toFile().deleteOnExit();
            return out;
        } catch (java.io.IOException e) {
            return null;
        }
    }

    /** {@code <os>-<arch>}, matching the layout the build script writes. */
    static String platformDir() {
        String os = System.getProperty("os.name", "").toLowerCase();
        String arch = System.getProperty("os.arch", "").toLowerCase();
        String osPart = os.contains("win") ? "windows"
                : (os.contains("mac") || os.contains("darwin")) ? "darwin"
                : os.contains("linux") ? "linux"
                : os.replaceAll("[^a-z0-9]", "");
        String archPart = switch (arch) {
            case "amd64", "x86_64" -> "x86_64";
            case "aarch64", "arm64" -> "aarch64";
            default -> arch;
        };
        return osPart + "-" + archPart;
    }

    static String libraryFileName() {
        String os = System.getProperty("os.name", "").toLowerCase();
        if (os.contains("win")) {
            return "thunderstrike_client.dll";
        }
        if (os.contains("mac") || os.contains("darwin")) {
            return "libthunderstrike_client.dylib";
        }
        return "libthunderstrike_client.so";
    }

    // ── Handle construction ────────────────────────────────────────

    private static MethodHandle handle(String symbol, FunctionDescriptor desc) {
        Optional<MemorySegment> addr = LOOKUP.find(symbol);
        if (addr.isEmpty()) {
            throw new ExceptionInInitializerError(
                    "thunderstrike: the native library does not export " + symbol
                            + " — it is probably an older build than this jar");
        }
        return LINKER.downcallHandle(addr.get(), desc);
    }

    static MemorySegment upcall(MethodHandle target, Arena arena) {
        return LINKER.upcallStub(target, CALLBACK_DESC, arena);
    }

    // ── Configuration ──────────────────────────────────────────────

    static final MethodHandle CONFIG_NEW =
            handle("thunderstrike_config_new", FunctionDescriptor.of(C_POINTER));
    static final MethodHandle CONFIG_FREE =
            handle("thunderstrike_config_free", FunctionDescriptor.ofVoid(C_POINTER));
    static final MethodHandle CONFIG_ADD_ENDPOINT =
            handle("thunderstrike_config_add_endpoint", FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER));
    static final MethodHandle CONFIG_SET_CLUSTER = handle(
            "thunderstrike_config_set_cluster",
            FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_SHORT, C_SHORT, C_SHORT));
    static final MethodHandle LAST_ERROR_MESSAGE =
            handle("thunderstrike_last_error_message", FunctionDescriptor.of(C_POINTER));
    static final MethodHandle CONFIG_SET_TLS = handle(
            "thunderstrike_config_set_tls",
            FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_POINTER, C_SIZE_T));
    static final MethodHandle CONFIG_SET_CONNECTIONS = handle(
            "thunderstrike_config_set_connections_per_endpoint",
            FunctionDescriptor.of(C_INT, C_POINTER, C_INT));
    static final MethodHandle CONFIG_SET_MAX_INFLIGHT =
            handle("thunderstrike_config_set_max_inflight", FunctionDescriptor.of(C_INT, C_POINTER, C_INT));
    static final MethodHandle CONFIG_SET_MAX_BUFFERED = handle(
            "thunderstrike_config_set_max_buffered_bytes",
            FunctionDescriptor.of(C_INT, C_POINTER, C_LONG_LONG));
    static final MethodHandle CONFIG_SET_MAX_RESPONSE = handle(
            "thunderstrike_config_set_max_response_bytes",
            FunctionDescriptor.of(C_INT, C_POINTER, C_LONG_LONG));
    static final MethodHandle CONFIG_SET_REQUEST_TIMEOUT = handle(
            "thunderstrike_config_set_request_timeout_ms",
            FunctionDescriptor.of(C_INT, C_POINTER, C_LONG_LONG));
    static final MethodHandle CONFIG_SET_CONNECT_TIMEOUT = handle(
            "thunderstrike_config_set_connect_timeout_ms",
            FunctionDescriptor.of(C_INT, C_POINTER, C_LONG_LONG));
    static final MethodHandle CONFIG_SET_KEEPALIVE = handle(
            "thunderstrike_config_set_tcp_keepalive_ms",
            FunctionDescriptor.of(C_INT, C_POINTER, C_LONG_LONG));
    static final MethodHandle CONFIG_SET_NODELAY =
            handle("thunderstrike_config_set_tcp_nodelay", FunctionDescriptor.of(C_INT, C_POINTER, C_INT));
    static final MethodHandle CONFIG_SET_MAX_RETRIES =
            handle("thunderstrike_config_set_max_retries", FunctionDescriptor.of(C_INT, C_POINTER, C_INT));
    static final MethodHandle CONFIG_SET_MULTIPLEXING =
            handle("thunderstrike_config_set_multiplexing", FunctionDescriptor.of(C_INT, C_POINTER, C_INT));
    static final MethodHandle CONFIG_SET_MUX_CHUNK_BYTES = handle(
            "thunderstrike_config_set_mux_chunk_bytes",
            FunctionDescriptor.of(C_INT, C_POINTER, C_LONG_LONG));
    static final MethodHandle CONFIG_SET_CLIENT_IDENTITY = handle(
            "thunderstrike_config_set_client_identity",
            FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_SIZE_T, C_POINTER, C_SIZE_T));
    static final MethodHandle CONFIG_SET_INSECURE_PLAINTEXT = handle(
            "thunderstrike_config_set_insecure_plaintext", FunctionDescriptor.of(C_INT, C_POINTER, C_INT));
    static final MethodHandle CONFIG_SET_BUSY_RETRIES = handle(
            "thunderstrike_config_set_busy_retries", FunctionDescriptor.of(C_INT, C_POINTER, C_INT));
    static final MethodHandle CONFIG_SET_BUSY_BACKOFF = handle(
            "thunderstrike_config_set_busy_backoff_ms",
            FunctionDescriptor.of(C_INT, C_POINTER, C_LONG_LONG, C_LONG_LONG));
    static final MethodHandle CONFIG_SET_BUSY_RETRY_WINDOW = handle(
            "thunderstrike_config_set_busy_retry_window_ms",
            FunctionDescriptor.of(C_INT, C_POINTER, C_LONG_LONG));
    static final MethodHandle CONFIG_SET_TIMEOUT_PER_MIB = handle(
            "thunderstrike_config_set_timeout_per_mib_ms",
            FunctionDescriptor.of(C_INT, C_POINTER, C_LONG_LONG));
    static final MethodHandle CONFIG_SET_ROUTING =
            handle("thunderstrike_config_set_routing", FunctionDescriptor.of(C_INT, C_POINTER, C_INT));
    static final MethodHandle CONFIG_SET_WORKER_THREADS = handle(
            "thunderstrike_config_set_worker_threads", FunctionDescriptor.of(C_INT, C_POINTER, C_INT));

    // ── Lifecycle ──────────────────────────────────────────────────

    static final MethodHandle CONNECT =
            handle("thunderstrike_connect", FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER));
    static final MethodHandle CLIENT_FREE =
            handle("thunderstrike_client_free", FunctionDescriptor.ofVoid(C_POINTER));

    // ── Blocking operations ────────────────────────────────────────

    static final MethodHandle GET = handle(
            "thunderstrike_get",
            FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_SIZE_T, C_POINTER));
    static final MethodHandle GET_INTO = handle(
            "thunderstrike_get_into",
            FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_SIZE_T, C_POINTER, C_SIZE_T, C_POINTER));
    static final MethodHandle SET_TTL = handle(
            "thunderstrike_set_ttl",
            FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_SIZE_T, C_POINTER, C_SIZE_T, C_INT));
    static final MethodHandle DELETE =
            handle("thunderstrike_delete", FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_SIZE_T));
    static final MethodHandle EXISTS =
            handle("thunderstrike_exists", FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_SIZE_T));
    static final MethodHandle APPEND = handle(
            "thunderstrike_append",
            FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_SIZE_T, C_POINTER, C_SIZE_T, C_POINTER));
    static final MethodHandle PING =
            handle("thunderstrike_ping", FunctionDescriptor.of(C_INT, C_POINTER));

    // ── Batches ────────────────────────────────────────────────────

    static final MethodHandle MGET = handle(
            "thunderstrike_mget",
            FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_POINTER, C_SIZE_T, C_POINTER, C_POINTER));
    static final MethodHandle MSET = handle(
            "thunderstrike_mset",
            FunctionDescriptor.of(
                    C_INT, C_POINTER, C_POINTER, C_POINTER, C_POINTER, C_POINTER, C_SIZE_T, C_POINTER));
    static final MethodHandle MDELETE = handle(
            "thunderstrike_mdelete",
            FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_POINTER, C_SIZE_T, C_POINTER));

    // ── Callback operations ────────────────────────────────────────

    static final MethodHandle GET_ASYNC = handle(
            "thunderstrike_get_async",
            FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_SIZE_T, C_POINTER, C_POINTER));
    static final MethodHandle SET_ASYNC = handle(
            "thunderstrike_set_async",
            FunctionDescriptor.of(
                    C_INT, C_POINTER, C_POINTER, C_SIZE_T, C_POINTER, C_SIZE_T, C_POINTER, C_POINTER));
    static final MethodHandle DELETE_ASYNC = handle(
            "thunderstrike_delete_async",
            FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_SIZE_T, C_POINTER, C_POINTER));

    // ── Buffers and diagnostics ────────────────────────────────────

    static final MethodHandle BUF_FREE =
            handle("thunderstrike_buf_free", FunctionDescriptor.ofVoid(C_POINTER));
    static final MethodHandle GET_STATS =
            handle("thunderstrike_get_stats", FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER));
    static final MethodHandle GET_MUX_STATS = handle(
            "thunderstrike_get_mux_stats",
            FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_POINTER, C_POINTER));
    static final MethodHandle SHARD_OF = handle(
            "thunderstrike_shard_of",
            FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER, C_SIZE_T, C_POINTER));
    static final MethodHandle HASH_SHARD = handle(
            "thunderstrike_hash_shard",
            FunctionDescriptor.of(C_INT, C_POINTER, C_SIZE_T, C_SHORT, C_POINTER));
    static final MethodHandle SHARD_COUNT = handle(
            "thunderstrike_shard_count", FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER));
    static final MethodHandle ENDPOINT_COUNT = handle(
            "thunderstrike_endpoint_count", FunctionDescriptor.of(C_INT, C_POINTER, C_POINTER));
    static final MethodHandle RETRY_CLASS =
            handle("thunderstrike_retry_class", FunctionDescriptor.of(C_INT, C_INT));
    static final MethodHandle STRERROR =
            handle("thunderstrike_strerror", FunctionDescriptor.of(C_POINTER, C_INT));
    static final MethodHandle VERSION =
            handle("thunderstrike_version", FunctionDescriptor.of(C_POINTER));

    // ── Helpers ────────────────────────────────────────────────────

    /** Read a NUL-terminated C string returned by the library. */
    static String cString(MemorySegment p) {
        if (p == null || p.equals(MemorySegment.NULL)) {
            return "";
        }
        // The returned pointer is zero-length until reinterpreted:
        // the library owns the memory and it is a static string.
        return p.reinterpret(Long.MAX_VALUE).getString(0);
    }

    /** Copy {@code len} bytes out of a native pointer. */
    static byte[] bytes(MemorySegment p, long len) {
        if (len <= 0 || p.equals(MemorySegment.NULL)) {
            return new byte[0];
        }
        if (len > Integer.MAX_VALUE - 8) {
            // Java's limit, not the server's: a value of 2 GiB or more
            // cannot be one byte[]. Not RESPONSE_TOO_LARGE, which is a
            // status the server sends.
            throw new ThunderstrikeException(
                    ThunderstrikeCode.BUFFER_TOO_SMALL,
                    "value of " + len + " bytes does not fit in a Java array");
        }
        return p.reinterpret(len).toArray(C_CHAR);
    }
}
