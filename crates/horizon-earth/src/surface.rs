//! What covers the ground: a column's biome and top blocks from its land cover class, climate
//! and slope. A compact port of Earth Platform's `EarthGenerator.surface` and `covered`; the
//! regional special cases (volcanic ground, the outback's red sand, ecoregions, salt pans, rock
//! strata) are not carried over yet.
use crate::{climate::Zone, worldcover as wc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Block {
    Air,
    Stone,
    Deepslate,
    Bedrock,
    Water,
    Grass,
    Dirt,
    CoarseDirt,
    Podzol,
    Sand,
    RedSand,
    Sandstone,
    RedSandstone,
    Gravel,
    SnowBlock,
    PowderSnow,
    PackedIce,
    BlueIce,
    Ice,
    Mud,
    Moss,
    Terracotta,
    OrangeTerracotta,
    Calcite,
    Andesite,
    Diorite,
}
impl Block {
    pub const ALL: [Block; 26] = [
        Block::Air,
        Block::Stone,
        Block::Deepslate,
        Block::Bedrock,
        Block::Water,
        Block::Grass,
        Block::Dirt,
        Block::CoarseDirt,
        Block::Podzol,
        Block::Sand,
        Block::RedSand,
        Block::Sandstone,
        Block::RedSandstone,
        Block::Gravel,
        Block::SnowBlock,
        Block::PowderSnow,
        Block::PackedIce,
        Block::BlueIce,
        Block::Ice,
        Block::Mud,
        Block::Moss,
        Block::Terracotta,
        Block::OrangeTerracotta,
        Block::Calcite,
        Block::Andesite,
        Block::Diorite,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Air => "minecraft:air",
            Self::Stone => "minecraft:stone",
            Self::Deepslate => "minecraft:deepslate",
            Self::Bedrock => "minecraft:bedrock",
            Self::Water => "minecraft:water",
            Self::Grass => "minecraft:grass_block",
            Self::Dirt => "minecraft:dirt",
            Self::CoarseDirt => "minecraft:coarse_dirt",
            Self::Podzol => "minecraft:podzol",
            Self::Sand => "minecraft:sand",
            Self::RedSand => "minecraft:red_sand",
            Self::Sandstone => "minecraft:sandstone",
            Self::RedSandstone => "minecraft:red_sandstone",
            Self::Gravel => "minecraft:gravel",
            Self::SnowBlock => "minecraft:snow_block",
            Self::PowderSnow => "minecraft:powder_snow",
            Self::PackedIce => "minecraft:packed_ice",
            Self::BlueIce => "minecraft:blue_ice",
            Self::Ice => "minecraft:ice",
            Self::Mud => "minecraft:mud",
            Self::Moss => "minecraft:moss_block",
            Self::Terracotta => "minecraft:terracotta",
            Self::OrangeTerracotta => "minecraft:orange_terracotta",
            Self::Calcite => "minecraft:calcite",
            Self::Andesite => "minecraft:andesite",
            Self::Diorite => "minecraft:diorite",
        }
    }
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&b| b == self).unwrap()
    }
}

/// A column's ground: its biome and the blocks on top and under them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Surface {
    pub biome: &'static str,
    pub top: Block,
    pub under: Block,
}
const fn s(biome: &'static str, top: Block, under: Block) -> Surface {
    Surface { biome, top, under }
}

/// The climate where a column stands, at its own height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Local {
    pub zone: Zone,
    pub temperature: f64,
    pub warmest: f64,
    pub rain: f64,
    pub latitude: f64,
}

/// What lies on the ground of a dry column.
pub struct Ground {
    pub code: u8,
    pub local: Local,
    /// Rise per run over a few blocks.
    pub steep: f64,
    /// Blocks above the sea's top water block.
    pub above_sea: i32,
    /// Right by the coast, low enough for a beach.
    pub shore: bool,
    /// A per-block roll in 0..100 and a patch noise in about -1..1 tens of metres across.
    pub roll: u32,
    pub patch: f64,
}

/// The class most land in a climate has, where the map has no data.
fn typical(zone: Zone) -> u8 {
    match zone {
        Zone::HotDesert | Zone::ColdDesert => wc::BARE,
        Zone::TropicalDry | Zone::HotSteppe | Zone::Mediterranean => wc::SHRUBLAND,
        Zone::ColdSteppe | Zone::Tundra => wc::GRASSLAND,
        Zone::Ice => wc::SNOW_AND_ICE,
        _ => wc::TREE_COVER,
    }
}

