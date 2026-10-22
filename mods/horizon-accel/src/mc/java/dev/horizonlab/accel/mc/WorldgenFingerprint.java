package dev.horizonlab.accel.mc;

import com.mojang.serialization.Codec;
import com.mojang.serialization.JsonOps;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.HexFormat;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.RegistryDataLoader;
import net.minecraft.resources.RegistryOps;
import net.minecraft.resources.ResourceKey;

/**
 * A hash of every world-generation registry entry the terrain swap reads, as the game's own codecs encode it.
 *
 * <p>Horizon generates terrain from vanilla's data as {@code horizon-prepare} exported it, so the swap is only
 * right where the live server's data is vanilla's. A data pack or mod that adds a biome, moves a structure or edits
 * a density function changes this hash, and the swap stays off. The expected hash is taken from vanilla's own data
 * with nothing else loaded ({@code VanillaTerrainBench} prints it).
 */
public final class WorldgenFingerprint {
    /** What noise, surface, carvers and structure terrain adaptation read. */
    static final Set<ResourceKey<? extends Registry<?>>> READ = Set.of(Registries.NOISE_SETTINGS, Registries.NOISE,
            Registries.DENSITY_FUNCTION, Registries.BIOME, Registries.CARVER, Registries.STRUCTURE,
            Registries.STRUCTURE_SET, Registries.TEMPLATE_POOL, Registries.PROCESSOR_LIST,
            Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST, Registries.DIMENSION_TYPE);

    private WorldgenFingerprint() {}

    public static String of(RegistryAccess access) {
        var ops = RegistryOps.create(JsonOps.INSTANCE, access);
        Map<String, String> entries = new TreeMap<>();
        for (RegistryDataLoader.RegistryData<?> data : RegistryDataLoader.WORLD_REGISTRIES) {
            if (READ.contains(data.key())) {
                add(entries, access, data, ops);
            }
        }
        String dump = System.getProperty("horizon.accel.fingerprintDump");
        if (dump != null) {
            // Diagnostics: every entry hashed, one per line, to find what a mod or data pack changed.
            try (var out = java.nio.file.Files.newBufferedWriter(java.nio.file.Path.of(dump))) {
                for (var entry : entries.entrySet()) {
                    out.write(entry.getKey() + "\t" + entry.getValue() + "\n");
                }
            } catch (java.io.IOException e) {
                throw new java.io.UncheckedIOException(e);
            }
        }
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            entries.forEach((key, value) -> {
                digest.update(key.getBytes(StandardCharsets.UTF_8));
                digest.update((byte) 0);
                digest.update(value.getBytes(StandardCharsets.UTF_8));
                digest.update((byte) '\n');
            });
            return HexFormat.of().formatHex(digest.digest());
        } catch (java.security.NoSuchAlgorithmException e) {
            throw new IllegalStateException(e);
        }
    }

    private static <T> void add(Map<String, String> entries, RegistryAccess access, RegistryDataLoader.RegistryData<T> data,
            RegistryOps<com.google.gson.JsonElement> ops) {
        Registry<T> registry = access.lookupOrThrow(data.key());
        Codec<T> codec = data.elementCodec();
        for (var entry : registry.entrySet()) {
            String key = data.key().identifier() + "/" + entry.getKey().identifier();
            entries.put(key, codec.encodeStart(ops, entry.getValue()).mapOrElse(json -> canonical(json).toString(),
                    e -> "error " + e.message()));
        }
    }

    /**
     * Objects with their keys sorted: a codec writes a map in the map's own order, and mods swap vanilla's maps for
     * faster ones (Lithium's spawn lists), which reorders the JSON without changing a value.
     */
    static com.google.gson.JsonElement canonical(com.google.gson.JsonElement json) {
        if (json.isJsonObject()) {
            var sorted = new com.google.gson.JsonObject();
            new TreeMap<>(json.getAsJsonObject().asMap()).forEach((key, value) -> sorted.add(key, canonical(value)));
            return sorted;
        }
        if (json.isJsonArray()) {
            var array = new com.google.gson.JsonArray();
            json.getAsJsonArray().forEach(value -> array.add(canonical(value)));
            return array;
        }
        return json;
    }
}
