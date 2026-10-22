package dev.horizonlab.accel.mc;

import dev.horizonlab.accel.HorizonAccel;
import dev.horizonlab.accel.Swaps;
import dev.horizonlab.accel.Zlib;
import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelHandlerContext;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.FilterOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.zip.InflaterInputStream;
import net.minecraft.network.VarInt;

/**
 * zlib through Rust (zlib-rs, {@code crates/horizon-accel/src/zlib.rs}) where vanilla uses the JDK's zlib: chunks
 * saved to and loaded from region files ({@code RegionFileVersion.VERSION_DEFLATE}, the default), and packets over
 * the network threshold ({@code CompressionEncoder}, {@code CompressionDecoder}). zlib-rs compresses a chunk about
 * 2.6 times and decompresses it 2.4 times as fast, at the same level.
 *
 * <p>The decompressed bytes are always vanilla's. The compressed bytes are a valid zlib stream of the same data at
 * the same level (about 2 % larger), not the JDK zlib's exact bytes, which nothing reads: any zlib reader, vanilla's
 * included, gets the same data back. A stream Rust can't read (corrupt, truncated) goes to the JDK's {@code Inflater}
 * whole, so errors and partial reads stay vanilla's.
 *
 * <p>Off when another mod patches the methods involved (a mod with its own native compression, say), or with
 * {@code -Dhorizon.accel.zlib=false}.
 */
public final class FastZlib {
    static final Swaps.Swap REGION = new Swaps.Swap("region-zlib",
            "net/minecraft/world/level/chunk/storage/RegionFileVersion", Set.of("wrap", "lambda$static$2", "lambda$static$3"));
    static final Swaps.Swap ENCODE = new Swaps.Swap("packet-deflate", "net/minecraft/network/CompressionEncoder",
            Set.of("encode"));
    static final Swaps.Swap DECODE = new Swaps.Swap("packet-inflate", "net/minecraft/network/CompressionDecoder",
            Set.of("decode", "setupInflaterInput", "inflate"));
    private static volatile Zlib zlib;
    private static volatile Boolean regionOn, encodeOn, decodeOn;
    private static final ThreadLocal<MemorySegment[]> SCRATCH = ThreadLocal.withInitial(() -> new MemorySegment[] {
            MemorySegment.NULL});

    private FastZlib() {}

    private static Zlib zlib() {
        Zlib z = zlib;
        if (z == null) {
            HorizonAccel library = AccelMod.library();
            if (library == null) return null;
            synchronized (FastZlib.class) {
                if (zlib == null) zlib = new Zlib(library);
                z = zlib;
            }
        }
        return z;
    }

    private static boolean decide(Swaps.Swap swap, String what) {
        var decision = Swaps.decide(swap, MixinTargets.of(swap.targetClass()), "dev.horizonlab.accel.", Set.of(), Map.of());
        if (!Boolean.parseBoolean(System.getProperty("horizon.accel.zlib", "true"))) {
            decision = new Swaps.Decision(swap, List.of("horizon.accel.zlib=false"));
        } else if (AccelMod.library() == null) {
            decision = new Swaps.Decision(swap, List.of("the library isn't loaded"));
        }
        AccelMod.LOG.info("horizon-accel: {}", decision.on() ? "Rust zlib for " + what : "vanilla zlib for " + what + ": "
                + String.join("; ", decision.reasons()));
        return decision.on();
    }

    private static boolean regionOn() {
        Boolean on = regionOn;
        if (on == null) {
            synchronized (FastZlib.class) {
                if (regionOn == null) regionOn = decide(REGION, "chunk saves and loads");
                on = regionOn;
            }
        }
        return on;
    }

    private static boolean encodeOn() {
        Boolean on = encodeOn;
        if (on == null) {
            synchronized (FastZlib.class) {
                if (encodeOn == null) encodeOn = decide(ENCODE, "outgoing packets");
                on = encodeOn;
            }
        }
        return on;
    }

    private static boolean decodeOn() {
        Boolean on = decodeOn;
        if (on == null) {
            synchronized (FastZlib.class) {
                if (decodeOn == null) decodeOn = decide(DECODE, "incoming packets");
                on = decodeOn;
            }
        }
        return on;
    }

