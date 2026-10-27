//! Optional Homeworld Remastered ship deaths and engine trails (`hwcRemasteredFx`, off by
//! default; a user-requested change).
//!
//! Needs the effects imported from the player's own Remastered install (`hwc-import --hwr`,
//! see [`hwc_hwr::import`]); without them the stock effects stay. Only the drawing changes: the
//! simulation still starts and runs the original death effects (they time the hulk's removal and
//! are saved with the game), and here their primitives are left out of the render list while a
//! Remastered death sequence plays at the ship instead. Ribbon engine trails keep the original's
//! recorded nozzle path and gating and are drawn with the Remastered ribbon texture and shading.
//!
//! Each Cataclysm ship takes the death sequence and trail of the closest Remastered Homeworld 1
//! ship by class (Kushan, or Taiidan for the Taiidan, Turanic and Beast).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use glam::{Mat3, Vec2, Vec3};
use hwc_hwr::fx::BlendMode;
use hwc_hwr::player::{Anchor, DrawList};
use hwc_hwr::{Library, Player, TexId};
use hwc_render::effects::{
    Blend, EffectBatch, EffectTexture, EffectVertex, Sprite, Strip, Triangle,
};
use hwc_render::trails::RibbonStyle;
use hwc_render::Renderer;
use hwc_sim::effects::{FxOwner, SOF_FORCE_VISIBLE};
use hwc_sim::statics::{ShipClass, ShipRace, ShipType};
use hwc_sim::{ShipId, Universe};

/// Cataclysm units per Remastered unit: a Cataclysm interceptor's collision radius is about
/// 36, the effects of a Remastered interceptor are sized for about 8.
const UNIT_SCALE: f32 = 4.5;
/// `fx_scale = "entity"` events are scaled by the ship's collision radius over this.
const ENTITY_RADIUS: f32 = 8.0;
/// The part of a dying ship's velocity its explosion keeps.
const DEATH_DRIFT: f32 = 0.35;
/// Owner of effects that follow nothing.
const NO_OWNER: u64 = u64::MAX;
/// Remastered trail widths to Cataclysm units.
const TRAIL_WIDTH_SCALE: f32 = UNIT_SCALE;

/// Where an import is looked for: `$HWC_HWR_DIR`, then `hwr` in the game data folder, then
/// `hwr` next to the executable.
pub fn find_dir(data_root: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("HWC_HWR_DIR") {
        candidates.push(PathBuf::from(d));
    }
    if let Some(r) = data_root {
        candidates.push(r.join(hwc_hwr::import::DATA_DIR));
    }
    if let Some(exe) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        candidates.push(exe.join(hwc_hwr::import::DATA_DIR));
    }
    candidates.into_iter().find(|d| Library::exists(d))
}

/// The Remastered ship standing in for a Cataclysm ship.
pub fn stand_in(
    class: Option<ShipClass>,
    race: ShipRace,
    radius: f32,
) -> (&'static str, &'static str) {
    let prefix = match race {
        ShipRace::Taiidan | ShipRace::Turanic | ShipRace::Beast => "tai_",
        _ => "kus_",
    };
    let base = match class {
        Some(ShipClass::Fighter) => "interceptor",
        Some(ShipClass::Corvette) => "heavycorvette",
        Some(ShipClass::Frigate) => "assaultfrigate",
        Some(ShipClass::Destroyer) => "destroyer",
        Some(ShipClass::HeavyCruiser) => "heavycruiser",
        Some(ShipClass::Carrier) => "carrier",
        Some(ShipClass::Mothership) => "mothership",
        Some(ShipClass::Resource) => "resourcecollector",
        _ if radius < 60.0 => "lightcorvette",
        _ if radius < 200.0 => "supportfrigate",
        _ if radius < 500.0 => "destroyer",
        _ => "heavycruiser",
    };
    (prefix, base)
}

