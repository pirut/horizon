//! What a mod's code reaches into, read from its class files without
//! decompiling (`docs/research/java-bridge.md`): the Minecraft classes and
//! members it names, the Fabric API modules and libraries it uses, and what
//! its mixins patch. Summed over many jars, this is the surface a Java mod
//! bridge has to provide, ranked by how many mods need each piece.
//!
//! Only names are read: the constant pool, the class header and the mixin
//! annotations. No method body is decoded or kept.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Cursor, Read},
    path::Path,
};

/// Packages that are the game itself: the classes a bridge must rebuild.
/// Brigadier, DataFixerUpper and the rest of `com/mojang` are open-source
/// libraries a bridge can ship as they are, so they count as libraries.
const MINECRAFT: [&str; 3] = ["net/minecraft/", "com/mojang/blaze3d/", "com/mojang/math/"];
const FABRIC_API: &str = "net/fabricmc/fabric/api/";
const JDK: [&str; 4] = ["java/", "javax/", "jdk/", "sun/"];
const MIXIN: &str = "Lorg/spongepowered/asm/mixin/Mixin;";

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reach {
    pub file: String,
    pub mod_id: String,
    /// `fabric`, `neoforge`, `forge`, or empty for a plain library jar.
    pub loader: String,
    pub classes: usize,
    /// Minecraft class to the members named on it (`name:descriptor`). A
    /// class with no members is only named in a type or a class literal.
    pub minecraft: BTreeMap<String, BTreeSet<String>>,
    /// Minecraft classes the mod's own classes extend or implement: the
    /// bridge's versions of these must be open to subclassing.
    pub extends: BTreeSet<String>,
    /// Fabric API module path (`client/rendering/v1`) to the classes used.
    pub fabric_api: BTreeMap<String, BTreeSet<String>>,
    /// Other packages named (two segments, `com/google`), by class count.
    pub libraries: BTreeMap<String, usize>,
    pub mixins: Vec<Mixin>,
    /// Jars shipped inside this one (`META-INF/jars/`), read the same way.
    pub nested: Vec<Reach>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mixin {
    pub class: String,
    pub targets: Vec<String>,
    pub injectors: Vec<Injector>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Injector {
    /// The annotation's simple name: `Inject`, `Redirect`, `Overwrite`...
    pub kind: String,
    /// The target methods as the annotation names them.
    pub methods: Vec<String>,
    /// Each `@At`: its value, then its target if it has one.
    pub at: Vec<String>,
}

impl Reach {
    /// Minecraft members named, over all classes.
    pub fn members(&self) -> usize {
        self.minecraft.values().map(BTreeSet::len).sum()
    }

    /// `data` (no code), `api` (code, no mixins) or `mixins`.
    pub fn tier(&self) -> &'static str {
        match (self.classes, self.mixins.len()) {
            (0, _) => "data",
            (_, 0) => "api",
            _ => "mixins",
        }
    }
}

pub fn read(path: &Path) -> Result<Reach, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    read_bytes(&name, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_bytes(file: &str, bytes: Vec<u8>) -> Result<Reach, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut reach = Reach {
        file: file.to_owned(),
        ..Reach::default()
    };
    let mut parsed = Vec::new();
    let mut nested = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_owned();
        let mut data = Vec::new();
        if name.ends_with(".class") || name.ends_with(".jar") || name == "fabric.mod.json" {
            entry
                .read_to_end(&mut data)
                .map_err(|e| format!("{name}: {e}"))?;
        }
        if name.ends_with(".class") && !name.starts_with("META-INF/versions/") {
            parsed.push(class(&data).map_err(|e| format!("{name}: {e}"))?);
        } else if name.ends_with(".jar") && name.starts_with("META-INF/jars/") {
            nested.push((name, data));
        } else if name == "fabric.mod.json" {
            reach.loader = "fabric".into();
            let v: serde_json::Value =
                serde_json::from_slice(&data).map_err(|e| format!("fabric.mod.json: {e}"))?;
            reach.mod_id = v["id"].as_str().unwrap_or("").to_owned();
        } else if name == "META-INF/neoforge.mods.toml" {
            reach.loader = "neoforge".into();
        } else if name == "META-INF/mods.toml" && reach.loader.is_empty() {
            reach.loader = "forge".into();
        }
    }
    reach.classes = parsed.len();
    let own: BTreeSet<&str> = parsed.iter().map(|c| c.name.as_str()).collect();
    for c in &parsed {
        for (owner, member) in &c.members {
            if is_minecraft(owner) {
                let set = reach.minecraft.entry(owner.clone()).or_default();
                set.insert(member.clone());
            }
        }
        for name in &c.names {
            if is_minecraft(name) {
                reach.minecraft.entry(name.clone()).or_default();
            } else if let Some(rest) = name.strip_prefix(FABRIC_API) {
                let (module, _) = rest.rsplit_once('/').unwrap_or(("", rest));
                let set = reach.fabric_api.entry(module.to_owned()).or_default();
                set.insert(name.clone());
            } else if !own.contains(name.as_str()) && !JDK.iter().any(|j| name.starts_with(j)) {
                let package: Vec<&str> = name.split('/').take(3).collect();
                // A class in the default package, or a package of one segment.
                if package.len() == 3 {
                    *reach.libraries.entry(package[..2].join("/")).or_default() += 1;
                }
            }
        }
        for parent in c.parents.iter().filter(|p| is_minecraft(p)) {
            reach.extends.insert(parent.clone());
        }
        if let Some(m) = &c.mixin {
            reach.mixins.push(m.clone());
        }
    }
    for (name, data) in nested {
        let file = name.rsplit('/').next().unwrap_or(&name);
        reach
            .nested
            .push(read_bytes(file, data).map_err(|e| format!("{name}: {e}"))?);
    }
    if reach.mod_id.is_empty() {
        reach.mod_id = file.trim_end_matches(".jar").to_owned();
    }
    Ok(reach)
}

