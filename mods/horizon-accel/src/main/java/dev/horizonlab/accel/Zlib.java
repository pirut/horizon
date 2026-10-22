package dev.horizonlab.accel;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;

/**
 * zlib streams for byte arrays through {@link HorizonAccel#deflate} and {@link HorizonAccel#inflate}, with a
 * native scratch buffer per thread that grows to the largest chunk it has seen, so a call costs two copies and
 * no allocation of native memory.
 */
public final class Zlib {
    /** Above this a stream is left to the JDK: 64 MiB, far over any chunk vanilla writes inline or to a .mcc file. */
    static final long MAX_INFLATED = 64L << 20;

    private final HorizonAccel accel;
    private final ThreadLocal<Scratch> scratch = ThreadLocal.withInitial(Scratch::new);

    public Zlib(HorizonAccel accel) {
        this.accel = accel;
    }

    /** {@code data[offset, offset + length)} as one zlib stream at {@code Deflater}'s default level. */
    public byte[] deflate(byte[] data, int offset, int length) {
        long bound = accel.deflateBound(length);
        MemorySegment memory = scratch.get().take(length + bound);
        MemorySegment in = memory.asSlice(0, length);
        MemorySegment out = memory.asSlice(length, bound);
        MemorySegment.copy(data, offset, in, ValueLayout.JAVA_BYTE, 0, length);
        long written = accel.deflate(in, out);
        if (written < 0) {
            throw new IllegalStateException("horizon-accel: deflate overflowed its bound");
        }
        return out.asSlice(0, written).toArray(ValueLayout.JAVA_BYTE);
    }

    /**
     * The zlib stream at the start of {@code data[offset, offset + length)}, decompressed; null when it is not a
     * whole zlib stream, so the caller reads it with the JDK and its errors stay the JDK's.
     */
    public byte[] inflate(byte[] data, int offset, int length) {
        long guess = Math.max(64 << 10, 8L * length);
        while (true) {
            long cap = Math.min(guess, MAX_INFLATED);
            MemorySegment memory = scratch.get().take(length + cap);
            MemorySegment in = memory.asSlice(0, length);
            MemorySegment out = memory.asSlice(length, cap);
            MemorySegment.copy(data, offset, in, ValueLayout.JAVA_BYTE, 0, length);
            long written = accel.inflate(in, out);
            if (written >= 0) {
                return out.asSlice(0, written).toArray(ValueLayout.JAVA_BYTE);
            }
            if (written != -1 || cap == MAX_INFLATED) {
                return null;
            }
            guess = cap * 4;
        }
    }

    /** A thread's native buffer; it only grows, and the garbage collector frees the ones outgrown. */
    private static final class Scratch {
        private MemorySegment memory = MemorySegment.NULL;

        MemorySegment take(long size) {
            if (memory.byteSize() < size) {
                memory = Arena.ofAuto().allocate(Math.max(size, 256 << 10), 64);
            }
            return memory;
        }
    }
}
