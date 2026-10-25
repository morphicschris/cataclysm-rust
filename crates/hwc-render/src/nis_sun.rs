//! The NIS sun lens flare (`lensFlareNISSunAdd` 0x577440, `lensFlareNISSunRemove` 0x577490,
//! `lensFlareNISSunSetPosition` 0x5774a0, `lensFlareNISSunSetVisible` 0x5774c0).
//!
//! Cataclysm keeps one extra lens flare of its own for the NIS: `nisStart` appends it to the
//! lens-flare list (after the particle systems' flares), `nisUpdate` moves it to the NIS object
//! of type `NISlensflareSource1` every step, `nisObjectShow` / `nisObjectHide` of that object
//! fade it in and out, and `nisStop` takes it off the list. It is drawn by the same pass as
//! the other flares ([`crate::fx::lens_flare_pass`]), with its own fade state, and is never
//! tested for occlusion (it is not a newly added particle flare) nor marked as undrawn after a
//! frame (it has no particle system).

use glam::Vec3;
use hwc_fx::particle::LensFlare;
use hwc_math::Vector;

use crate::fx::{lens_flare_pass, FlareState, FxFrameData, FxTexture, FxView, LensFlareSet};

/// The NIS sun flare (the static flare record at 0x9dcee0 and its list node).
#[derive(Debug, Clone, PartialEq)]
pub struct NisSunFlare {
    pub flare: LensFlare,
    /// The flare's fade and screen state (+0x18 onwards in the record).
    pub state: FlareState,
    /// On the lens-flare list (between [`add`](Self::add) and [`remove`](Self::remove)).
    pub listed: bool,
}

impl Default for NisSunFlare {
    fn default() -> Self {
        Self {
            flare: LensFlare { position: Vector::ZERO, intensity: 0.0, enabled: 0, pending: 0, active: 0 },
            state: FlareState::default(),
            listed: false,
        }
    }
}

impl NisSunFlare {
    /// `lensFlareNISSunAdd` (`nisStart`): onto the list, not enabled until first shown, full
    /// intensity, faded out. The position is kept from before.
    pub fn add(&mut self) {
        self.listed = true;
        self.flare.active = 0;
        self.flare.enabled = 0;
        self.flare.pending = 0;
        self.flare.intensity = 1.0;
        self.state.fade = 0.0;
    }

    /// `lensFlareNISSunRemove` (`nisStop`): off the list.
    pub fn remove(&mut self) {
        self.listed = false;
    }

    /// `lensFlareNISSunSetPosition` (`nisUpdate`, for the `NISlensflareSource1` object).
    pub fn set_position(&mut self, position: Vec3) {
        self.flare.position = Vector::new(position.x, position.y, position.z);
    }

    /// `lensFlareNISSunSetVisible` (`nisObjectShow` / `nisObjectHide` of the flare source):
    /// with lens flares on (`enabled`, the options setting), the flare is enabled and fades
    /// in (`visible`) or out. With lens flares off nothing changes.
    pub fn set_visible(&mut self, enabled: bool, visible: bool) {
        if enabled {
            self.flare.pending = u8::from(!visible);
            self.flare.enabled = 1;
        }
    }
}

/// [`lens_flare_pass`] over `flares` followed by the NIS sun when it is listed, the sun with
/// its own state. `states` are the particle flares' states (as kept by the renderer).
#[allow(clippy::too_many_arguments)]
pub fn lens_flare_pass_with_nis_sun(
    flares: &mut [LensFlare],
    states: &mut Vec<FlareState>,
    sun: Option<&mut NisSunFlare>,
    set: &LensFlareSet,
    texture: Option<FxTexture>,
    view: &FxView,
    occluded: Option<&dyn Fn(Vec3, Vec3, f32) -> bool>,
    out: &mut FxFrameData,
) -> Vec<usize> {
    let Some(sun) = sun.filter(|s| s.listed) else {
        return lens_flare_pass(flares, states, set, texture, view, occluded, out);
    };
    let n = flares.len();
    let mut all: Vec<LensFlare> = flares.to_vec();
    all.push(sun.flare.clone());
    states.resize(n, FlareState::default());
    states.push(sun.state);
    let slots = lens_flare_pass(&mut all, states, set, texture, view, occluded, out);
    sun.state = states.pop().unwrap_or_default();
    sun.flare = all.pop().expect("sun flare");
    flares.clone_from_slice(&all);
    slots
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Vec2};

    use super::*;
    use crate::fx::FlareElement;

    fn view() -> FxView {
        let v = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 100.0), Vec3::ZERO, Vec3::Y);
        let p = Mat4::perspective_rh(1.0, 1.0, 1.0, 1000.0);
        FxView::new(v, p, 200, 200)
    }

    fn set() -> LensFlareSet {
        LensFlareSet {
            elements: vec![FlareElement { radius: 0.05, offset: 1.0, image: -2, color: [255, 255, 255] }],
            rects: Vec::new(),
            texture_file: None,
            fadeout_frame_size: 1.5,
        }
    }

    #[test]
    fn add_show_hide_and_remove() {
        let mut s = NisSunFlare::default();
        s.set_position(Vec3::new(1.0, 2.0, 3.0));
        s.state.fade = 0.7;
        s.add();
        assert!(s.listed);
        assert_eq!((s.flare.enabled, s.flare.pending, s.flare.intensity, s.state.fade), (0, 0, 1.0, 0.0));
        assert_eq!(s.flare.position, Vector::new(1.0, 2.0, 3.0));
        s.set_visible(false, true);
        assert_eq!(s.flare.enabled, 0, "lens flares off");
        s.set_visible(true, true);
        assert_eq!((s.flare.enabled, s.flare.pending), (1, 0));
        s.set_visible(true, false);
        assert_eq!((s.flare.enabled, s.flare.pending), (1, 1));
        s.remove();
        assert!(!s.listed);
    }

    #[test]
    fn the_sun_fades_in_after_the_other_flares_and_keeps_its_state() {
        let mut flares = vec![LensFlare { position: Vector::new(5.0, 0.0, 0.0), intensity: 1.0, enabled: 1, pending: 0, active: 1 }];
        let mut states = Vec::new();
        let mut sun = NisSunFlare::default();
        sun.add();
        sun.set_visible(true, true);
        let mut out = FxFrameData::default();
        let slots = lens_flare_pass_with_nis_sun(&mut flares, &mut states, Some(&mut sun), &set(), None, &view(), None, &mut out);
        assert_eq!(slots, vec![0, 1]);
        assert_eq!(states.len(), 1);
        assert!((sun.state.fade - 0.4).abs() < 1e-6);
        assert!(sun.state.visible);
        assert_eq!(sun.state.screen, Vec2::new(100.0, 100.0));
        // The particle flare was marked undrawn; the sun has no particle system.
        assert_eq!(flares[0].pending, 1);
        assert_eq!(sun.flare.pending, 0);
        // Hidden: it fades out.
        sun.set_visible(true, false);
        let mut out = FxFrameData::default();
        lens_flare_pass_with_nis_sun(&mut flares, &mut states, Some(&mut sun), &set(), None, &view(), None, &mut out);
        assert!((sun.state.fade - 0.24).abs() < 1e-6);
        // Off the list: only the particle flares.
        sun.remove();
        let slots = lens_flare_pass_with_nis_sun(&mut flares, &mut states, Some(&mut sun), &set(), None, &view(), None, &mut out);
        assert_eq!(slots, vec![0]);
    }
}
