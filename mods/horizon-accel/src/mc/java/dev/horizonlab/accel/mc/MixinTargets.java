package dev.horizonlab.accel.mc;

import java.io.IOException;
import java.io.InputStream;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeSet;
import org.objectweb.asm.AnnotationVisitor;
import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;

/**
 * The mixins applied to a class, each with the methods of that class it changes: the {@code method} of its
 * injectors (Mixin's and MixinExtras' {@code @Inject}, {@code @Redirect}, {@code @Modify*}, {@code @Wrap*}), and
 * the methods it {@code @Overwrite}s. Read from each mixin's class file, as {@code horizon-port-pack reach} reads
 * mod jars: annotations only, never code.
 */
public final class MixinTargets {
    private MixinTargets() {}

    /** {@code target} is an internal name ({@code net/minecraft/...}). */
    public static Map<String, Set<String>> of(String target) {
        Map<String, Set<String>> mixins = new HashMap<>();
        var info = org.spongepowered.asm.mixin.transformer.ClassInfo.fromCache(target);
        if (info == null) {
            return mixins;
        }
        for (var mixin : info.getAppliedMixins()) {
            mixins.put(mixin.getClassName(), methods(mixin.getClassName()));
        }
        return mixins;
    }

    /** The target methods one mixin class names; unreadable, it counts as changing everything. */
    static Set<String> methods(String mixin) {
        Set<String> methods = new TreeSet<>();
        String resource = mixin.replace('.', '/') + ".class";
        try (InputStream in = MixinTargets.class.getClassLoader().getResourceAsStream(resource)) {
            if (in == null) {
                return Set.of("*");
            }
            new ClassReader(in).accept(new ClassVisitor(Opcodes.ASM9) {
                @Override
                public MethodVisitor visitMethod(int access, String name, String descriptor, String signature,
                        String[] exceptions) {
                    return new MethodVisitor(Opcodes.ASM9) {
                        @Override
                        public AnnotationVisitor visitAnnotation(String type, boolean visible) {
                            if (type.equals("Lorg/spongepowered/asm/mixin/Overwrite;")) {
                                methods.add(name);
                                return null;
                            }
                            if (!type.startsWith("Lorg/spongepowered/asm/mixin/injection/")
                                    && !type.startsWith("Lcom/llamalad7/mixinextras/injector/")) {
                                return null;
                            }
                            return new AnnotationVisitor(Opcodes.ASM9) {
                                @Override
                                public AnnotationVisitor visitArray(String key) {
                                    if (!key.equals("method")) {
                                        return null;
                                    }
                                    return new AnnotationVisitor(Opcodes.ASM9) {
                                        @Override
                                        public void visit(String unused, Object value) {
                                            methods.add(methodName((String) value));
                                        }
                                    };
                                }
                            };
                        }
                    };
                }
            }, ClassReader.SKIP_CODE);
        } catch (IOException | RuntimeException e) {
            return Set.of("*");
        }
        return methods;
    }

    /** {@code "doFill(Lnet/...;)V"} or {@code "Lowner;doFill"} to {@code doFill}; {@code "*"} (every method) stays. */
    static String methodName(String selector) {
        String name = selector;
        int owner = name.indexOf(';'), arguments = name.indexOf('(');
        if (name.startsWith("L") && owner > 0 && (arguments < 0 || owner < arguments)) {
            name = name.substring(owner + 1);
        }
        for (String end : List.of("(", "*")) {
            int at = name.indexOf(end);
            if (at > 0) {
                name = name.substring(0, at);
            }
        }
        return name;
    }
}