/// The running Remastered effects of a world.
pub struct HwrFx {
    player: Player,
    /// Presentation clock of the last draw.
    clock: Option<f64>,
    /// Hulks whose death sequence started.
    started: BTreeSet<ShipId>,
    /// The original death effects being hidden (and their spawns, through `family`).
    hidden_roots: BTreeSet<u32>,
    hidden_owners: BTreeSet<u32>,
    /// Uploaded textures by (texture, additive); `None` for art that failed to load.
    textures: HashMap<(TexId, bool), Option<EffectTexture>>,
    /// Textures drawn without being uploaded yet (uploaded at the next prepare).
    wanted: BTreeSet<(TexId, bool)>,
    /// Ribbon look per ship type (`None`: the stock trail).
    ribbons: BTreeMap<ShipType, Option<RibbonStyle>>,
    ramps: HashMap<&'static str, Vec<f32>>,
    ribbon_tex: Option<TexId>,
    glow_tex: Option<TexId>,
    draw: DrawList,
}

impl HwrFx {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let mut lib = Library::open(dir).map_err(|e| e.to_string())?;
        let ribbon_tex = match lib.texture("effect/trails/trail_ribbon.tga") {
            Some(hwc_hwr::library::TexRef::Still(id)) => Some(id),
            _ => None,
        };
        let glow_tex = match lib.texture("effect/trails/glowtex.tga") {
            Some(hwc_hwr::library::TexRef::Still(id)) => Some(id),
            _ => None,
        };
        let mut ramps = HashMap::new();
        for race in ["kus_", "tai_"] {
            let ramp = match lib.texture(&format!("effect/trails/{race}trail_clr.tga")) {
                Some(hwc_hwr::library::TexRef::Still(id)) => {
                    lib.load_image(id).map(|img| ramp_of(&img))
                }
                _ => None,
            };
            ramps.insert(race, ramp.unwrap_or_else(|| vec![1.0, 0.6, 0.3, 0.0]));
        }
        Ok(Self {
            player: Player::new(lib, 0x4857_5246),
            clock: None,
            started: BTreeSet::new(),
            hidden_roots: BTreeSet::new(),
            hidden_owners: BTreeSet::new(),
            textures: HashMap::new(),
            wanted: BTreeSet::new(),
            ribbons: BTreeMap::new(),
            ramps,
            ribbon_tex,
            glow_tex,
            draw: DrawList::default(),
        })
    }

    fn sequence_for(
        &self,
        u: &Universe,
        t: ShipType,
    ) -> Option<(hwc_hwr::ships::DeathSequence, f32)> {
        let st = u.statics.ship(t)?;
        let radius = st.header.coll.coll_sphere_size.max(1.0);
        let (prefix, base) = stand_in(st.ship_class, st.ship_race, radius);
        let lib = &self.player.lib;
        let seq = lib
            .death(&format!("{prefix}{base}"))
            .or_else(|| lib.death(&format!("kus_{base}")))
            .filter(|s| !s.events.is_empty())?;
        Some((seq.clone(), radius))
    }

    /// After a universe update: start the death sequence of every new hulk and note its original
    /// death effect for hiding.
    pub fn after_update(&mut self, u: &Universe) {
        self.started.retain(|id| u.hulks.contains(id));
        let fx = u.effects.as_ref();
        for &id in &u.hulks {
            if self.started.contains(&id) {
                continue;
            }
            self.started.insert(id);
            let owner = FxOwner::Ship(id).id();
            if let Some(fx) = fx {
                for e in &fx.world.effects {
                    if e.owner == Some(owner) && e.flags & SOF_FORCE_VISIBLE != 0 {
                        self.hidden_roots.insert(e.id);
                    }
                }
            }
            self.hidden_owners.insert(owner);
            let Some(s) = u.ship(id) else { continue };
            let seq = self.sequence_for(u, s.ship_type);
            if std::env::var_os("HWC_HWR_TRACE").is_some() {
                eprintln!(
                    "hwr: ship {id:?} died at {:?}, sequence {:?}",
                    s.posinfo.position,
                    seq.as_ref().map(|(s, r)| (s.events.len(), *r))
                );
            }
            if let Some((seq, radius)) = seq {
                // The explosion stays where the ship died, drifting on with part of its speed
                // (the hulk itself flies on until the original's death effect removes it).
                let mut anchor = anchor_of(s);
                anchor.velocity *= DEATH_DRIFT;
                self.player.start_death(
                    &seq,
                    NO_OWNER,
                    anchor,
                    radius,
                    radius / ENTITY_RADIUS,
                    UNIT_SCALE,
                );
            }
        }
        // Forget what no longer runs.
        if let Some(fx) = fx {
            let live: BTreeSet<u32> = fx.world.effects.iter().map(|e| e.id).collect();
            self.hidden_roots.retain(|id| live.contains(id));
        }
        let hulk_owners: BTreeSet<u32> = u.hulks.iter().map(|&id| FxOwner::Ship(id).id()).collect();
        self.hidden_owners.retain(|o| hulk_owners.contains(o));
    }

    /// Whether an original effect is replaced (a death effect, its spawns, anything a hulk owns).
    pub fn hides(&self, e: &hwc_fx::runtime::Effect) -> bool {
        self.hidden_roots.contains(&e.id)
            || e.family.is_some_and(|f| self.hidden_roots.contains(&f))
            || e.owner.is_some_and(|o| self.hidden_owners.contains(&o))
    }

    /// Uploads the textures the effects asked for.
    pub fn prepare(&mut self, renderer: &mut Renderer) {
        for (id, additive) in std::mem::take(&mut self.wanted) {
            if self.textures.contains_key(&(id, additive)) {
                continue;
            }
            let tex = self.player.lib.load_image(id).map(|img| {
                let mut rgba = img.rgba;
                if !img.alpha && !additive {
                    // Alpha-blended art without an alpha channel: coverage from brightness.
                    for p in rgba.chunks_mut(4) {
                        p[3] = p[0].max(p[1]).max(p[2]);
                    }
                }
                renderer.create_effect_texture(img.width, img.height, &rgba)
            });
            self.textures.insert((id, additive), tex);
        }
    }

    /// An uploaded texture; `None` (draw nothing) until it is, or when it failed to load.
    fn texture(&mut self, id: TexId, additive: bool) -> Option<EffectTexture> {
        match self.textures.get(&(id, additive)) {
            Some(t) => *t,
            None => {
                self.wanted.insert((id, additive));
                None
            }
        }
    }

    /// The Remastered ribbon look for a ship type's trails, once its texture is uploaded.
    pub fn ribbon(&mut self, u: &Universe, t: ShipType, radius: f32) -> Option<RibbonStyle> {
        if let Some(r) = self.ribbons.get(&t) {
            return r.clone();
        }
        let texture = self.texture(self.ribbon_tex?, true)?;
        let glow = self.glow_tex.and_then(|g| self.texture(g, true));
        let st = u.statics.ship(t);
        let (prefix, base) = st.map_or(("kus_", "interceptor"), |st| {
            stand_in(st.ship_class, st.ship_race, radius)
        });
        let lib = &self.player.lib;
        let trail = lib
            .engine_trails(&format!("{prefix}{base}"))
            .first()
            .or_else(|| lib.engine_trails(&format!("kus_{base}")).first())
            .cloned();
        let style = trail.map(|tr| {
            let width = tr.width * TRAIL_WIDTH_SCALE;
            RibbonStyle {
                texture,
                width,
                ramp: self.ramps.get(prefix).cloned().unwrap_or_default(),
                max_points: (tr.length * 10.0).clamp(8.0, 38.0) as usize,
                glow: glow.map(|g| (g, width * 1.5)),
                hot: 0.55,
                blend: Blend::Additive,
            }
        });
        self.ribbons.insert(t, style.clone());
        style
    }

    /// Advances the effects to presentation time `now` (seconds) and draws them into `out`.
    pub fn draw(&mut self, u: &Universe, now: f64, out: &mut EffectBatch) {
        // Effects follow the game clock in steps of at most a thirtieth of a second (at most
        // two seconds' worth per frame, so a stall does not stack up).
        let mut left = self.clock.map_or(0.0, |c| (now - c).clamp(0.0, 2.0)) as f32;
        self.clock = Some(now);
        while left > 1e-4 {
            let dt = left.min(1.0 / 30.0);
            left -= dt;
            self.player.update(dt, |owner| {
                if owner == NO_OWNER {
                    None
                } else {
                    u.ship(ShipId(owner as u16)).map(anchor_of)
                }
            });
        }
        let mut list = std::mem::take(&mut self.draw);
        list.clear();
        self.player.draw(&mut list);
        for s in &list.sprites {
            let add = s.blend == BlendMode::Additive;
            let Some(texture) = self.texture(s.texture, add) else {
                continue;
            };
            out.sprites.push(Sprite {
                position: s.position,
                half_size: Vec2::splat(s.half_size),
                rotation: s.rotation,
                color: s.color,
                uv: s.uv,
                texture,
                blend: blend(s.blend),
            });
        }
        for q in &list.quads {
            let add = q.blend == BlendMode::Additive;
            let Some(texture) = self.texture(q.texture, add) else {
                continue;
            };
            let [u0, v0, u1, v1] = q.uv;
            let uv = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
            let v = |i: usize| EffectVertex {
                position: q.corners[i],
                color: q.color,
                uv: uv[i],
            };
            for [a, b, c] in [[0, 1, 2], [0, 2, 3]] {
                out.triangles.push(Triangle {
                    vertices: [v(a), v(b), v(c)],
                    texture,
                    blend: blend(q.blend),
                });
            }
        }
        for s in &list.strips {
            let add = s.blend == BlendMode::Additive;
            let Some(texture) = self.texture(s.texture, add) else {
                continue;
            };
            out.strips.push(Strip {
                points: s.points.clone(),
                width: s.width,
                start_color: s.head,
                end_color: s.tail,
                texture,
                blend: blend(s.blend),
            });
        }
        if std::env::var_os("HWC_HWR_TRACE").is_some() {
            eprintln!(
                "hwr: t {now:.2} active {} drawn {} sprites {} quads {} strips -> batch {} sprites, {} textures wanted",
                self.player.active(),
                list.sprites.len(),
                list.quads.len(),
                list.strips.len(),
                out.sprites.len(),
                self.wanted.len()
            );
        }
        self.draw = list;
    }
}

