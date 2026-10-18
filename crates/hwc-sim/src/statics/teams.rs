//! Team colours (`TeamColors.script`, read by `teReset`).

use hwc_formats::script::ScriptFile;

use super::cscan::{scan, scan_i32, scan_rgb, Scanned};

/// Number of players / colour schemes.
pub const MAX_PLAYERS: usize = 8;

/// RGBA colour.
pub type Color = [u8; 4];

/// One player's colours (0x30 bytes each).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TeamColorScheme {
    /// `BaseColor`: hull colour.
    pub base: Color,
    /// `StripeColor`: trim colour.
    pub stripe: Color,
    /// `TrailColor` at the four control points along an engine trail.
    pub trail: [Color; 4],
}

/// Hue/luminance/saturation adjustment applied to the base colour.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HlsColorize {
    pub hue: f32,
    pub lum: f32,
    pub sat: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TeamColors {
    pub schemes: [TeamColorScheme; MAX_PLAYERS],
    /// `SpecularAdjust`, `DiffuseAdjust`, `AmbientAdjust` (16-bit).
    pub specular_adjust: i16,
    pub diffuse_adjust: i16,
    pub ambient_adjust: i16,
    /// `BaseToTrailFactors0..3`.
    pub base_to_trail: [HlsColorize; 4],
    /// Standard sensor/overlay dot colours: `FriendlyColor`, `AlliedColor`, `HostileColor`,
    /// `NeutralColor`, `CtateColor` (sic), `ResourceColor`.
    pub friendly: Color,
    pub allied: Color,
    pub hostile: Color,
    pub neutral: Color,
    pub crate_color: Color,
    pub resource: Color,
}

fn rgba(v: &[Scanned], first: usize) -> Color {
    let c = |i: usize| v.get(first + i).and_then(Scanned::int).unwrap_or(0) as u8;
    [c(0), c(1), c(2), 255]
}

impl TeamColors {
    /// Reads the script. HW1's `TOColor` and `BaseToTOFactors` are no longer bound in Cataclysm,
    /// so those lines are ignored.
    pub fn parse(script: &ScriptFile) -> Self {
        let mut t = TeamColors::default();
        for e in script.entries() {
            let raw = e.raw.as_str();
            match e.key.as_str() {
                "TrailColor" => {
                    let v = scan(raw, "%d,%d,%d,%d,%d");
                    if v.len() == 5 {
                        let (p, i) = (v[0].int().unwrap_or(-1), v[1].int().unwrap_or(-1));
                        if let (Ok(p), Ok(i)) = (usize::try_from(p), usize::try_from(i)) {
                            if p < MAX_PLAYERS && i < 4 {
                                t.schemes[p].trail[i] = rgba(&v, 2);
                            }
                        }
                    }
                }
                key @ ("BaseColor" | "StripeColor") => {
                    let v = scan(raw, "%d,%d,%d,%d");
                    if v.len() == 4 {
                        if let Some(p) = v[0].int().and_then(|p| usize::try_from(p).ok()).filter(|&p| p < MAX_PLAYERS) {
                            let target = if key == "BaseColor" { &mut t.schemes[p].base } else { &mut t.schemes[p].stripe };
                            *target = rgba(&v, 1);
                        }
                    }
                }
                key @ ("SpecularAdjust" | "DiffuseAdjust" | "AmbientAdjust") => {
                    if let Some(v) = scan_i32(raw) {
                        *match key {
                            "SpecularAdjust" => &mut t.specular_adjust,
                            "DiffuseAdjust" => &mut t.diffuse_adjust,
                            _ => &mut t.ambient_adjust,
                        } = v as i16;
                    }
                }
                key if key.starts_with("BaseToTrailFactors") => {
                    let Some(i) = key["BaseToTrailFactors".len()..].parse::<usize>().ok().filter(|&i| i < 4) else {
                        continue;
                    };
                    if key.len() != "BaseToTrailFactors".len() + 1 {
                        continue;
                    }
                    let v = scan(raw, "%f,%f,%f");
                    let f = |n: usize| v.get(n).and_then(Scanned::float);
                    let c = &mut t.base_to_trail[i];
                    if let Some(x) = f(0) {
                        c.hue = x;
                    }
                    if let Some(x) = f(1) {
                        c.lum = x;
                    }
                    if let Some(x) = f(2) {
                        c.sat = x;
                    }
                }
                key @ ("FriendlyColor" | "AlliedColor" | "HostileColor" | "NeutralColor" | "CtateColor"
                | "ResourceColor") => {
                    if let Some(c) = scan_rgb(raw) {
                        *match key {
                            "FriendlyColor" => &mut t.friendly,
                            "AlliedColor" => &mut t.allied,
                            "HostileColor" => &mut t.hostile,
                            "NeutralColor" => &mut t.neutral,
                            "CtateColor" => &mut t.crate_color,
                            _ => &mut t.resource,
                        } = c;
                    }
                }
                _ => {}
            }
        }
        t
    }

    /// The multiplayer adjustment `teReset` makes when the local player picked the Beast:
    /// schemes 0 and 1 swap so the local player keeps scheme 0's colours.
    pub fn swap_for_beast_player(&mut self) {
        self.schemes.swap(0, 1);
    }
}
