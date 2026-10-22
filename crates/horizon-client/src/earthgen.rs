//! Earth worlds (`horizon:earth`): the real planet at one block per metre, from
//! `horizon-earth` (docs/earth.md). Until the cubic engine lands, a world shows
//! the true heights in the usual Y -64..320 window, set when the world is made
//! so that its spawn's ground stands near Y 16; far terrain shows every height.
use crate::{
    chunk::{self, Chunk, Section},
    streaming::{ChunkGenerator, LodSampler, Proto},
};
use horizon_earth::{Column, Earth, cube::Cubes, surface::Block};
use horizon_world::{nbt::Tag, registry::BlockRegistry, registry::BlockStateId};
use std::sync::Arc;

pub const GENERATOR: &str = "horizon:earth";
/// Blocks around the spawn whose data is fetched before the world opens.
const NEAR: i32 = 512;
/// Far terrain's reach, fetched while the player plays.
const FAR: i32 = 32_768;
/// Where a new world's spawn ground stands.
const SPAWN_Y: i32 = 16;

/// Where an Earth world is: its home and the Earth Y of world Y 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub latitude: f64,
    pub longitude: f64,
    /// A multiple of 16, so that cubes are sections.
    pub shift: i32,
}
impl Place {
    /// Earth Platform's default home, El Capitan Meadow in Yosemite Valley.
    pub const YOSEMITE: (f64, f64) = (37.724, -119.6365);
    /// The overworld's place, if its generator is an Earth one.
    pub fn from_settings(settings: &Tag) -> Option<Self> {
        let generator = settings
            .get("dimensions")?
            .get("minecraft:overworld")?
            .get("generator")?;
        if crate::save::text(generator.get("type"))? != GENERATOR {
            return None;
        }
        let s = generator.get("settings")?;
        let double = |key: &str| match s.get(key)? {
            Tag::Double(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        };
        Some(Self {
            latitude: double("latitude")?,
            longitude: double("longitude")?,
            shift: crate::save::number(s.get("shift"))? as i32,
        })
    }
    /// The generator compound `world_gen_settings` keeps.
    pub fn generator(&self) -> Tag {
        Tag::compound([
            ("type", Tag::String(GENERATOR.into())),
            (
                "settings",
                Tag::compound([
                    ("latitude", Tag::Double(self.latitude.to_bits())),
                    ("longitude", Tag::Double(self.longitude.to_bits())),
                    ("shift", Tag::Int(self.shift)),
                ]),
            ),
        ])
    }
    /// A new world at a latitude and longitude: fetches what its spawn needs
    /// and sets the window so that the spawn's ground is near Y 16.
    pub fn settle(latitude: f64, longitude: f64) -> Self {
        let earth = earth(latitude, longitude);
        let (hx, hz) = earth.home();
        earth.prepare_near(hx, hz, NEAR);
        let (_, y, _) = earth.spawn();
        Self {
            latitude,
            longitude,
            shift: (y - SPAWN_Y).div_euclid(16) * 16,
        }
    }
}

/// Earth Platform's cache folder in the game folder, shared by every Earth world.
fn earth(latitude: f64, longitude: f64) -> Earth {
    let mut settings = horizon_earth::Settings::at(latitude, longitude);
    settings.cache = Some(std::path::PathBuf::from("earthplatform/cache"));
    Earth::new(settings)
}

pub struct EarthTerrain {
    cubes: Cubes,
    /// Block states by `Block::index`.
    states: [u32; Block::ALL.len()],
    bedrock: u32,
    /// Biome table indices by name.
    biome: Box<dyn Fn(&str) -> u16 + Send + Sync>,
    /// The Earth block of world X and Z 0, a cube corner.
    origin: (i32, i32),
    shift: i32,
}
impl EarthTerrain {
    pub fn new(
        place: Place,
        registry: &BlockRegistry,
        biomes: Arc<crate::biome::Biomes>,
    ) -> crate::Result<Arc<Self>> {
        let id = |name: &str| -> crate::Result<u32> {
            Ok(registry
                .default_state(name)
                .ok_or_else(|| format!("missing {name}"))?
                .0)
        };
        let mut states = [0; Block::ALL.len()];
        for b in Block::ALL {
            states[b.index()] = id(b.name())?;
        }
        let earth = Arc::new(earth(place.latitude, place.longitude));
        let (hx, hz) = earth.home();
        let origin = (hx & !15, hz & !15);
        earth.prepare_near(hx, hz, NEAR);
        // The far terrain's data follows while the player plays.
        let far = earth.clone();
        std::thread::Builder::new()
            .name("Earth data".into())
            .spawn(move || {
                far.prepare(hx, hz, NEAR, FAR);
            })?;
        Ok(Arc::new(Self {
            cubes: Cubes::new(earth),
            states,
            bedrock: id("minecraft:bedrock")?,
            biome: Box::new(move |name| biomes.id(name)),
            origin,
            shift: place.shift,
        }))
    }
    /// The world spawn's chunk and height (ChunkGenerator.getOrigin and getSpawnHeight).
    pub fn spawn(&self) -> ((i32, i32), i32) {
        let (x, y, z) = self.cubes.earth().spawn();
        (
            ((x - self.origin.0) >> 4, (z - self.origin.1) >> 4),
            y - self.shift,
        )
    }
    fn surface(&self, c: &Column) -> crate::lod::Surface {
        let water = if c.frozen { Block::Ice } else { Block::Water };
        crate::lod::Surface {
            height: c.ground - self.shift + 1,
            state: BlockStateId(self.states[c.surface.top.index()]),
            water: c
                .water
                .map(|w| (w - self.shift + 1, BlockStateId(self.states[water.index()]))),
            biome: (self.biome)(c.surface.biome),
        }
    }
    pub fn generate(&self, cx: i32, cz: i32) -> Chunk {
        let mut chunk = Chunk::empty(cx, cz);
        let (ex, ez) = ((self.origin.0 >> 4) + cx, (self.origin.1 >> 4) + cz);
        let columns = self.cubes.columns(ex, ez);
        // One biome per 4 × 4 cell, from the column at its centre, at every height.
        let mut cells = [0u32; 64];
        for (i, cell) in cells.iter_mut().enumerate() {
            let (x, z) = ((i & 3) * 4 + 2, (i >> 2 & 3) * 4 + 2);
            *cell = u32::from((self.biome)(columns[z * 16 + x].surface.biome));
        }
        let biomes =
            horizon_core::paletted::PalettedContainer::from_values(chunk::BIOME_STRATEGY, &cells);
        let mut values = [0u32; 4096];
        for (s, section) in chunk.sections.iter_mut().enumerate() {
            let cy = (chunk::MIN_Y + self.shift) / 16 + s as i32;
            let blocks = match self.cubes.cube(ex, cy, ez) {
                Some(cube) => {
                    for (v, &b) in values.iter_mut().zip(cube.iter()) {
                        *v = self.states[b as usize];
                    }
                    if s == 0 {
                        values[..256].fill(self.bedrock);
                    }
                    Section::from_blocks(&values)
                }
                None if s == 0 => {
                    values.fill(0);
                    values[..256].fill(self.bedrock);
                    Section::from_blocks(&values)
                }
                None => horizon_core::paletted::PalettedContainer::new(chunk::block_strategy(), 0),
            };
            *section = Arc::new(Section {
                blocks,
                biomes: biomes.clone(),
                ..Section::default()
            });
        }
        chunk
    }
}

impl ChunkGenerator for EarthTerrain {
    type Worker = ();
    fn worker(&self) {}
    fn lod_sampler(self: Arc<Self>) -> Box<dyn LodSampler> {
        Box::new(Sampler(self))
    }
    fn lod_surface(&self, _: &mut (), x: i32, z: i32) -> Option<crate::lod::Surface> {
        let earth = self.cubes.earth();
        Some(self.surface(&earth.region(x + self.origin.0, z + self.origin.1, 1, 1)[0]))
    }
    fn proto(&self, _: &mut (), x: i32, z: i32) -> Proto {
        Box::new(self.generate(x, z))
    }
    fn finish(&self, proto: Proto) -> Chunk {
        let mut chunk = *proto.downcast::<Chunk>().expect("Earth proto");
        chunk.fresh = true;
        chunk
    }
}

/// Far terrain a grid at a time: one `Earth::region` per grid, not one per column.
struct Sampler(Arc<EarthTerrain>);
impl LodSampler for Sampler {
    fn fork(&self) -> Option<Box<dyn LodSampler>> {
        Some(Box::new(Self(self.0.clone())))
    }
    fn sample(&mut self, x: i32, z: i32) -> Option<crate::lod::Surface> {
        self.0.lod_surface(&mut (), x, z)
    }
    fn grid(&mut self, x: i32, z: i32, stride: i32, side: usize) -> Vec<crate::lod::Column> {
        let t = &self.0;
        // Each region column reads the data at its voxel's centre.
        let half = stride / 2;
        t.cubes
            .earth()
            .region(x + t.origin.0 - half, z + t.origin.1 - half, side, stride)
            .iter()
            .map(|c| crate::lod::Column::from_surface(Some(t.surface(c))))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An Earth with no data (no network, no cache): open sea everywhere.
    fn terrain(shift: i32) -> EarthTerrain {
        let mut settings = horizon_earth::Settings::at(Place::YOSEMITE.0, Place::YOSEMITE.1);
        settings.allow_network = false;
        let earth = Arc::new(Earth::new(settings));
        let (hx, hz) = earth.home();
        let mut states = [0; Block::ALL.len()];
        for (i, s) in states.iter_mut().enumerate() {
            *s = i as u32 + 10;
        }
        states[Block::Air.index()] = 0;
        EarthTerrain {
            cubes: Cubes::new(earth),
            states,
            bedrock: 99,
            biome: Box::new(|_| 3),
            origin: (hx & !15, hz & !15),
            shift,
        }
    }

    #[test]
    fn chunks_agree_with_far_terrain_and_stand_on_bedrock() {
        for shift in [-64, 0, 1024] {
            let t = terrain(shift);
            for (cx, cz) in [(0, 0), (-3, 2)] {
                let chunk = t.generate(cx, cz);
                for (x, z) in [(0, 0), (7, 9), (15, 15)] {
                    assert_eq!(chunk.get(x, chunk::MIN_Y, z), 99);
                    let s = t.lod_surface(&mut (), cx * 16 + x, cz * 16 + z).unwrap();
                    let top = chunk::MIN_Y + chunk::HEIGHT - 1;
                    if (chunk::MIN_Y + 1..=top).contains(&(s.height - 1)) {
                        assert_eq!(
                            chunk.get(x, s.height - 1, z),
                            s.state.0,
                            "{shift} {cx} {cz}"
                        );
                    }
                    if let Some((w, state)) = s.water
                        && (chunk::MIN_Y + 1..top).contains(&(w - 1))
                    {
                        assert_eq!(chunk.get(x, w - 1, z), state.0);
                        assert_eq!(chunk.get(x, w, z), 0);
                    }
                }
            }
        }
    }

    #[test]
    fn far_grids_sample_each_voxel_at_its_corner() {
        let mut sampler = Sampler(Arc::new(terrain(0)));
        let grid = sampler.grid(-40, 24, 8, 4);
        for (i, column) in grid.iter().enumerate() {
            let (x, z) = (-40 + (i % 4) as i32 * 8, 24 + (i / 4) as i32 * 8);
            let single = sampler.sample(x, z).unwrap();
            let surface = column.surface.unwrap();
            assert_eq!(
                (surface.height, surface.water),
                (single.height, single.water)
            );
        }
    }
}
