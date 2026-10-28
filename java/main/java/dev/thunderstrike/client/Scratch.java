package dev.thunderstrike.client;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;

/**
 * Per-thread reusable native memory for the single-key operations.
 *
 * <p>The obvious way to call a C function from Panama is
 * {@code try (Arena a = Arena.ofConfined())}, allocate the arguments,
 * call, and let the arena close. That is correct but it puts an arena
 * construction, two allocations and a teardown on every single
 * request — which showed up in the benchmark as roughly an order of
 * magnitude more overhead per call than .NET's P/Invoke, for a
 * boundary that is nominally just as fast.
 *
 * <p>Instead each thread keeps one set of buffers and reuses them.
 * Steady state is then a {@code memcpy} into memory that is already
 * there, and nothing is allocated or freed per call.
 *
 * <h2>Why this is safe</h2>
 *
 * <ul>
 *   <li>The buffers are {@link ThreadLocal}, so no two threads ever
 *       touch the same one.</li>
 *   <li>Every native call that uses them is synchronous and completes
 *       before the method returns, so a thread cannot have two
 *       requests borrowing its scratch at once. The asynchronous
 *       entry points deliberately do <em>not</em> use this class —
 *       their arguments must outlive the call, so they get their own
 *       arena.</li>
 *   <li>{@link Arena#ofAuto()} keeps the memory alive as long as the
 *       segments are reachable and releases it when the thread and
 *       its {@code ThreadLocal} go away, so a short-lived thread does
 *       not leak.</li>
 * </ul>
 */
final class Scratch {

    /** Start big enough for ordinary keys and values; grow on demand. */
    private static final long INITIAL = 256;

    private static final ThreadLocal<Scratch> LOCAL = ThreadLocal.withInitial(Scratch::new);

    /**
     * Auto arena rather than confined: these segments outlive any one
     * call and are released when the thread's ThreadLocal becomes
     * unreachable.
     */
    private final Arena arena = Arena.ofAuto();

    private MemorySegment keyBuf;
    private MemorySegment valueBuf;

    /** Fixed-size out-parameters, allocated once instead of per call. */
    final MemorySegment bufOut;
    final MemorySegment u64Out;
    final MemorySegment u32Out;
    final MemorySegment u16Out;
    final MemorySegment sizeOut;
    final MemorySegment ptrOut;
    final MemorySegment statsOut;

    private Scratch() {
        keyBuf = arena.allocate(INITIAL);
        valueBuf = arena.allocate(INITIAL);
        bufOut = arena.allocate(Native.BUF_LAYOUT);
        u64Out = arena.allocate(Native.C_LONG_LONG);
        u32Out = arena.allocate(Native.C_INT);
        u16Out = arena.allocate(Native.C_SHORT);
        sizeOut = arena.allocate(Native.C_SIZE_T);
        ptrOut = arena.allocate(Native.C_POINTER);
        statsOut = arena.allocate(Native.STATS_LAYOUT);
    }

    static Scratch get() {
        return LOCAL.get();
    }

    /** Copy `data` into this thread's key buffer. */
    MemorySegment key(byte[] data) {
        if (data.length == 0) {
            // An empty key is legal on the wire, and the ABI reads no
            // bytes for length 0.
            return MemorySegment.NULL;
        }
        if (data.length > keyBuf.byteSize()) {
            keyBuf = arena.allocate(Math.max(data.length, keyBuf.byteSize() * 2));
        }
        MemorySegment.copy(data, 0, keyBuf, Native.C_CHAR, 0, data.length);
        return keyBuf;
    }

    /** Copy `data` into this thread's value buffer. */
    MemorySegment value(byte[] data) {
        if (data.length == 0) {
            return MemorySegment.NULL;
        }
        if (data.length > valueBuf.byteSize()) {
            valueBuf = arena.allocate(Math.max(data.length, valueBuf.byteSize() * 2));
        }
        MemorySegment.copy(data, 0, valueBuf, Native.C_CHAR, 0, data.length);
        return valueBuf;
    }

    /**
     * A scratch region of at least `n` bytes, for a caller-owned
     * destination buffer.
     */
    MemorySegment sized(long n) {
        if (n > valueBuf.byteSize()) {
            valueBuf = arena.allocate(Math.max(n, valueBuf.byteSize() * 2));
        }
        return valueBuf;
    }
}