    /** What {@code VERSION_DEFLATE.wrap(out)} returns here, or null to run vanilla's. */
    public static OutputStream deflating(OutputStream out) {
        return regionOn() ? new Deflating(out, zlib()) : null;
    }

    /** What {@code VERSION_DEFLATE.wrap(in)} returns here, or null to run vanilla's. */
    public static InputStream inflating(InputStream in) throws IOException {
        if (!regionOn()) return null;
        byte[] stream;
        try (in) {
            stream = in.readAllBytes();
        }
        byte[] data = zlib().inflate(stream, 0, stream.length);
        return data != null ? new ByteArrayInputStream(data)
                : new InflaterInputStream(new ByteArrayInputStream(stream));
    }

    /**
     * Collects a chunk's bytes and writes them compressed when closed, as {@code DeflaterOutputStream} does (its
     * {@code flush} doesn't sync-flush either).
     */
    private static final class Deflating extends FilterOutputStream {
        private final Zlib zlib;
        private final Buffer buffer = new Buffer();
        private boolean closed;

        Deflating(OutputStream out, Zlib zlib) {
            super(out);
            this.zlib = zlib;
        }

        @Override
        public void write(int b) throws IOException {
            open();
            buffer.write(b);
        }

        @Override
        public void write(byte[] b, int off, int len) throws IOException {
            open();
            buffer.write(b, off, len);
        }

        @Override
        public void flush() throws IOException {
            open();
            out.flush();
        }

        @Override
        public void close() throws IOException {
            if (closed) return;
            closed = true;
            try (OutputStream target = out) {
                target.write(zlib.deflate(buffer.bytes(), 0, buffer.size()));
            }
        }

        private void open() throws IOException {
            if (closed) throw new IOException("Stream closed");
        }
    }

    private static final class Buffer extends ByteArrayOutputStream {
        Buffer() {
            super(64 << 10);
        }

        byte[] bytes() {
            return buf;
        }
    }

    /**
     * {@code CompressionEncoder.encode} for a packet at or over the threshold (vanilla's own code handles the rest and
     * its errors): its size as a VarInt, then its bytes compressed. False to run vanilla's.
     */
    public static boolean encode(ByteBuf in, ByteBuf out) {
        if (!encodeOn()) return false;
        int size = in.readableBytes();
        HorizonAccel library = AccelMod.library();
        long bound = library.deflateBound(size);
        MemorySegment scratch = scratch(size + bound);
        MemorySegment src = scratch.asSlice(0, size);
        MemorySegment dst = scratch.asSlice(size, bound);
        in.getBytes(in.readerIndex(), src.asByteBuffer());
        long written = library.deflate(src, dst);
        if (written < 0) return false;
        in.skipBytes(size);
        VarInt.write(out, size);
        out.writeBytes(dst.asSlice(0, written).asByteBuffer());
        return true;
    }

    /**
     * {@code CompressionDecoder.decode}'s compressed path for a packet of {@code length} bytes once vanilla's checks
     * passed: the decompressed packet, or null to run vanilla's (which reads {@code in} from its reader index), when
     * Rust doesn't produce exactly {@code length} bytes. Vanilla's {@code Inflater} also stops at {@code length}.
     */
    public static ByteBuf decode(ChannelHandlerContext ctx, ByteBuf in, int length) {
        if (!decodeOn()) return null;
        int size = in.readableBytes();
        MemorySegment scratch = scratch((long) size + length);
        MemorySegment src = scratch.asSlice(0, size);
        MemorySegment dst = scratch.asSlice(size, length);
        in.getBytes(in.readerIndex(), src.asByteBuffer());
        long written = AccelMod.library().inflate(src, dst);
        if (written != length && written != -1) return null;
        ByteBuf output = ctx.alloc().directBuffer(length);
        output.writeBytes(dst.asByteBuffer());
        in.skipBytes(size);
        return output;
    }

    /** This thread's native buffer of at least {@code size} bytes; it only grows. */
    private static MemorySegment scratch(long size) {
        MemorySegment[] holder = SCRATCH.get();
        if (holder[0].byteSize() < size) {
            holder[0] = Arena.ofAuto().allocate(Math.max(size, 64 << 10), 64);
        }
        return holder[0];
    }
}
