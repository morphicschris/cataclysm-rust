//! Old-style derelicts (`Derelicts\*.shp`, `DerelictStaticInfo`, 0x124 bytes) and the built-in
//! hyperspace gate.

use super::cscan::{parse_bool, scan_f32};
use super::enums::DerelictType;
use super::header::{GuidanceInfo, HeaderKeys, HealthInfo, StaticHeader};
use super::navlight::{self, NavLightStatic};
use super::Loader;

#[derive(Debug, Clone, Default)]
pub struct DerelictStaticInfo {
    pub header: StaticHeader,
    pub health: HealthInfo,
    pub guidance: GuidanceInfo,
    /// +0xf8 / +0xfc / +0x100 `sinbank`, `pitchturn`, `pitchdescend`.
    pub sinbank: f32,
    pub pitchturn: f32,
    pub pitchdescend: f32,
    /// +0x104.
    pub nav_lights: Option<Vec<NavLightStatic>>,
    /// +0x108.
    pub derelict_type: Option<DerelictType>,
    /// +0x10c `salvageable`.
    pub salvageable: bool,
    /// +0x110 `scaleFactor` (world-rendered objects only).
    pub scale_factor: f32,
    /// +0x114 `worldRender`.
    pub world_render: bool,
    /// +0x118 `minimumZoomDistance`.
    pub minimum_zoom_distance: f32,
    /// +0x11c `renderlistFade`.
    pub renderlist_fade: f32,
    /// +0x120 `renderlistLimit`, squared.
    pub renderlist_limit_sqr: f32,
    /// Source file (empty for the hyperspace gate).
    pub file: String,
    /// Lines no table binds.
    pub unbound: Vec<String>,
}

const DERELICT_KEYS: HeaderKeys = HeaderKeys {
    coll_sphere_size: false,
    immobile: true,
    right_of_way: true,
    coll_modifiers: true,
    coll_box_scale: true,
};

impl DerelictStaticInfo {
    fn apply(&mut self, key: &str, raw: &str) -> bool {
        let f = |t: &mut f32| {
            if let Some(v) = scan_f32(raw) {
                *t = v;
            }
        };
        match key {
            "pitchdescend" => f(&mut self.pitchdescend),
            "sinbank" => f(&mut self.sinbank),
            "pitchturn" => f(&mut self.pitchturn),
            "scaleFactor" => f(&mut self.scale_factor),
            "worldRender" => self.world_render = parse_bool(raw),
            "salvageable" => self.salvageable = parse_bool(raw),
            "minimumZoomDistance" => f(&mut self.minimum_zoom_distance),
            "renderlistFade" => f(&mut self.renderlist_fade),
            "renderlistLimit" => {
                if let Some(v) = scan_f32(raw) {
                    self.renderlist_limit_sqr = v * v;
                }
            }
            _ => return self.health.apply(key, raw) || self.guidance.apply(key, raw),
        }
        true
    }
}

/// `Derelicts\<Name>.shp` for every type but the gate.
pub(crate) fn load_derelict(ld: &mut Loader, t: DerelictType) -> Option<DerelictStaticInfo> {
    if t == DerelictType::HyperspaceGate {
        return Some(hyperspace_gate(ld));
    }
    let dir = "Derelicts\\";
    let file = format!("{dir}{}.shp", t.name());
    if !ld.src.exists(&file) {
        return None;
    }
    let script = ld.read_script(&file)?;
    let mut d = DerelictStaticInfo { derelict_type: Some(t), file: file.clone(), ..Default::default() };
    d.header.right_of_way = 1;
    for e in script.entries() {
        let (key, raw) = (e.key.as_str(), e.raw.as_str());
        let bound = ld.apply_header_files(&mut d.header, key, raw, dir)
            || d.header.apply(key, raw, DERELICT_KEYS)
            || d.apply(key, raw)
            || matches!(key, "NUMBER_OF_NAV_LIGHTS" | "NavLight");
        if !bound {
            d.unbound.push(key.to_owned());
        }
    }
    let tw = ld.tweaks.clone();
    d.header.precalc(&tw);
    d.health.precalc(&tw);
    d.guidance.precalc(&mut d.header, &tw);
    d.nav_lights = navlight::scan_nav_lights(&script);
    if let (Some(l), Some(mex)) = (d.nav_lights.as_mut(), d.header.mex.clone()) {
        let mut w = Vec::new();
        navlight::apply_mex(l, &mex, &mut w);
        for w in w {
            ld.warn(format!("{file}: {w}"));
        }
    }
    Some(d)
}

/// `InitStatHyperspaceGateInfo`: a flat box `HYPERSPACEGATE_HEIGHT` x `HYPERSPACEGATE_WIDTH` x 50
/// centred on the origin, with `HYPERSPACEGATE_HEALTH` hit points.
fn hyperspace_gate(ld: &Loader) -> DerelictStaticInfo {
    let tw = &ld.tweaks;
    let mut d = DerelictStaticInfo { derelict_type: Some(DerelictType::HyperspaceGate), ..Default::default() };
    d.health.max_health = tw.hyperspace_gate_health;
    d.health.one_over_max_health = 1.0 / d.health.max_health;
    d.renderlist_limit_sqr = f32::from_bits(0x4dee_6b28);
    let c = &mut d.header.coll;
    c.up_length = tw.hyperspace_gate_height;
    c.right_length = tw.hyperspace_gate_width;
    c.forward_length = 50.0;
    c.coll_rect_offset = [tw.hyperspace_gate_height * -0.5, tw.hyperspace_gate_width * -0.5, -25.0];
    let size = tw.hyperspace_gate_height.max(tw.hyperspace_gate_width);
    c.approx_coll_sphere_size = size;
    c.avoid_coll_sphere_size = size;
    c.avoid_coll_sphere_pad = size;
    c.approx_coll_modifier = 1.0;
    c.avoid_coll_modifier = 1.0;
    c.original_coll_sphere_size = size;
    c.coll_sphere_size = size;
    c.coll_sphere_size_sqr = size * size;
    c.coll_sphere_offset = [0.0; 3];
    d
}
