//! Static game data: what `universeStaticInit` and the startup tweak/research/caps/colour loaders
//! build in memory, as typed Rust data.
//!
//! [`StaticWorld::load`] reads everything for all races from the game archive. The original loads
//! ship statics on demand (only types a level needs); loading all of them gives the same values
//! per type, since each type's load is independent of the others.

pub mod caps;
pub mod cscan;
pub mod derelict;
pub mod dock;
pub mod enums;
pub mod gun;
pub mod header;
pub mod hull;
pub mod lod;
pub mod missile;
pub mod navlight;
pub mod research;
pub mod resource;
pub mod ship;
pub mod source;
pub mod teams;
pub mod tweaks;

use std::collections::BTreeMap;
use std::sync::Arc;

use hwc_formats::big::{normalize_name, BigArchive};
use hwc_formats::mex::Mex;
use hwc_formats::script::ScriptFile;

pub use caps::UnitCaps;
pub use derelict::DerelictStaticInfo;
pub use dock::{DockStaticInfo, DockStaticPoint};
pub use enums::*;
pub use gun::GunStatic;
pub use header::{GuidanceInfo, HealthInfo, StaticCollInfo, StaticHeader};
pub use lod::{LodInfo, LodLevel, LodType};
pub use missile::MissileStaticInfo;
pub use navlight::NavLightStatic;
pub use research::{ResearchStatics, ResearchTable, TechSet};
pub use resource::{AsteroidBreakInfo, AsteroidStaticInfo, DustCloudStaticInfo, NebulaStaticInfo, ResourceInfo};
pub use ship::ShipStaticInfo;
pub use source::{DataSource, DirSource};
pub use teams::TeamColors;
pub use tweaks::Tweaks;

use header::StaticHeader as Header;

/// A mesh-extension point (`[x, y, z]`) as a vector.
pub(crate) fn v3(a: [f32; 3]) -> hwc_math::Vector {
    hwc_math::Vector::new(a[0], a[1], a[2])
}

/// Choices the original makes from game state while loading.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadOptions {
    /// Single-player campaign: three hard-coded build costs change and Somtaaw ship health is
    /// scaled by `single_player_health_factor`.
    pub single_player: bool,
    /// The campaign's health factor for the player's (Somtaaw) ships; set by the mission when a
    /// game starts, 1 by default.
    pub single_player_health_factor: f32,
    /// The original's debug switch that stubs every ship out (all types disabled).
    pub force_default_ship: bool,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self { single_player: false, single_player_health_factor: 1.0, force_default_ship: false }
    }
}

/// Shared state while loading: data source, tweak values, default assets and warnings.
pub(crate) struct Loader<'a> {
    pub src: &'a dyn DataSource,
    pub tweaks: Tweaks,
    pub opts: LoadOptions,
    pub default_lod: Option<Arc<LodInfo>>,
    pub default_mex: Option<Arc<Mex>>,
    mex_cache: BTreeMap<String, Arc<Mex>>,
    pub warnings: Vec<String>,
}

impl<'a> Loader<'a> {
    fn new(src: &'a dyn DataSource, tweaks: Tweaks, opts: LoadOptions) -> Self {
        let mut ld = Loader {
            src,
            tweaks,
            opts,
            default_lod: None,
            default_mex: None,
            mex_cache: BTreeMap::new(),
            warnings: Vec::new(),
        };
        ld.default_lod = LodInfo::load(src, "DefaultShip\\", "DefaultShip.lod").map(Arc::new);
        ld.default_mex = ld.parse_mex("DefaultShip\\DefaultShip.mex");
        if ld.default_lod.is_none() || ld.default_mex.is_none() {
            ld.warn("default ship LOD or mex missing".into());
        }
        ld
    }

    pub fn warn(&mut self, w: String) {
        self.warnings.push(w);
    }

    pub fn read_script(&mut self, path: &str) -> Option<ScriptFile> {
        let bytes = self.src.read(path)?;
        match ScriptFile::parse_bytes(&bytes) {
            Ok(s) => Some(s),
            Err(e) => {
                self.warn(format!("{path}: {e}"));
                None
            }
        }
    }