pub fn ground(g: &Ground) -> Surface {
    use Block::*;
    let t = g.local.temperature;
    let steep = g.steep;
    if g.shore && g.above_sea <= 2 && !matches!(g.code, wc::WETLAND | wc::MANGROVES) {
        if steep > 1.0 {
            return s("minecraft:stony_shore", Stone, Stone);
        }
        return s(
            if t < 0.0 {
                "minecraft:snowy_beach"
            } else {
                "minecraft:beach"
            },
            Sand,
            Sand,
        );
    }
    let code = match g.code {
        wc::NO_DATA | wc::WATER => typical(g.local.zone),
        // The world is wild land: towns take the cover their climate gives.
        wc::BUILT_UP => match typical(g.local.zone) {
            wc::TREE_COVER => wc::GRASSLAND,
            other => other,
        },
        other => other,
    };
    covered(code, g)
}

fn rock() -> Surface {
    s("minecraft:stony_peaks", Block::Stone, Block::Stone)
}

fn covered(code: u8, g: &Ground) -> Surface {
    use Block::*;
    let (t, zone, steep, patch, roll) =
        (g.local.temperature, g.local.zone, g.steep, g.patch, g.roll);
    let cliff = steep > 1.25;
    let rocky = steep > 0.8 && patch < (steep - 0.8) * 1.6 - 0.2;
    let dry = matches!(zone, Zone::ColdDesert | Zone::ColdSteppe);
    let moor = matches!(zone, Zone::Oceanic | Zone::SubpolarOceanic)
        && g.local.warmest < 14.0
        && g.local.rain > 800.0;
    match code {
        wc::TREE_COVER => {
            if steep > 1.7 {
                return rock();
            }
            let forest = forest(&g.local, patch);
            if steep > 1.3 && patch < (steep - 1.3) * 2.0 - 0.3 {
                return s(forest.biome, Stone, Stone);
            }
            if steep > 0.8 {
                return s(
                    forest.biome,
                    if patch > 0.35 { Podzol } else { Grass },
                    Dirt,
                );
            }
            forest
        }
        wc::SHRUBLAND => {
            if cliff || rocky && t < -3.0 {
                return rock();
            }
            let top = if rocky {
                Stone
            } else if patch > 0.4 || roll < 20 {
                CoarseDirt
            } else {
                Grass
            };
            if moor {
                return s(
                    "minecraft:windswept_hills",
                    if patch > 0.2 { Moss } else { top },
                    Dirt,
                );
            }
            match zone {
                Zone::Rainforest => s("minecraft:sparse_jungle", top, Dirt),
                Zone::TropicalDry | Zone::HotSteppe => s(
                    if patch > 0.5 {
                        "minecraft:windswept_savanna"
                    } else {
                        "minecraft:savanna"
                    },
                    top,
                    Dirt,
                ),
                Zone::HotDesert => s(
                    "minecraft:desert",
                    if rocky {
                        Stone
                    } else if patch > 0.2 {
                        CoarseDirt
                    } else {
                        Sand
                    },
                    Sandstone,
                ),
                Zone::ColdDesert | Zone::ColdSteppe => {
                    if t < -3.0 {
                        s("minecraft:snowy_plains", top, Dirt)
                    } else {
                        s(
                            "minecraft:savanna",
                            if patch > 0.3 || roll < 55 {
                                CoarseDirt
                            } else {
                                Grass
                            },
                            Dirt,
                        )
                    }
                }
                Zone::Mediterranean => s("minecraft:plains", top, Dirt),
                Zone::Subarctic => {
                    if t < -3.0 {
                        s("minecraft:snowy_plains", top, Dirt)
                    } else {
                        s(
                            "minecraft:taiga",
                            if patch > 0.3 { Podzol } else { top },
                            Dirt,
                        )
                    }
                }
                Zone::Tundra | Zone::Ice => s(
                    if t < -3.0 {
                        "minecraft:snowy_plains"
                    } else {
                        "minecraft:meadow"
                    },
                    top,
                    Dirt,
                ),
                _ => s("minecraft:plains", top, Dirt),
            }
        }
        wc::CROPLAND | wc::GRASSLAND => {
            if cliff || rocky && t < -3.0 {
                return rock();
            }
            let top = if rocky { Stone } else { Grass };
            if t < -3.0 {
                return s("minecraft:snowy_plains", top, Dirt);
            }
            if dry && top == Grass && (patch > 0.35 || roll < 60) {
                return s("minecraft:savanna", CoarseDirt, Dirt);
            }
            grassland(top, &g.local, patch, moor)
        }
        wc::BARE => {
            let hot = matches!(zone, Zone::HotDesert | Zone::HotSteppe | Zone::TropicalDry);
            let arid = hot || dry;
            // Bare ground in wet lowlands is cleared land: it goes back to grass.
            if !arid
                && !cliff
                && steep < 0.7
                && g.local.rain > 700.0
                && g.above_sea < 300
                && t > 0.0
            {
                return covered(wc::GRASSLAND, g);
            }
            if arid && (cliff || steep > 0.9) {
                let band = [Terracotta, OrangeTerracotta, Terracotta, RedSandstone]
                    [(g.above_sea.rem_euclid(11) / 3) as usize];
                return s("minecraft:badlands", band, Terracotta);
            }
            if hot {
                let red = roll < 25;
                return s(
                    "minecraft:desert",
                    if red { RedSand } else { Sand },
                    if red { RedSandstone } else { Sandstone },
                );
            }
            if dry && !cliff && steep <= 0.9 {
                let top = if patch > 0.2 {
                    Sand
                } else if patch > -0.3 {
                    Gravel
                } else {
                    CoarseDirt
                };
                return s(
                    if t < -3.0 && g.local.warmest < 5.0 {
                        "minecraft:snowy_plains"
                    } else {
                        "minecraft:desert"
                    },
                    top,
                    if top == Sand { Sandstone } else { Stone },
                );
            }
            if g.local.warmest < -6.0 && steep < 0.5 && patch > 0.1 {
                return s("minecraft:snowy_slopes", SnowBlock, Stone);
            }
            let top = if cliff || steep > 0.7 {
                Stone
            } else if patch > 0.3 {
                Gravel
            } else if patch < -0.3 {
                Andesite
            } else if roll < 50 {
                Stone
            } else if roll < 80 {
                Gravel
            } else {
                CoarseDirt
            };
            s("minecraft:stony_peaks", top, Stone)
        }
        wc::SNOW_AND_ICE => {
            if steep > 1.6 || steep > 1.3 && patch > 0.3 {
                if g.above_sea < 600 {
                    return s("minecraft:frozen_peaks", BlueIce, PackedIce);
                }
                return rock();
            }
            let top = if steep < 0.5 && patch > 0.5 {
                PowderSnow
            } else {
                SnowBlock
            };
            s(
                if steep > 0.75 {
                    "minecraft:frozen_peaks"
                } else {
                    "minecraft:snowy_slopes"
                },
                top,
                PackedIce,
            )
        }
        wc::WETLAND => {
            if t < 0.0 {
                return s(
                    "minecraft:snowy_plains",
                    if patch > 0.25 { Moss } else { Grass },
                    Dirt,
                );
            }
            s(
                "minecraft:swamp",
                if patch > 0.4 { Mud } else { Grass },
                Dirt,
            )
        }
        wc::MANGROVES => {
            if g.above_sea > 12 || t < 18.0 {
                return covered(wc::WETLAND, g);
            }
            s("minecraft:mangrove_swamp", Mud, Mud)
        }
        wc::MOSS_AND_LICHEN => {
            if cliff || rocky && t < -2.0 {
                return rock();
            }
            let top = if rocky {
                Stone
            } else if patch > 0.2 {
                Moss
            } else if roll < 30 {
                Gravel
            } else {
                Grass
            };
            let snowy = t < -2.0 && (g.local.warmest < 2.0 || g.local.latitude.abs() > 55.0);
            s(
                if snowy {
                    "minecraft:snowy_plains"
                } else {
                    "minecraft:meadow"
                },
                top,
                Dirt,
            )
        }
        _ => covered(typical(zone), g),
    }
}