/// Every `.jar` in `path` (one level), or `path` itself.
pub fn read_all(path: &Path) -> Result<Vec<Reach>, String> {
    if !path.is_dir() {
        return Ok(vec![read(path)?]);
    }
    let mut jars: Vec<_> = fs::read_dir(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jar"))
        .collect();
    jars.sort();
    jars.iter().map(|j| read(j)).collect()
}

fn is_minecraft(name: &str) -> bool {
    MINECRAFT.iter().any(|m| name.starts_with(m))
}

/// The names one class file holds.
#[derive(Debug, Default)]
struct Class {
    name: String,
    parents: Vec<String>,
    /// Every class named: class constants and the types in descriptors.
    names: BTreeSet<String>,
    /// Field and method references, `(owner, name:descriptor)`.
    members: BTreeSet<(String, String)>,
    mixin: Option<Mixin>,
}

enum Const {
    Utf8(String),
    Class(u16),
    NameAndType(u16, u16),
    Member(u16, u16),
    Other,
}

struct Bytes<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bytes<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.at + n;
        let s = self.data.get(self.at..end).ok_or("truncated class file")?;
        self.at = end;
        Ok(s)
    }
    fn u1(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u2(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u4(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// An annotation element's value, as far as mixins need it.
enum Value {
    Str(String),
    Annotation(String, Vec<(String, Value)>),
    Array(Vec<Value>),
    Other,
}

fn class(data: &[u8]) -> Result<Class, String> {
    let mut b = Bytes { data, at: 0 };
    if b.u4()? != 0xCAFE_BABE {
        return Err("not a class file".into());
    }
    b.take(4)?;
    let count = b.u2()? as usize;
    let mut pool = vec![Const::Other];
    while pool.len() < count {
        let tag = b.u1()?;
        let entry = match tag {
            1 => {
                let len = b.u2()? as usize;
                Const::Utf8(String::from_utf8_lossy(b.take(len)?).into_owned())
            }
            7 => Const::Class(b.u2()?),
            9..=11 => Const::Member(b.u2()?, b.u2()?),
            12 => Const::NameAndType(b.u2()?, b.u2()?),
            3 | 4 | 17 | 18 => {
                b.take(4)?;
                Const::Other
            }
            5 | 6 => {
                b.take(8)?;
                pool.push(Const::Other);
                Const::Other
            }
            15 => {
                b.take(3)?;
                Const::Other
            }
            8 | 16 | 19 | 20 => {
                b.take(2)?;
                Const::Other
            }
            t => return Err(format!("constant pool tag {t}")),
        };
        pool.push(entry);
    }
    let utf8 = |i: u16| match pool.get(i as usize) {
        Some(Const::Utf8(s)) => Ok(s.as_str()),
        _ => Err(format!("constant {i} is not text")),
    };
    let class_name = |i: u16| match pool.get(i as usize) {
        Some(Const::Class(n)) => utf8(*n),
        _ => Err(format!("constant {i} is not a class")),
    };

    let mut c = Class::default();
    for entry in &pool {
        match entry {
            Const::Class(n) => {
                let name = utf8(*n)?;
                if name.starts_with('[') {
                    descriptor_classes(name, &mut c.names);
                } else {
                    c.names.insert(name.to_owned());
                }
            }
            Const::Member(owner, nat) => {
                let Some(Const::NameAndType(n, d)) = pool.get(*nat as usize) else {
                    return Err(format!("constant {nat} is not a name and type"));
                };
                let owner = class_name(*owner)?;
                c.members
                    .insert((owner.to_owned(), format!("{}:{}", utf8(*n)?, utf8(*d)?)));
            }
            // Descriptors and signatures: `(Lnet/minecraft/...;)V`.
            Const::Utf8(s) if s.contains(';') => descriptor_classes(s, &mut c.names),
            _ => {}
        }
    }

    b.take(2)?;
    c.name = class_name(b.u2()?)?.to_owned();
    let parent = b.u2()?;
    if parent != 0 {
        c.parents.push(class_name(parent)?.to_owned());
    }
    for _ in 0..b.u2()? {
        c.parents.push(class_name(b.u2()?)?.to_owned());
    }

    let mut injectors = Vec::new();
    for is_method in [false, true] {
        for _ in 0..b.u2()? {
            b.take(2)?;
            let name = utf8(b.u2()?)?.to_owned();
            b.take(2)?;
            for a in annotations(&mut b, &pool)? {
                if let Some(i) = is_method.then(|| injector(&name, a)).flatten() {
                    injectors.push(i);
                }
            }
        }
    }
    for (kind, elements) in annotations(&mut b, &pool)? {
        if kind != MIXIN {
            continue;
        }
        let mut targets = Vec::new();
        for (key, value) in elements {
            if key == "value" || key == "targets" {
                strings(value, &mut targets);
            }
        }
        // `@Mixin(Gui.class)` holds a descriptor, `targets` a dotted name.
        for t in &mut targets {
            *t = t
                .strip_prefix('L')
                .and_then(|t| t.strip_suffix(';'))
                .unwrap_or(t)
                .replace('.', "/");
        }
        c.mixin = Some(Mixin {
            class: c.name.clone(),
            targets,
            injectors: std::mem::take(&mut injectors),
        });
    }
    Ok(c)
}

/// Reads an attribute table, returning the annotations in it (visible and
/// invisible: Mixin's are class-retention) and skipping everything else.
fn annotations(
    b: &mut Bytes,
    pool: &[Const],
) -> Result<Vec<(String, Vec<(String, Value)>)>, String> {
    let mut found = Vec::new();
    for _ in 0..b.u2()? {
        let name = b.u2()?;
        let len = b.u4()? as usize;
        let body = b.take(len)?;
        let is_annotations = matches!(
            pool.get(name as usize),
            Some(Const::Utf8(n)) if n == "RuntimeVisibleAnnotations" || n == "RuntimeInvisibleAnnotations"
        );
        if is_annotations {
            let mut a = Bytes { data: body, at: 0 };
            for _ in 0..a.u2()? {
                if let Value::Annotation(kind, elements) = annotation(&mut a, pool)? {
                    found.push((kind, elements));
                }
            }
        }
    }
    Ok(found)
}

fn annotation(b: &mut Bytes, pool: &[Const]) -> Result<Value, String> {
    let text = |i: u16| match pool.get(i as usize) {
        Some(Const::Utf8(s)) => Ok(s.clone()),
        _ => Err(format!("constant {i} is not text")),
    };
    let kind = text(b.u2()?)?;
    let mut elements = Vec::new();
    for _ in 0..b.u2()? {
        let key = text(b.u2()?)?;
        elements.push((key, element(b, pool)?));
    }
    Ok(Value::Annotation(kind, elements))
}

fn element(b: &mut Bytes, pool: &[Const]) -> Result<Value, String> {
    Ok(match b.u1()? {
        b's' | b'c' => match pool.get(b.u2()? as usize) {
            Some(Const::Utf8(s)) => Value::Str(s.clone()),
            _ => Value::Other,
        },
        b'e' => {
            b.take(4)?;
            Value::Other
        }
        b'@' => annotation(b, pool)?,
        b'[' => {
            let n = b.u2()?;
            Value::Array((0..n).map(|_| element(b, pool)).collect::<Result<_, _>>()?)
        }
        _ => {
            b.take(2)?;
            Value::Other
        }
    })
}

fn strings(value: Value, out: &mut Vec<String>) {
    match value {
        Value::Str(s) => out.push(s),
        Value::Array(values) => values.into_iter().for_each(|v| strings(v, out)),
        _ => {}
    }
}

const INJECTORS: [&str; 5] = [
    "Lorg/spongepowered/asm/mixin/injection/",
    "Lorg/spongepowered/asm/mixin/Overwrite;",
    "Lorg/spongepowered/asm/mixin/gen/",
    "Lcom/llamalad7/mixinextras/injector/",
    "Lorg/spongepowered/asm/mixin/Shadow;",
];

fn injector(method: &str, (kind, elements): (String, Vec<(String, Value)>)) -> Option<Injector> {
    if !INJECTORS[..4].iter().any(|p| kind.starts_with(p)) {
        return None;
    }
    let simple = kind.trim_end_matches(';').rsplit('/').next()?.to_owned();
    let mut i = Injector {
        kind: simple,
        ..Injector::default()
    };
    for (key, value) in elements {
        match key.as_str() {
            "method" => strings(value, &mut i.methods),
            // `@Accessor("field")` and `@Invoker("method")`.
            "value" if matches!(i.kind.as_str(), "Accessor" | "Invoker") => {
                strings(value, &mut i.methods)
            }
            "at" => at(value, &mut i.at),
            _ => {}
        }
    }
    if i.methods.is_empty() {
        // `@Overwrite`, and accessors named after what they reach.
        i.methods.push(method.to_owned());
    }
    Some(i)
}

fn at(value: Value, out: &mut Vec<String>) {
    match value {
        Value::Array(values) => values.into_iter().for_each(|v| at(v, out)),
        Value::Annotation(_, elements) => {
            let mut parts = Vec::new();
            for (key, v) in elements {
                if key == "value" || key == "target" {
                    strings(v, &mut parts);
                }
            }
            out.push(parts.join(" "));
        }
        _ => {}
    }
}

/// The `L...;` class names inside a descriptor or signature.
fn descriptor_classes(text: &str, out: &mut BTreeSet<String>) {
    let mut rest = text;
    while let Some(start) = rest.find('L') {
        let tail = &rest[start + 1..];
        let Some(end) = tail.find([';', '<']) else {
            break;
        };
        let name = &tail[..end];
        if name.contains('/') && !name.contains([' ', '(', ')', '[']) {
            out.insert(name.to_owned());
        }
        rest = &tail[end..];
    }
}

/// The surface over many mods: what to build first.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Summary {
    pub mods: usize,
    pub by_tier: BTreeMap<String, usize>,
    /// `owner.name:descriptor` with the number of mods naming it, most first.
    pub members: Vec<(String, usize)>,
    pub classes: Vec<(String, usize)>,
    pub fabric_api: Vec<(String, usize)>,
    /// `target` (the class) with the number of mods patching it.
    pub mixin_targets: Vec<(String, usize)>,
    /// `target.method`, by mods.
    pub injected_methods: Vec<(String, usize)>,
    pub libraries: Vec<(String, usize)>,
}

pub fn summarize(mods: &[Reach]) -> Summary {
    let mut members = BTreeMap::new();
    let mut classes = BTreeMap::new();
    let mut fabric = BTreeMap::new();
    let mut targets = BTreeMap::new();
    let mut methods = BTreeMap::new();
    let mut libraries = BTreeMap::new();
    let mut by_tier = BTreeMap::new();
    for r in mods {
        *by_tier.entry(r.tier().to_owned()).or_default() += 1;
        // A mod counts once per name, nested jars included.
        let mut seen: [BTreeSet<String>; 6] = Default::default();
        let mut stack = vec![r];
        while let Some(r) = stack.pop() {
            for (class, set) in &r.minecraft {
                seen[1].insert(class.clone());
                seen[0].extend(set.iter().map(|m| format!("{class}.{m}")));
            }
            seen[2].extend(r.fabric_api.keys().cloned());
            for m in &r.mixins {
                seen[3].extend(m.targets.iter().cloned());
                for t in &m.targets {
                    for i in &m.injectors {
                        seen[4].extend(i.methods.iter().map(|x| format!("{t}.{x}")));
                    }
                }
            }
            seen[5].extend(r.libraries.keys().cloned());
            stack.extend(&r.nested);
        }
        for (set, map) in seen.into_iter().zip([
            &mut members,
            &mut classes,
            &mut fabric,
            &mut targets,
            &mut methods,
            &mut libraries,
        ]) {
            for name in set {
                *map.entry(name).or_insert(0usize) += 1;
            }
        }
    }
    let ranked = |m: BTreeMap<String, usize>| {
        let mut v: Vec<_> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    };
    Summary {
        mods: mods.len(),
        by_tier,
        members: ranked(members),
        classes: ranked(classes),
        fabric_api: ranked(fabric),
        mixin_targets: ranked(targets),
        injected_methods: ranked(methods),
        libraries: ranked(libraries),
    }
}

/// `reach.md`: a table per mod, then the most needed names.
pub fn markdown(mods: &[Reach], s: &Summary) -> String {
    let mut out = String::from("# What the mods' code reaches\n\n");
    out += &format!(
        "{} mods: {}.\n\n",
        s.mods,
        s.by_tier
            .iter()
            .map(|(t, n)| format!("{n} {t}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    out += "| Mod | Loader | Tier | Classes | Minecraft classes | Minecraft members | Fabric API modules | Mixins | Injectors | Nested jars |\n";
    out += "|---|---|---|---:|---:|---:|---:|---:|---:|---:|\n";
    for r in mods {
        out += &format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            r.mod_id,
            r.loader,
            r.tier(),
            r.classes,
            r.minecraft.len(),
            r.members(),
            r.fabric_api.len(),
            r.mixins.len(),
            r.mixins.iter().map(|m| m.injectors.len()).sum::<usize>(),
            r.nested.len(),
        );
    }
    let mut section = |title: &str, rows: &[(String, usize)]| {
        out += &format!("\n## {title}\n\n| Name | Mods |\n|---|---:|\n");
        for (name, n) in rows.iter().take(50) {
            out += &format!("| `{name}` | {n} |\n");
        }
    };
    section("Minecraft classes", &s.classes);
    section("Minecraft members", &s.members);
    section("Fabric API modules", &s.fabric_api);
    section("Mixin targets", &s.mixin_targets);
    section("Injected methods", &s.injected_methods);
    section("Libraries", &s.libraries);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Writes the few class-file structures the reader looks at.
    #[derive(Default)]
    struct Builder {
        pool: Vec<Vec<u8>>,
    }

    impl Builder {
        fn add(&mut self, entry: Vec<u8>) -> u16 {
            self.pool.push(entry);
            self.pool.len() as u16
        }
        fn utf8(&mut self, s: &str) -> u16 {
            let mut e = vec![1];
            e.extend((s.len() as u16).to_be_bytes());
            e.extend(s.as_bytes());
            self.add(e)
        }
        fn class(&mut self, name: &str) -> u16 {
            let n = self.utf8(name);
            self.add([&[7][..], &n.to_be_bytes()].concat())
        }
        fn method_ref(&mut self, owner: &str, name: &str, desc: &str) -> u16 {
            let o = self.class(owner);
            let (n, d) = (self.utf8(name), self.utf8(desc));
            let nat = self.add([&[12][..], &n.to_be_bytes(), &d.to_be_bytes()].concat());
            self.add([&[10][..], &o.to_be_bytes(), &nat.to_be_bytes()].concat())
        }
        /// An attribute table holding one invisible annotation.
        fn annotation(&mut self, kind: &str, elements: &[(&str, Vec<u8>)]) -> Vec<u8> {
            let attr = self.utf8("RuntimeInvisibleAnnotations");
            let kind = self.utf8(kind);
            let mut body = vec![0, 1];
            body.extend(kind.to_be_bytes());
            body.extend((elements.len() as u16).to_be_bytes());
            for (key, value) in elements {
                body.extend(self.utf8(key).to_be_bytes());
                body.extend(value);
            }
            let mut table = vec![0, 1];
            table.extend(attr.to_be_bytes());
            table.extend((body.len() as u32).to_be_bytes());
            table.extend(body);
            table
        }
        fn string_array(&mut self, tag: u8, values: &[&str]) -> Vec<u8> {
            let mut v = vec![b'['];
            v.extend((values.len() as u16).to_be_bytes());
            for s in values {
                v.push(tag);
                v.extend(self.utf8(s).to_be_bytes());
            }
            v
        }
        fn finish(self, this: u16, parent: u16, methods: Vec<Vec<u8>>, attrs: Vec<u8>) -> Vec<u8> {
            let mut out = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 65];
            out.extend((self.pool.len() as u16 + 1).to_be_bytes());
            self.pool.iter().for_each(|e| out.extend(e));
            out.extend([0, 0x21]);
            out.extend(this.to_be_bytes());
            out.extend(parent.to_be_bytes());
            out.extend([0, 0, 0, 0]);
            out.extend((methods.len() as u16).to_be_bytes());
            methods.iter().for_each(|m| out.extend(m));
            out.extend(if attrs.is_empty() { vec![0, 0] } else { attrs });
            out
        }
    }

    /// A HUD mod's entrypoint: calls into the game and Fabric API.
    fn entrypoint() -> Vec<u8> {
        let mut b = Builder::default();
        let this = b.class("dev/example/hud/Hud");
        let parent = b.class("java/lang/Object");
        b.class("net/fabricmc/api/ClientModInitializer");
        b.class("net/fabricmc/fabric/api/client/rendering/v1/HudRenderCallback");
        b.method_ref(
            "net/minecraft/client/Minecraft",
            "getInstance",
            "()Lnet/minecraft/client/Minecraft;",
        );
        b.method_ref(
            "net/minecraft/client/gui/GuiGraphics",
            "drawString",
            "(Lnet/minecraft/client/gui/Font;Ljava/lang/String;III)V",
        );
        b.method_ref(
            "com/google/gson/Gson",
            "toJson",
            "(Ljava/lang/Object;)Ljava/lang/String;",
        );
        b.finish(this, parent, vec![], vec![])
    }

    /// `@Mixin(Gui.class)` with `@Inject(method = "render", at = @At("TAIL"))`.
    fn mixin() -> Vec<u8> {
        let mut b = Builder::default();
        let this = b.class("dev/example/hud/mixin/GuiMixin");
        let parent = b.class("java/lang/Object");
        let value = b.string_array(b'c', &["Lnet/minecraft/client/gui/Gui;"]);
        let class_attrs = b.annotation(MIXIN, &[("value", value)]);
        let method_name = b.utf8("onRender");
        let desc = b.utf8("(Lnet/minecraft/client/gui/GuiGraphics;)V");
        let targets = b.string_array(b's', &["render"]);
        let at_kind = b.utf8("Lorg/spongepowered/asm/mixin/injection/At;");
        let (at_key, at_value) = (b.utf8("value"), b.utf8("TAIL"));
        let mut at = vec![b'[', 0, 1, b'@'];
        at.extend(at_kind.to_be_bytes());
        at.extend([0, 1]);
        at.extend(at_key.to_be_bytes());
        at.push(b's');
        at.extend(at_value.to_be_bytes());
        let inject = b.annotation(
            "Lorg/spongepowered/asm/mixin/injection/Inject;",
            &[("method", targets), ("at", at)],
        );
        let mut method = vec![0, 2];
        method.extend(method_name.to_be_bytes());
        method.extend(desc.to_be_bytes());
        method.extend(inject);
        b.finish(this, parent, vec![method], class_attrs)
    }

    fn jar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        for (name, data) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
        out.into_inner()
    }

    #[test]
    fn reads_what_a_fabric_mod_reaches() {
        let inner = jar(&[("dev/example/lib/Util.class", &entrypoint())]);
        let outer = jar(&[
            ("fabric.mod.json", br#"{"id": "hud"}"#),
            ("dev/example/hud/Hud.class", &entrypoint()),
            ("dev/example/hud/mixin/GuiMixin.class", &mixin()),
            ("META-INF/jars/util.jar", &inner),
        ]);
        let r = read_bytes("hud.jar", outer).unwrap();
        assert_eq!((r.mod_id.as_str(), r.loader.as_str()), ("hud", "fabric"));
        assert_eq!((r.classes, r.tier()), (2, "mixins"));
        assert_eq!(
            r.minecraft["net/minecraft/client/gui/GuiGraphics"],
            BTreeSet::from([
                "drawString:(Lnet/minecraft/client/gui/Font;Ljava/lang/String;III)V".to_owned()
            ])
        );
        // Named only in a descriptor.
        assert!(r.minecraft["net/minecraft/client/gui/Font"].is_empty());
        assert!(r.minecraft.contains_key("net/minecraft/client/gui/Gui"));
        assert_eq!(
            r.fabric_api.keys().collect::<Vec<_>>(),
            ["client/rendering/v1"]
        );
        assert_eq!(r.libraries.get("com/google"), Some(&1));
        assert_eq!(
            r.mixins,
            [Mixin {
                class: "dev/example/hud/mixin/GuiMixin".into(),
                targets: vec!["net/minecraft/client/gui/Gui".into()],
                injectors: vec![Injector {
                    kind: "Inject".into(),
                    methods: vec!["render".into()],
                    at: vec!["TAIL".into()],
                }],
            }]
        );
        assert_eq!((r.nested.len(), r.nested[0].tier()), (1, "api"));

        let s = summarize(&[r.clone(), r]);
        assert_eq!(
            s.mixin_targets,
            [("net/minecraft/client/gui/Gui".to_owned(), 2)]
        );
        assert_eq!(
            s.injected_methods,
            [("net/minecraft/client/gui/Gui.render".to_owned(), 2)]
        );
        assert!(
            s.members.contains(&(
                "net/minecraft/client/Minecraft.getInstance:()Lnet/minecraft/client/Minecraft;"
                    .to_owned(),
                2
            ))
        );
        assert!(markdown(&[], &s).contains("| `net/minecraft/client/gui/Gui` | 2 |"));
    }

    #[test]
    fn finds_classes_in_signatures() {
        let mut names = BTreeSet::new();
        descriptor_classes(
            "Ljava/util/List<Lnet/minecraft/world/item/ItemStack;>;",
            &mut names,
        );
        assert_eq!(
            names,
            BTreeSet::from([
                "java/util/List".to_owned(),
                "net/minecraft/world/item/ItemStack".to_owned()
            ])
        );
    }

    #[test]
    fn rejects_what_is_not_a_class() {
        assert!(class(b"PK\x03\x04").is_err());
        assert!(class(&[0xCA, 0xFE, 0xBA, 0xBE, 0, 0]).is_err());
    }
}
