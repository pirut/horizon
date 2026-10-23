//! Earth worlds (`horizon:earth`): the real planet at one block per metre, from
//! `horizon-earth` (docs/earth.md), at its true heights. The level's 384 blocks
//! are a window of 16-block cubes around the player that moves when they climb
//! or fall far enough (`chunk::min_y`), as Earth Platform keeps a band of cubes
//! loaded around its players; far terrain shows every height.
use crate::{
    chunk::{self, Chunk, Section},
    streaming::{ChunkGenerator, LodSampler, Proto},
};
use horizon_core::paletted::PalettedContainer;
use horizon_earth::{Column, Earth, cube, cube::Cubes, surface::Block};
use horizon_world::{nbt::Tag, registry::BlockRegistry, registry::BlockStateId};
use std::sync::{Arc, Mutex};

pub const GENERATOR: &str = "horizon:earth";
/// Blocks around the spawn whose data is fetched before the world opens.
const NEAR: i32 = 512;
/// Far terrain's reach, fetched while the player plays.
const FAR: i32 = 32_768;
/// How close to the window's top or bottom the player comes before it moves:
/// it then centres on them, 192 blocks either way.
pub const WINDOW_MARGIN: i32 = 64;

/// Where an Earth world is: its home, and the Earth Y of world Y 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub latitude: f64,
    pub longitude: f64,
    /// A multiple of 16, so that cubes are sections: 0 for the true heights.
    /// Worlds made before the window moved kept their spawn near Y 16.
    pub shift: i32,
}
impl Place {
    /// Earth Platform's default home, El Capitan Meadow in Yosemite Valley.
    pub const YOSEMITE: (f64, f64) = (37.724, -119.6365);
    /// A new world at a latitude and longitude, at the true heights.
    pub fn at(latitude: f64, longitude: f64) -> Self {
        Self {
            latitude,
            longitude,
            shift: 0,
        }
    }
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
            shift: crate::save::number(s.get("shift")).unwrap_or(0) as i32,
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
}

/// Whether a biome of this base temperature snows (Biome.coldEnoughToSnow
/// without vanilla's cooling above Y 80: an Earth biome is the real climate at
/// its height).
pub fn snows(temperature: f32) -> bool {
    temperature < 0.15
}

/// Earth Platform's cache folder in the game folder, shared by every Earth world.
fn earth(latitude: f64, longitude: f64) -> Earth {
    let mut settings = horizon_earth::Settings::at(latitude, longitude);
    settings.cache = Some(std::path::PathBuf::from("earthplatform/cache"));
    Earth::new(settings)
}

/// The open world's generator, kept while its window moves.
static OPEN: Mutex<Option<(Place, Arc<EarthTerrain>)>> = Mutex::new(None);