fn grassland(top: Block, local: &Local, patch: f64, moor: bool) -> Surface {
    use Block::*;
    if moor {
        return s(
            "minecraft:windswept_hills",
            if patch > 0.3 { Moss } else { top },
            Dirt,
        );
    }
    let biome = match local.zone {
        Zone::TropicalDry | Zone::HotSteppe | Zone::HotDesert => "minecraft:savanna",
        Zone::Rainforest => "minecraft:sparse_jungle",
        Zone::Subarctic | Zone::Tundra => "minecraft:meadow",
        Zone::Ice => "minecraft:snowy_plains",
        _ if patch > 0.55 => "minecraft:sunflower_plains",
        _ => "minecraft:plains",
    };
    s(biome, top, Dirt)
}

/// Forest of the climate: rainforest, oak, birch, pine or taiga where it is on Earth.
fn forest(local: &Local, patch: f64) -> Surface {
    use Block::*;
    let t = local.temperature;
    let biome = match local.zone {
        Zone::Rainforest => {
            if patch > 0.6 {
                "minecraft:bamboo_jungle"
            } else {
                "minecraft:jungle"
            }
        }
        Zone::TropicalDry | Zone::HotSteppe | Zone::HotDesert => "minecraft:savanna",
        Zone::Subarctic | Zone::Tundra | Zone::Ice => {
            if t < -3.0 {
                "minecraft:snowy_taiga"
            } else if local.rain > 700.0 && patch > 0.2 {
                "minecraft:old_growth_spruce_taiga"
            } else {
                "minecraft:taiga"
            }
        }
        Zone::Continental => {
            if t < 4.0 {
                "minecraft:taiga"
            } else if patch > 0.45 {
                "minecraft:birch_forest"
            } else {
                "minecraft:forest"
            }
        }
        Zone::Mediterranean | Zone::ColdSteppe | Zone::ColdDesert => {
            if t < 9.0 {
                "minecraft:taiga"
            } else {
                "minecraft:forest"
            }
        }
        Zone::Subtropical => {
            if patch > 0.5 {
                "minecraft:dark_forest"
            } else if patch < -0.6 {
                "minecraft:flower_forest"
            } else {
                "minecraft:forest"
            }
        }
        Zone::Oceanic | Zone::SubpolarOceanic => {
            if t < 5.0 {
                "minecraft:taiga"
            } else if patch > 0.5 {
                "minecraft:dark_forest"
            } else {
                "minecraft:forest"
            }
        }
    };
    let top = if biome == "minecraft:old_growth_spruce_taiga" && patch > 0.5 {
        Podzol
    } else {
        Grass
    };
    s(biome, top, Dirt)
}

