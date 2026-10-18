//! Navigation lights (`NUMBER_OF_NAV_LIGHTS` / `NavLight` lines, `scriptSetNAVLightStatics`)
//! completed from the `Nav` chunks of the mesh extensions (`mexGetNAVLightStaticInfo`).

use hwc_formats::mex::{ChunkData, Mex};
use hwc_formats::script::ScriptFile;

use super::cscan::{decomma, scan, scan_i32, Scanned};
use super::enums::NavLightType;
use super::math::Vec3;

/// `NAVLightStatic` (0x48 bytes).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NavLightStatic {
    /// +0x00. `None` when the type name was not recognised.
    pub light_type: Option<NavLightType>,
    /// +0x04. From the mex.
    pub position: Vec3,
    /// +0x10. RGBA from the mex (alpha 255).
    pub color: [u8; 4],
    /// +0x14..+0x20: `flashrateon`, `flashrateoff`, `startdelay` (seconds), `size` (metres).
    pub flash_rate_on: f32,
    pub flash_rate_off: f32,
    pub start_delay: f32,
    pub size: f32,
    /// +0x24. Lowest LOD the light is drawn at.
    pub min_lod: i32,
    /// +0x28. Light name, matched against the mex.
    pub name: String,
    /// +0x3c. Texture file name (`None` for `NULL`; the original registers the texture and keeps
    /// its handle).
    pub texture: Option<String>,
    /// Mesh object the light follows (Cataclysm). The original resolves it to a per-LOD
    /// hierarchy index (+0x40); that needs the meshes and is not done here.
    pub attached_to: Option<String>,
    /// True when the `Nav` chunk was found.
    pub mex_point_found: bool,
}

/// Reads the nav-light lines. `None` without a positive `NUMBER_OF_NAV_LIGHTS`.
pub(crate) fn scan_nav_lights(script: &ScriptFile) -> Option<Vec<NavLightStatic>> {
    let mut lights: Option<Vec<NavLightStatic>> = None;
    let mut filled = 0usize;
    for e in script.entries() {
        match lights.as_mut() {
            None => {
                if e.key == "NUMBER_OF_NAV_LIGHTS" {
                    let n = scan_i32(&e.raw).unwrap_or(0);
                    if n < 1 {
                        return None;
                    }
                    lights = Some(vec![NavLightStatic::default(); n as usize]);
                }
            }
            Some(list) => {
                if e.key != "NavLight" {
                    continue;
                }
                let l = &mut list[filled];
                let v = scan(&decomma(&e.raw), "%s%s%f%f%f%f%d%s");
                if let Some(s) = v.first().and_then(Scanned::str) {
                    l.name = s.to_owned();
                }
                let float = |i: usize, t: &mut f32| {
                    if let Some(x) = v.get(i).and_then(Scanned::float) {
                        *t = x;
                    }
                };
                float(2, &mut l.flash_rate_on);
                float(3, &mut l.flash_rate_off);
                float(4, &mut l.start_delay);
                float(5, &mut l.size);
                if let Some(x) = v.get(6).and_then(Scanned::int) {
                    l.min_lod = x;
                }
                l.texture = v.get(7).and_then(Scanned::str).filter(|t| *t != "NULL").map(str::to_owned);
                l.light_type = v.get(1).and_then(Scanned::str).and_then(NavLightType::from_name);
                filled += 1;
                if filled >= list.len() {
                    break;
                }
            }
        }
    }
    lights
}

/// `mexGetNAVLightStaticInfo`: position, colour and attachment of each light from the `Nav`
/// chunk (named `Nav`) carrying the light's name.
pub(crate) fn apply_mex(lights: &mut [NavLightStatic], mex: &Mex, warnings: &mut Vec<String>) {
    for l in lights {
        let found = mex.chunks.iter().find_map(|c| match &c.data {
            ChunkData::NavLight { position, color, name, attached_to }
                if c.tag.eq_ignore_ascii_case("Nav") && c.name.eq_ignore_ascii_case("Nav") && name.eq_ignore_ascii_case(&l.name) =>
            {
                Some((*position, *color, attached_to.clone()))
            }
            _ => None,
        });
        match found {
            Some((position, [r, g, b], attached)) => {
                l.position = position;
                l.color = [r, g, b, 255];
                l.attached_to = attached.filter(|a| !a.is_empty());
                l.mex_point_found = true;
            }
            None => warnings.push(format!("nav light {} not in mex", l.name)),
        }
    }
}