pub struct EarthTerrain {
    cubes: Cubes,
    /// Block states by `Block::index`.
    states: [u32; Block::ALL.len()],
    /// Biome table indices by name.
    biome: Box<dyn Fn(&str) -> u16 + Send + Sync>,
    /// The Earth block of world X and Z 0, a cube corner.
    origin: (i32, i32),
    shift: i32,
}
impl EarthTerrain {
    /// The generator of `place`: the one already open there (a window move
    /// reopens the level), else a new one, with its spawn's data fetched.
    pub fn open(
        place: Place,
        registry: &BlockRegistry,
        biomes: Arc<crate::biome::Biomes>,
    ) -> crate::Result<Arc<Self>> {
        let mut open = OPEN.lock().map_err(|_| "Earth generator poisoned")?;
        if let Some((at, terrain)) = &*open
            && *at == place
        {
            return Ok(terrain.clone());
        }
        let terrain = Self::new(place, registry, biomes)?;
        *open = Some((place, terrain.clone()));
        Ok(terrain)
    }
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
    /// The window's lowest block for a player at `y`: centred on them, inside
    /// the Earth's cubes.
    pub fn window(&self, y: i32) -> i32 {
        let lowest = cube::MIN_Y - self.shift;
        let highest = cube::MIN_Y + cube::HEIGHT - chunk::HEIGHT - self.shift;
        ((y - chunk::HEIGHT / 2).div_euclid(16) * 16).clamp(lowest, highest)
    }
    /// The window a player at `y` needs, if the one at `min_y` no longer holds them.
    pub fn moved_window(&self, min_y: i32, y: i32) -> Option<i32> {
        let inside = min_y + WINDOW_MARGIN..min_y + chunk::HEIGHT - WINDOW_MARGIN;
        let window = self.window(y);
        (!inside.contains(&y) && window != min_y).then_some(window)
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
    /// The Earth cube column of chunk (`cx`, `cz`).
    fn cube_column(&self, cx: i32, cz: i32) -> (i32, i32) {
        ((self.origin.0 >> 4) + cx, (self.origin.1 >> 4) + cz)
    }
    /// One biome per 4 × 4 cell, from the column at its centre, at every height.
    fn biomes(&self, ex: i32, ez: i32) -> PalettedContainer {
        let columns = self.cubes.columns(ex, ez);
        let mut cells = [0u32; 64];
        for (i, cell) in cells.iter_mut().enumerate() {
            let (x, z) = ((i & 3) * 4 + 2, (i >> 2 & 3) * 4 + 2);
            *cell = u32::from((self.biome)(columns[z * 16 + x].surface.biome));
        }
        PalettedContainer::from_values(chunk::BIOME_STRATEGY, &cells)
    }
    /// Section `s` of the window: the cube at its height.
    fn section(&self, (ex, ez): (i32, i32), s: usize, biomes: &PalettedContainer) -> Section {
        let cy = (chunk::min_y() + self.shift).div_euclid(16) + s as i32;
        let blocks = match self.cubes.cube(ex, cy, ez) {
            Some(cube) => {
                let mut values = [0u32; 4096];
                for (v, &b) in values.iter_mut().zip(cube.iter()) {
                    *v = self.states[b as usize];
                }
                Section::from_blocks(&values)
            }
            None => PalettedContainer::new(chunk::block_strategy(), 0),
        };
        Section {
            blocks,
            biomes: biomes.clone(),
            ..Section::default()
        }
    }
    pub fn generate(&self, cx: i32, cz: i32) -> Chunk {
        let mut chunk = Chunk::empty(cx, cz);
        let column = self.cube_column(cx, cz);
        let biomes = self.biomes(column.0, column.1);
        for (s, section) in chunk.sections.iter_mut().enumerate() {
            *section = Arc::new(self.section(column, s, &biomes));
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
    fn fill_unsaved(&self, chunk: &mut Chunk, saved: u32) {
        let column = self.cube_column(chunk.x, chunk.z);
        let biomes = self.biomes(column.0, column.1);
        for (s, section) in chunk.sections.iter_mut().enumerate() {
            if saved & 1 << s == 0 {
                *section = Arc::new(self.section(column, s, &biomes));
            }
        }
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
            biome: Box::new(|_| 3),
            origin: (hx & !15, hz & !15),
            shift,
        }
    }

    #[test]
    fn chunks_agree_with_far_terrain() {
        for shift in [-64, 0, 1024] {
            let t = terrain(shift);
            for (cx, cz) in [(0, 0), (-3, 2)] {
                let chunk = t.generate(cx, cz);
                for (x, z) in [(0, 0), (7, 9), (15, 15)] {
                    let s = t.lod_surface(&mut (), cx * 16 + x, cz * 16 + z).unwrap();
                    let top = chunk::min_y() + chunk::HEIGHT - 1;
                    if (chunk::min_y()..=top).contains(&(s.height - 1)) {
                        assert_eq!(
                            chunk.get(x, s.height - 1, z),
                            s.state.0,
                            "{shift} {cx} {cz}"
                        );
                    }
                    if let Some((w, state)) = s.water
                        && (chunk::min_y()..top).contains(&(w - 1))
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

    #[test]
    fn the_window_centres_on_the_player_and_moves_near_its_edges() {
        let t = terrain(0);
        assert_eq!(t.window(1269), 1072);
        assert_eq!(t.window(-10), -208);
        // Inside the Earth's cubes, Y -16384 to 16383.
        assert_eq!(t.window(-16380), -16384);
        assert_eq!(t.window(16383), 16384 - 384);
        let lo = t.window(1269);
        assert_eq!(t.moved_window(lo, 1269), None);
        assert_eq!(t.moved_window(lo, lo + WINDOW_MARGIN), None);
        for y in [
            lo + WINDOW_MARGIN - 1,
            lo + 384 - WINDOW_MARGIN,
            lo - 500,
            lo + 900,
        ] {
            let moved = t.moved_window(lo, y).unwrap();
            assert_eq!(moved, t.window(y));
            assert_eq!(
                t.moved_window(moved, y),
                None,
                "{y} stays in its new window"
            );
        }
        // At the Earth's floor the window stays put.
        let floor = t.window(-16384);
        assert_eq!(t.moved_window(floor, -16384), None);
        // A world made before the window moved keeps its shift.
        assert_eq!(terrain(1248).window(16), -176);
    }

    #[test]
    fn loaded_chunks_get_the_sections_their_record_lacks() {
        let t = terrain(0);
        let generated = t.generate(2, -1);
        let mut loaded = Chunk::empty(2, -1);
        let marker = PalettedContainer::new(chunk::block_strategy(), 7);
        for s in [3, 9] {
            Arc::make_mut(&mut loaded.sections[s]).blocks = marker.clone();
        }
        t.fill_unsaved(&mut loaded, 1 << 3 | 1 << 9);
        for s in 0..chunk::SECTIONS {
            let want = if s == 3 || s == 9 {
                7
            } else {
                generated.sections[s].get(0)
            };
            assert_eq!(loaded.sections[s].get(0), want, "section {s}");
        }
    }
}