/// The sea's biome by temperature and depth (blocks of water over the bed).
pub fn sea(temperature: f64, depth: i32, latitude: f64, wobble: f64) -> Surface {
    use Block::*;
    let deep = depth as f64 + wobble;
    let biome = if temperature < -2.0 && latitude > -50.0 {
        "minecraft:frozen_ocean"
    } else if temperature < 6.0 {
        if deep > 40.0 {
            "minecraft:deep_cold_ocean"
        } else {
            "minecraft:cold_ocean"
        }
    } else if temperature > 15.0 {
        if deep > if temperature > 21.0 { 26.0 } else { 40.0 } {
            "minecraft:deep_lukewarm_ocean"
        } else if temperature > 21.0 && deep <= 13.0 {
            "minecraft:warm_ocean"
        } else {
            "minecraft:lukewarm_ocean"
        }
    } else if deep > 40.0 {
        "minecraft:deep_ocean"
    } else {
        "minecraft:ocean"
    };
    let floor = if depth < 30 { Sand } else { Gravel };
    s(biome, floor, floor)
}

/// A lake or river's biome and bed.
pub fn lake(temperature: f64) -> Surface {
    use Block::*;
    if temperature < 0.0 {
        s("minecraft:frozen_river", Gravel, Gravel)
    } else {
        s("minecraft:river", Sand, Dirt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(zone: Zone, temperature: f64) -> Local {
        Local {
            zone,
            temperature,
            warmest: temperature + 8.0,
            rain: 900.0,
            latitude: 40.0,
        }
    }
    fn on(code: u8, zone: Zone, t: f64, steep: f64) -> Surface {
        ground(&Ground {
            code,
            local: local(zone, t),
            steep,
            above_sea: 200,
            shore: false,
            roll: 60,
            patch: 0.0,
        })
    }

    #[test]
    fn the_climate_picks_the_forest() {
        assert_eq!(
            on(wc::TREE_COVER, Zone::Rainforest, 26.0, 0.1).biome,
            "minecraft:jungle"
        );
        assert_eq!(
            on(wc::TREE_COVER, Zone::Subarctic, -1.0, 0.1).biome,
            "minecraft:taiga"
        );
        assert_eq!(
            on(wc::TREE_COVER, Zone::Subarctic, -6.0, 0.1).biome,
            "minecraft:snowy_taiga"
        );
        assert_eq!(
            on(wc::TREE_COVER, Zone::Oceanic, 10.0, 0.1).biome,
            "minecraft:forest"
        );
    }

    #[test]
    fn steep_ground_is_rock_and_beaches_are_sand() {
        assert_eq!(
            on(wc::GRASSLAND, Zone::Continental, 8.0, 2.0).top,
            Block::Stone
        );
        assert_eq!(
            on(wc::BARE, Zone::HotDesert, 25.0, 0.1).biome,
            "minecraft:desert"
        );
        let beach = ground(&Ground {
            code: wc::GRASSLAND,
            local: local(Zone::Subtropical, 18.0),
            steep: 0.1,
            above_sea: 1,
            shore: true,
            roll: 3,
            patch: 0.0,
        });
        assert_eq!((beach.biome, beach.top), ("minecraft:beach", Block::Sand));
        assert_eq!(sea(25.0, 5, 10.0, 0.0).biome, "minecraft:warm_ocean");
        assert_eq!(sea(2.0, 60, 60.0, 0.0).biome, "minecraft:deep_cold_ocean");
    }
}