    fn parse_mex(&mut self, path: &str) -> Option<Arc<Mex>> {
        let key = normalize_name(path);
        if let Some(m) = self.mex_cache.get(&key) {
            return Some(m.clone());
        }
        let bytes = self.src.read(path)?;
        match Mex::parse(&bytes) {
            Ok(m) => {
                let m = Arc::new(m);
                self.mex_cache.insert(key, m.clone());
                Some(m)
            }
            Err(e) => {
                self.warn(format!("{path}: {e}"));
                None
            }
        }
    }

    /// `LODFile` and `pMexData`, which every object table binds. `DefaultShip` selects the
    /// default ship's data; a missing mex also falls back to it.
    pub fn apply_header_files(&mut self, h: &mut Header, key: &str, raw: &str, dir: &str) -> bool {
        match key {
            "LODFile" => {
                h.lod_file = Some(raw.to_owned());
                h.lod = if raw.eq_ignore_ascii_case("DefaultShip") {
                    self.default_lod.clone()
                } else {
                    let lod = LodInfo::load(self.src, dir, raw).map(Arc::new);
                    if lod.is_none() {
                        self.warn(format!("LOD file {dir}{raw} missing"));
                    }
                    lod
                };
            }
            "pMexData" => {
                h.mex_file = Some(raw.to_owned());
                let path = format!("{dir}{raw}");
                let named = if raw.eq_ignore_ascii_case("DefaultShip") || !self.src.exists(&path) {
                    if !raw.eq_ignore_ascii_case("DefaultShip") {
                        self.warn(format!("mex {path} missing; using the default ship's"));
                    }
                    None
                } else {
                    self.parse_mex(&path)
                };
                h.mex_is_default = named.is_none();
                h.mex = named.or_else(|| self.default_mex.clone());
            }
            _ => return false,
        }
        true
    }
}

/// Everything loaded at startup.
#[derive(Debug, Clone)]
pub struct StaticWorld {
    pub options: LoadOptions,
    /// Global tunables after the startup tweak scripts.
    pub tweaks: Tweaks,
    /// Per ship type (index = [`ShipType::index`]); `None` where the race has no file for the type
    /// (the original disables such types).
    pub ships: Vec<Option<ShipStaticInfo>>,
    /// Per race (index = [`ShipRace::index`]).
    pub missiles: Vec<Option<MissileStaticInfo>>,
    pub mines: Vec<Option<MissileStaticInfo>>,
    /// Twice the largest mine collision radius (used to size mine searches).
    pub mine_max_diameter: f32,
    pub asteroids: Vec<Option<AsteroidStaticInfo>>,
    pub dust_clouds: Vec<Option<DustCloudStaticInfo>>,
    pub nebulae: Vec<Option<NebulaStaticInfo>>,
    pub derelicts: Vec<Option<DerelictStaticInfo>>,
    pub research: ResearchStatics,
    /// Caps after `UnitLimitCaps.script`.
    pub unit_caps: UnitCaps,
    /// Caps a game starts with for each `UnitLimitCaps<n>.script` present (layered on
    /// `unit_caps`).
    pub unit_cap_presets: BTreeMap<u32, UnitCaps>,
    pub team_colors: TeamColors,
    /// Battle chatter tunables after `BattleChatter.script`.
    pub battle_chatter: crate::battle_chatter::ChatterTable,
    pub default_lod: Option<Arc<LodInfo>>,
    pub default_mex: Option<Arc<Mex>>,
    /// Problems the original would have reported (or silently tolerated) while loading.
    pub warnings: Vec<String>,
}

impl StaticWorld {
    /// Loads all static data from the game archive with default options.
    pub fn load(big: &BigArchive) -> Self {
        Self::load_from(big, LoadOptions::default())
    }

