package dev.horizonlab.accel;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.zip.DeflaterOutputStream;
import java.util.zip.InflaterInputStream;

/**
 * {@link Zlib} against the JDK's zlib on real chunks: each way's stream decodes to the same bytes with the other's
 * reader, and how long each takes.
 *
 * <pre>java --enable-native-access=ALL-UNNAMED -cp &lt;classes&gt; dev.horizonlab.accel.ZlibBench &lt;libhorizon_accel.so&gt; &lt;region folder&gt;</pre>
 *
 * The region folder is a saved world's {@code region/}: every zlib chunk in its {@code .mca} files is used.
 */
public final class ZlibBench {
    public static void main(String[] args) throws IOException {
        Zlib zlib = new Zlib(HorizonAccel.load(Path.of(args[0])));
        List<byte[]> chunks = new ArrayList<>();
        try (var files = Files.list(Path.of(args[1]))) {
            for (Path file : files.filter(f -> f.toString().endsWith(".mca")).sorted().toList()) {
                chunks.addAll(chunks(Files.readAllBytes(file)));
            }
        }
        long raw = 0, jdkSize = 0, rustSize = 0;
        for (byte[] chunk : chunks) {
            byte[] ours = zlib.deflate(chunk, 0, chunk.length);
            byte[] theirs = jdkDeflate(chunk);
            check(Arrays.equals(jdkInflate(ours), chunk), "the JDK reads Horizon's stream back");
            check(Arrays.equals(zlib.inflate(theirs, 0, theirs.length), chunk), "Horizon reads the JDK's stream back");
            raw += chunk.length;
            jdkSize += theirs.length;
            rustSize += ours.length;
        }
        check(zlib.inflate(new byte[] {1, 2, 3}, 0, 3) == null, "not zlib is left to the JDK");
        System.out.printf("%d chunks, %d bytes each: JDK %d bytes compressed, Horizon %d (%+.1f %%)%n", chunks.size(),
                raw / chunks.size(), jdkSize / chunks.size(), rustSize / chunks.size(),
                100.0 * (rustSize - jdkSize) / jdkSize);
        List<byte[]> jdkStreams = chunks.stream().map(ZlibBench::jdkDeflate).toList();
        for (int round = 0; round < 5; round++) {
            long a = System.nanoTime();
            for (byte[] c : chunks) jdkDeflate(c);
            long b = System.nanoTime();
            for (byte[] c : chunks) zlib.deflate(c, 0, c.length);
            long c0 = System.nanoTime();
            for (byte[] s : jdkStreams) jdkInflate(s);
            long d = System.nanoTime();
            for (byte[] s : jdkStreams) zlib.inflate(s, 0, s.length);
            long e = System.nanoTime();
            System.out.printf("round %d, ms/chunk: deflate JDK %.3f, Horizon %.3f; inflate JDK %.3f, Horizon %.3f%n",
                    round, (b - a) / 1e6 / chunks.size(), (c0 - b) / 1e6 / chunks.size(),
                    (d - c0) / 1e6 / chunks.size(), (e - d) / 1e6 / chunks.size());
        }
        System.out.println("ZlibBench: ok");
    }

    /** The zlib-compressed (type 2) chunks of one region file, decompressed. */
    static List<byte[]> chunks(byte[] region) throws IOException {
        List<byte[]> out = new ArrayList<>();
        for (int i = 0; i < 1024; i++) {
            int offset = readInt(region, i * 4) >>> 8;
            if (offset == 0) continue;
            int at = offset * 4096, length = readInt(region, at);
            if (region[at + 4] != 2) continue;
            out.add(jdkInflate(Arrays.copyOfRange(region, at + 5, at + 4 + length)));
        }
        return out;
    }

    static int readInt(byte[] b, int at) {
        return (b[at] & 255) << 24 | (b[at + 1] & 255) << 16 | (b[at + 2] & 255) << 8 | b[at + 3] & 255;
    }

    /** As {@code RegionFileVersion.VERSION_DEFLATE} writes. */
    static byte[] jdkDeflate(byte[] data) {
        var bytes = new ByteArrayOutputStream();
        try (var out = new DeflaterOutputStream(bytes)) {
            out.write(data);
        } catch (IOException e) {
            throw new IllegalStateException(e);
        }
        return bytes.toByteArray();
    }

    static byte[] jdkInflate(byte[] stream) {
        try (var in = new InflaterInputStream(new ByteArrayInputStream(stream))) {
            return in.readAllBytes();
        } catch (IOException e) {
            throw new IllegalStateException(e);
        }
    }

    static void check(boolean ok, String what) {
        if (!ok) throw new AssertionError(what);
    }
}
