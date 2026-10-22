//! Cubes: the Earth world in 16-block cubes keyed by their real coordinates, as Earth Platform's
//! cubic engine stores it, from Y -16384 to 16383. A cube's blocks come from its column's ground,
//! water and surface, worked out once per 16 × 16 column and shared by every cube above and
//! below it.
use crate::{Column, Earth, fetch::Flight, surface::Block};
use std::sync::Arc;

pub const MIN_Y: i32 = -16384;
pub const HEIGHT: i32 = 32768;
/// Blocks of a cube, x fastest, then z, then y, as indices into [`Block::ALL`].
pub type Cube = [u8; 4096];

/// Cubes over an [`Earth`], with each 16 × 16 column's data kept while its cubes are made.
pub struct Cubes {
    earth: Arc<Earth>,
    columns: Flight<(i32, i32), Vec<Column>>,
}
impl Cubes {
    pub fn new(earth: Arc<Earth>) -> Self {
        Self {
            earth,
            columns: Flight::new(4096),
        }
    }
    pub fn earth(&self) -> &Earth {
        &self.earth
    }
    pub fn columns(&self, cx: i32, cz: i32) -> Arc<Vec<Column>> {
        self.columns
            .get(&(cx, cz), || Ok(self.earth.region(cx * 16, cz * 16, 16, 1)))
            .expect("Earth regions don't fail")
    }
    /// The cube at cube coordinates (`cx`, `cy`, `cz`); None when it is all air.
    pub fn cube(&self, cx: i32, cy: i32, cz: i32) -> Option<Box<Cube>> {
        let columns = self.columns(cx, cz);
        let (y0, y1) = (cy * 16, cy * 16 + 15);
        if columns
            .iter()
            .all(|c| c.ground.max(c.water.unwrap_or(i32::MIN)) < y0)
        {
            return None;
        }
        let mut out: Box<Cube> = Box::new([Block::Air as u8; 4096]);
        let stone = Block::Stone.index() as u8;
        // Cubes buried under every column are solid rock.
        if columns.iter().all(|c| c.ground - 4 > y1) {
            out.fill(stone);
            return Some(out);
        }
        let ids = |b: Block| b.index() as u8;
        for (i, c) in columns.iter().enumerate() {
            let (x, z) = (i % 16, i / 16);
            let (top, under) = (ids(c.surface.top), ids(c.surface.under));
            let water = if c.frozen {
                ids(Block::Ice)
            } else {
                ids(Block::Water)
            };
            for y in y0..=y1 {
                let block = if y > c.ground {
                    match c.water {
                        Some(w) if y < w => ids(Block::Water),
                        Some(w) if y == w => water,
                        _ => continue,
                    }
                } else if y == c.ground {
                    top
                } else if y > c.ground - 4 {
                    under
                } else {
                    stone
                };
                out[((y - y0) as usize) << 8 | z << 4 | x] = block;
            }
        }
        Some(out)
    }
}