    /// Loads all static data from any data source.
    pub fn load_from(src: &dyn DataSource, options: LoadOptions) -> Self {
        let tweaks = Tweaks::load(src);
        let mut ld = Loader::new(src, tweaks.clone(), options.clone());

        let mut ships: Vec<Option<ShipStaticInfo>> = ShipType::ALL.iter().map(|&t| ship::load_ship(&mut ld, t)).collect();
        for s in ships.iter_mut().flatten() {
            finish_ship_health(s, &tweaks, &options);
        }

        let missiles = ShipRace::ALL.iter().map(|&r| missile::load_missile(&mut ld, r)).collect();
        let mines: Vec<Option<MissileStaticInfo>> =
            ShipRace::ALL.iter().map(|&r| missile::load_mine(&mut ld, r)).collect();
        let max_mine = mines.iter().flatten().map(|m| m.header.coll.coll_sphere_size).fold(0.0f32, |a, b| if a < b { b } else { a });

        let asteroids = AsteroidType::ALL.iter().map(|&t| resource::load_asteroid(&mut ld, t)).collect();
        let dust_clouds = DustCloudType::ALL.iter().map(|&t| resource::load_dust_cloud(&mut ld, t)).collect();
        let nebulae = NebulaType::ALL.iter().map(|&t| resource::load_nebula(&mut ld, t)).collect();
        let derelicts = DerelictType::ALL.iter().map(|&t| derelict::load_derelict(&mut ld, t)).collect();

        let research = ld.read_script("ResearchDepend.script").map(|s| ResearchStatics::parse(&s)).unwrap_or_default();
        for w in &research.warnings {
            ld.warnings.push(format!("ResearchDepend.script: {w}"));
        }

        let mut unit_caps = UnitCaps::default();
        if let Some(s) = ld.read_script("UnitLimitCaps.script") {
            unit_caps.apply(&s);
        }
        let mut unit_cap_presets = BTreeMap::new();
        for n in 0..10u32 {
            if let Some(s) = ld.read_script(&format!("UnitLimitCaps{n}.script")) {
                let mut caps = unit_caps.clone();
                caps.apply(&s);
                unit_cap_presets.insert(n, caps);
            }
        }
        let team_colors = ld.read_script("TeamColors.script").map(|s| TeamColors::parse(&s)).unwrap_or_default();
        let battle_chatter = crate::battle_chatter::ChatterTable::parse(ld.read_script("BattleChatter.script").as_ref());

        StaticWorld {
            options,
            tweaks,
            ships,
            missiles,
            mines,
            mine_max_diameter: max_mine + max_mine,
            asteroids,
            dust_clouds,
            nebulae,
            derelicts,
            research,
            unit_caps,
            unit_cap_presets,
            team_colors,
            battle_chatter,
            default_lod: ld.default_lod.clone(),
            default_mex: ld.default_mex.clone(),
            warnings: ld.warnings,
        }
    }

    pub fn ship(&self, t: ShipType) -> Option<&ShipStaticInfo> {
        self.ships[t.index()].as_ref()
    }

    /// Loaded ship statics of one race, in type order.
    pub fn ships_of_race(&self, race: ShipRace) -> impl Iterator<Item = &ShipStaticInfo> {
        race.ship_types().filter_map(move |t| self.ship(t))
    }
}

/// The health pass at the end of `universeStaticInit` plus `InitStatShipInfoPost`.
fn finish_ship_health(s: &mut ShipStaticInfo, tw: &Tweaks, opts: &LoadOptions) {
    let h = &mut s.health;
    if tw.global_ship_health_modifier != 1.0 {
        h.max_health *= tw.global_ship_health_modifier;
    }
    if h.max_health == 0.0 {
        h.max_health = 1.0;
    }
    s.base_max_health = h.max_health;
    h.max_health = if opts.single_player && s.ship_race == ShipRace::Somtaaw {
        opts.single_player_health_factor * s.base_max_health
    } else {
        s.base_max_health
    };
    h.one_over_max_health = 1.0 / h.max_health;
}

/// A world holding just `ships` (for unit tests that build their own statics).
#[cfg(test)]
pub(crate) fn test_world(list: Vec<ShipStaticInfo>) -> StaticWorld {
    let mut ships: Vec<Option<ShipStaticInfo>> = vec![None; ShipType::COUNT];
    for s in list {
        let i = s.ship_type.index();
        ships[i] = Some(s);
    }
    StaticWorld {
        options: Default::default(),
        tweaks: Tweaks::default(),
        ships,
        missiles: Vec::new(),
        mines: Vec::new(),
        mine_max_diameter: 0.0,
        asteroids: Vec::new(),
        dust_clouds: Vec::new(),
        nebulae: Vec::new(),
        derelicts: Vec::new(),
        research: Default::default(),
        unit_caps: Default::default(),
        unit_cap_presets: std::collections::BTreeMap::new(),
        team_colors: Default::default(),
        battle_chatter: Default::default(),
        default_lod: None,
        default_mex: None,
        warnings: Vec::new(),
    }
}