/// A ship's position, velocity and axes for the effect player.
fn anchor_of(s: &hwc_sim::Ship) -> Anchor {
    let c = &s.rotinfo.coordsys;
    let col = |i: usize| {
        let v = c.col(i);
        Vec3::new(v.x, v.y, v.z)
    };
    // Homeworld's columns are up, right, heading.
    let basis = Mat3::from_cols(col(1), col(0), col(2));
    let p = s.posinfo.position;
    let v = s.posinfo.velocity;
    Anchor {
        position: Vec3::new(p.x, p.y, p.z),
        velocity: Vec3::new(v.x, v.y, v.z),
        basis,
    }
}

fn blend(b: BlendMode) -> Blend {
    match b {
        BlendMode::Additive => Blend::Additive,
        BlendMode::Alpha => Blend::Alpha,
    }
}

/// Brightness along a `*_trail_clr` ramp (dark at the left, bright at the right), nozzle first,
/// normalised to 1.
fn ramp_of(img: &hwc_hwr::Image) -> Vec<f32> {
    let y = img.height / 2;
    let samples = 8;
    let mut v: Vec<f32> = (0..samples)
        .map(|i| {
            let x = ((samples - 1 - i) * (img.width - 1) as usize / (samples - 1)) as u32;
            let o = ((y * img.width + x) * 4) as usize;
            f32::from(img.rgba[o].max(img.rgba[o + 1]).max(img.rgba[o + 2]))
        })
        .collect();
    let max = v.iter().copied().fold(0.0, f32::max);
    if max > 0.0 {
        v.iter_mut().for_each(|x| *x /= max);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stand_ins() {
        assert_eq!(
            stand_in(Some(ShipClass::Fighter), ShipRace::Somtaaw, 10.0),
            ("kus_", "interceptor")
        );
        assert_eq!(
            stand_in(Some(ShipClass::Destroyer), ShipRace::Turanic, 300.0),
            ("tai_", "destroyer")
        );
        assert_eq!(
            stand_in(None, ShipRace::Traders, 100.0),
            ("kus_", "supportfrigate")
        );
    }

    #[test]
    fn ramp_runs_bright_to_dark() {
        let img = hwc_hwr::Image {
            width: 4,
            height: 1,
            rgba: vec![
                0, 0, 0, 255, 10, 10, 10, 255, 20, 20, 20, 255, 40, 40, 40, 255,
            ],
            alpha: false,
        };
        let r = ramp_of(&img);
        assert_eq!(r.first(), Some(&1.0));
        assert_eq!(r.last(), Some(&0.0));
    }
}
