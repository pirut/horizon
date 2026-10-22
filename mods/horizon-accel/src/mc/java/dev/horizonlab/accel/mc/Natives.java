package dev.horizonlab.accel.mc;

import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.HexFormat;
import java.util.Locale;

/**
 * Where horizon-accel finds its Rust library and the prepared resources when the system properties don't say, so the
 * jar works dropped into {@code mods/} with nothing on the command line. The jar carries the library for each
 * platform it was built for under {@code natives/<os>-<arch>/}; the one for this platform is unpacked to
 * {@code <game dir>/horizon-accel/<sha256 prefix>/}, a new folder per build, so a running game never has its library
 * replaced underneath it.
 */
final class Natives {
    private Natives() {
    }

    /** This platform's folder name under {@code natives/}: {@code macos-aarch64}, {@code linux-x86_64}, ... */
    static String platform() {
        String os = System.getProperty("os.name", "").toLowerCase(Locale.ROOT);
        String arch = System.getProperty("os.arch", "").toLowerCase(Locale.ROOT);
        String system = os.startsWith("mac") ? "macos" : os.startsWith("windows") ? "windows" : "linux";
        String machine = arch.equals("amd64") || arch.equals("x86_64") ? "x86_64"
                : arch.equals("aarch64") || arch.equals("arm64") ? "aarch64" : arch;
        return system + "-" + machine;
    }

    /** The library's file name on this platform. */
    static String fileName() {
        String platform = platform();
        return platform.startsWith("macos") ? "libhorizon_accel.dylib"
                : platform.startsWith("windows") ? "horizon_accel.dll" : "libhorizon_accel.so";
    }

    /**
     * The library to load: {@code property} when set, else the one this jar carries (unpacked under {@code home}),
     * else {@code home/<file name>}; null when there is none.
     */
    static Path library(String property, Path home) throws IOException {
        if (property != null) {
            Path path = Path.of(property);
            return Files.isRegularFile(path) ? path : null;
        }
        String name = "/natives/" + platform() + "/" + fileName();
        try (InputStream in = Natives.class.getResourceAsStream(name)) {
            if (in != null) {
                byte[] bytes = in.readAllBytes();
                Path path = home.resolve(sha256(bytes).substring(0, 16)).resolve(fileName());
                if (!Files.isRegularFile(path) || Files.size(path) != bytes.length) {
                    Files.createDirectories(path.getParent());
                    Path partial = Files.createTempFile(path.getParent(), "lib", ".part");
                    Files.write(partial, bytes);
                    Files.move(partial, path, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
                }
                return path;
            }
        }
        Path path = home.resolve(fileName());
        return Files.isRegularFile(path) ? path : null;
    }

    /** The resources: {@code property} when set, else {@code home/resources}; null unless it holds facts.json. */
    static Path resources(String property, Path home) {
        Path path = property != null ? Path.of(property) : home.resolve("resources");
        return Files.isRegularFile(path.resolve("facts.json")) && Files.isDirectory(path.resolve("worldgen")) ? path
                : null;
    }

    /** What to set, or where to put files, for whichever of the two is missing. */
    static String missing(Path library, Path resources, Path home) {
        StringBuilder text = new StringBuilder();
        if (library == null) {
            text.append("This jar has no library for ").append(platform()).append(": put ").append(fileName())
                    .append(" in ").append(home).append(" or set -Dhorizon.accel.library. ");
        }
        if (resources == null) {
            text.append("No prepared resources: put horizon-prepare's facts.json and worldgen/ in ")
                    .append(home.resolve("resources")).append(" or set -Dhorizon.accel.resources.");
        }
        return text.toString().trim();
    }

    private static String sha256(byte[] bytes) {
        try {
            return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(bytes));
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException(e);
        }
    }
}
