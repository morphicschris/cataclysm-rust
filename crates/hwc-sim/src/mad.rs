//! Mesh animations as the simulation sees them (`Mesh.c` `mad*` 0x4b1f00..0x4b25c0, the `madLinkIn*`
//! link layer 0x4a0800..0x4a1310). Only the clock lives here: which animation runs, how far along
//! it is, whether it has stopped. Several orders wait on it (a carrier's doors, a worker's jack,
//! an Acolyte's pods, the guns of the ion array frigates), so it is simulation state; the view
//! draws from it.
//!
//! A type's animations are listed in its `.shp`: `NUMBER_OF_ANIMS` and `ANIM i name` number them
//! for the ship modules (`madAnimationStart(ship, i, ..)`); `MadGunAnims` and `MadStartInfo` are
//! the older link layer, which only the ion array frigates use among the fleets.

use hwc_math::random::RAN_GAME;

use crate::object::ShipId;
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// `madAnimationStart` flags: repeat.
pub const LOOP: u32 = 1;
/// `madAnimationStart` flags: forget the animation when it ends (else it holds its last frame).
pub const STOP_AT_END: u32 = 2;

/// The gun states of the link layer (ship +0x304).
pub mod gun {
    pub const OPEN: i32 = 1;
    pub const CLOSED: i32 = 2;
    pub const OPENING: i32 = 3;
    pub const CLOSING: i32 = 4;
}

/// What the link layer does when an animation ends (ship +0x318, +0x31c).
const AFTER_NOTHING: i32 = 0;
const AFTER_OPEN: i32 = 1;
const AFTER_CLOSE: i32 = 2;

/// `madAnimationStart`'s first update starts the animation the link layer chose (+0x300 bit 1).
const BIT_BUSY: u32 = 1;
const BIT_START: u32 = 2;

/// One animation's window on the file's timeline.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Window {
    pub start: f32,
    pub end: f32,
}

/// A type's animations (`madStatic`, static +0x314).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MadStatic {
    /// The file's animations.
    pub windows: Vec<Window>,
    /// `ANIM i name`: ship animation number to file animation (+0x88); `None` without
    /// `NUMBER_OF_ANIMS` (+0x8c 0: the link layer's file numbers are used directly).
    pub remap: Option<Vec<i32>>,
    /// `MadStartInfo`: started then paused when the ship is placed (+8) and built (+0x10).
    pub start_placed: Option<i32>,
    pub start_built: Option<i32>,
    /// `MadGunAnims`: open, close, open damaged, close damaged (+0x24..+0x30).
    pub gun_open: Vec<i32>,
    pub gun_close: Vec<i32>,
    pub gun_open_damaged: Vec<i32>,
    pub gun_close_damaged: Vec<i32>,
    /// `madGunOpenDamagedHealthThreshold` (static +0x318).
    pub damaged_threshold: f32,
}

impl MadStatic {
    /// `setNumberOfAnimsInfo`, `setMadAnimInfo`, `setBindAnimationsInfo`, `setMadStartInfo`,
    /// `setGunAnimInfo`, from the ship file's keys and its `.mad`. `None` when the type has none.
    pub fn load(entries: &[(String, String)], file: Option<&[u8]>) -> Option<MadStatic> {
        let get = |k: &str| entries.iter().find(|(key, _)| key.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str());
        let count: Option<usize> = get("NUMBER_OF_ANIMS").and_then(|v| v.trim().parse().ok());
        if count.is_none_or(|n| n == 0) && get("BindAnimations").is_none() {
            return None;
        }
        let mad = hwc_formats::mad::Mad::parse(file?).ok()?;
        let find = |name: &str| mad.animation_index(name.trim()).map_or(-1, |i| i as i32);
        let mut s = MadStatic {
            windows: mad.animations.iter().map(|a| Window { start: a.start_time, end: a.end_time }).collect(),
            ..MadStatic::default()
        };
        if let Some(n) = count.filter(|&n| n > 0) {
            let mut remap = vec![-1; n];
            for (k, v) in entries {
                if !k.eq_ignore_ascii_case("ANIM") {
                    continue;
                }
                let mut w = v.split_whitespace();
                if let (Some(Ok(i)), Some(name)) = (w.next().map(str::parse::<usize>), w.next()) {
                    if i < n {
                        remap[i] = find(name);
                    }
                }
            }
            s.remap = Some(remap);
        }
        if let Some(v) = get("MadStartInfo") {
            let f: Vec<&str> = v.split(',').map(str::trim).collect();
            if f.len() >= 4 {
                s.start_placed = (f[0] == "1").then(|| find(f[1]));
                s.start_built = (f[2] == "1").then(|| find(f[3]));
            }
        }
        if let Some(v) = get("MadGunAnims") {
            let f: Vec<&str> = v.split(',').map(str::trim).collect();
            let mut i = 0;
            let mut list = || {
                let n: usize = f.get(i).and_then(|x| x.parse().ok()).unwrap_or(0);
                i += 1;
                let l: Vec<i32> = f.iter().skip(i).take(n).map(|x| find(x)).collect();
                i += n;
                l
            };
            s.gun_open = list();
            s.gun_close = list();
            s.gun_open_damaged = list();
            s.gun_close_damaged = list();
        }
        s.damaged_threshold = get("madGunOpenDamagedHealthThreshold").and_then(|v| v.trim().parse().ok()).unwrap_or(0.0);
        Some(s)
    }

    /// The file animation a number given to `madAnimationStart` plays.
    fn file_animation(&self, index: i32) -> Option<Window> {
        let f = match &self.remap {
            Some(r) => *r.get(usize::try_from(index).ok()?)?,
            None => index,
        };
        self.windows.get(usize::try_from(f).ok()?).copied()
    }
}

/// A ship's animation state (`madAnimation`, ship +0x4bc, and the link layer's +0x300..+0x31c).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MadState {
    /// +0xc: the animation, numbered as `madAnimationStart` was given it (-1 none).
    pub current: i32,
    /// +8: [`LOOP`], [`STOP_AT_END`].
    pub flags: u32,
    /// +0x20: on the file's timeline.
    pub time: f32,
    /// +0x14.
    pub paused: bool,
    /// Counts starts, for the view.
    pub started: u32,
    /// +0x304.
    pub gun_state: i32,
    /// +0x798: a Somtaaw carrier's doors are shut.
    pub door_closed: bool,
    /// +0x79c: when they shut again (below 0: not set).
    pub door_time: f32,
    /// +0x300.
    bits: u32,
    /// +0x314: the animation the next update starts.
    pending: i32,
    /// +0x318, +0x31c.
    after: i32,
    next: i32,
}

impl Default for MadState {
    fn default() -> Self {
        Self { current: -1, flags: 0, time: 0.0, paused: false, started: 0, gun_state: 0, door_closed: true, door_time: -1.0, bits: 0, pending: -1, after: 0, next: 0 }
    }
}

fn statics<'a>(u: &'a Universe, id: ShipId) -> Option<&'a MadStatic> {
    let s = u.ships.get(id)?;
    u.statics.ship(s.ship_type)?.mad.as_ref()
}

/// The ship's state, made when first wanted (a ship of a saved game older than this has none).
fn state_mut(u: &mut Universe, id: ShipId) -> Option<&mut MadState> {
    statics(u, id)?;
    Some(u.ships.get_mut(id)?.base.mad.get_or_insert_with(MadState::default))
}

/// Whether the type has animations (`ship->madAnimation`).
pub fn has_mad(stat: &ShipStaticInfo) -> bool {
    stat.mad.is_some()
}

/// The animation running (`+0xc`), -1 for none.
pub fn current(u: &Universe, id: ShipId) -> i32 {
    u.ships.get(id).and_then(|s| s.base.mad).map_or(-1, |m| m.current)
}

/// It has run its course and holds the last frame, or was paused (`+0x14`).
pub fn paused(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).and_then(|s| s.base.mad).is_some_and(|m| m.paused)
}

/// `madAnimationStart` (0x4b20c0).
pub fn start(u: &mut Universe, id: ShipId, index: i32, flags: u32) {
    let Some(w) = statics(u, id).and_then(|m| m.file_animation(index)) else { return };
    let Some(m) = state_mut(u, id) else { return };
    m.current = index;
    m.flags = flags;
    m.time = w.start;
    m.paused = false;
    m.started = m.started.wrapping_add(1);
}

/// `madAnimationPause`.
pub fn pause(u: &mut Universe, id: ShipId, on: bool) {
    if let Some(m) = state_mut(u, id) {
        m.paused = on;
    }
}

/// `madAnimationStop`.
pub fn stop(u: &mut Universe, id: ShipId) {
    if let Some(m) = state_mut(u, id) {
        m.current = -1;
        m.paused = false;
    }
}

/// `madAnimationFinished`: nothing is running.
pub fn finished(u: &Universe, id: ShipId) -> bool {
    let Some(m) = u.ships.get(id).and_then(|s| s.base.mad) else { return true };
    let Some(w) = statics(u, id).and_then(|s| s.file_animation(m.current)) else { return true };
    !(m.current != -1 && m.time < w.end && !m.paused)
}

/// `madAnimSoundTableInit` (0x4b19f0): the sound of a ship type's animation.
fn sound_of(t: usize, anim: i32) -> Option<u32> {
    Some(match (t, anim) {
        (1 | 34, 0) => 0x81a,
        (1 | 34, 1) => 0x81b,
        (3, 0) => 0x822,
        (3, 1) => 0x823,
        (6, 0) => 0x81c,
        (14 | 49, 0) => 0x81e,
        (14 | 49, 1) => 0x81f,
        (1 | 34, 2) | (14 | 49, 3) => 0x405,
        (1 | 34, 3) | (14 | 49, 2) => 0x406,
        (2 | 8 | 12 | 17 | 42 | 52, 0) => 0x405,
        (2 | 8 | 12 | 17 | 42 | 52, 1) => 0x406,
        _ => return None,
    })
}

/// The sound `madAnimationUpdate` plays on an animation's first frame.
fn animation_sound(u: &mut Universe, id: ShipId, anim: i32) {
    let Some(t) = u.ships.get(id).map(|s| s.ship_type.index()) else { return };
    if let Some(event) = sound_of(t, anim) {
        ship_sound(u, id, event);
    }
}

/// `soundEventPlay(ship, event, 0)`.
pub fn ship_sound(u: &mut Universe, id: ShipId, event: u32) {
    u.special_events.push(crate::universe::SpecialEvent::ShipSound { ship: id, event });
}

/// `madAnimationUpdate` (0x4b2210): true when a one-shot animation ended this step.
fn advance(u: &mut Universe, id: ShipId, dt: f32) -> bool {
    let Some(m) = u.ships.get(id).and_then(|s| s.base.mad) else { return false };
    let Some(w) = statics(u, id).and_then(|s| s.file_animation(m.current)) else { return false };
    if m.paused {
        return false;
    }
    if m.time == w.start {
        animation_sound(u, id, m.current);
    }
    let Some(m) = state_mut(u, id) else { return false };
    if dt + m.time <= w.end {
        m.time += dt;
        return false;
    }
    if m.flags & LOOP != 0 {
        let len = w.end - w.start;
        if len > dt {
            loop {
                m.time = (dt + m.time - w.end) + w.start;
                if m.time + dt <= w.end {
                    break;
                }
            }
        } else {
            m.time = w.start;
        }
        return false;
    }
    if m.flags & STOP_AT_END == 0 {
        m.time = w.end;
        m.paused = true;
    } else {
        m.current = -1;
        m.paused = false;
    }
    true
}

/// A gun animation chosen as `madOpenGunsShip` does: a random one of the damaged list when the
/// ship's health is below the threshold and the list has any (one draw of the game's stream).
fn pick(u: &mut Universe, id: ShipId, whole: &[i32], damaged: &[i32], threshold: f32) -> i32 {
    let s = u.ships.ship(id);
    let max = u.statics.ship(s.ship_type).map_or(1.0, |st| st.health.max_health);
    let list = if threshold * max <= s.health || damaged.is_empty() { whole } else { damaged };
    let r = u.randy.random(RAN_GAME);
    list[(r % list.len() as u32) as usize]
}

/// The ship's guns need opening first (`GUN` animations listed).
pub fn guns_must_open(u: &Universe, id: ShipId) -> bool {
    let needs = statics(u, id).is_some_and(|m| !m.gun_open.is_empty());
    needs && u.ships.get(id).and_then(|s| s.base.mad).map_or(0, |m| m.gun_state) != gun::OPEN
}

/// `madOpenGunsShip` (0x4a0bc0).
pub fn open_guns(u: &mut Universe, id: ShipId) {
    let Some(st) = statics(u, id).cloned() else { return };
    if st.gun_open.is_empty() {
        return;
    }
    let Some(m) = state_mut(u, id) else { return };
    match m.gun_state {
        gun::OPEN | gun::OPENING => return,
        gun::CLOSING => {
            m.next = AFTER_OPEN;
            return;
        }
        _ => {}
    }
    m.bits |= BIT_BUSY;
    let a = pick(u, id, &st.gun_open, &st.gun_open_damaged, st.damaged_threshold);
    let Some(m) = state_mut(u, id) else { return };
    m.pending = a;
    m.after = AFTER_OPEN;
    m.gun_state = gun::OPENING;
    m.bits |= BIT_START;
}

/// `madLinkInCloseGunsShip` (0x4a0d10).
pub fn close_guns(u: &mut Universe, id: ShipId) {
    let Some(st) = statics(u, id).cloned() else { return };
    if st.gun_close.is_empty() {
        return;
    }
    let Some(m) = state_mut(u, id) else { return };
    match m.gun_state {
        gun::CLOSED | gun::CLOSING => return,
        gun::OPENING => {
            m.next = AFTER_CLOSE;
            return;
        }
        _ => {}
    }
    m.bits |= BIT_BUSY;
    let a = pick(u, id, &st.gun_close, &st.gun_close_damaged, st.damaged_threshold);
    let Some(m) = state_mut(u, id) else { return };
    m.pending = a;
    m.after = AFTER_CLOSE;
    m.gun_state = gun::CLOSING;
    m.bits |= BIT_START;
}

/// The ion array frigates close their guns to dock (`clDock`, `InitializeDockVars`,
/// `dockChangeSingleShipToDock`, `processDockToDo`).
pub fn close_guns_to_dock(u: &mut Universe, id: ShipId) {
    use crate::statics::ShipType::{bIonArrayFrigate, p1IonArrayFrigate};
    let Some(s) = u.ships.get(id) else { return };
    if matches!(s.ship_type, bIonArrayFrigate | p1IonArrayFrigate)
        && s.base.mad.is_some_and(|m| matches!(m.gun_state, gun::OPENING | gun::OPEN))
    {
        close_guns(u, id);
    }
}

/// `madLinkInSetUpInitialBuiltMadState`, `madLinkInSetUpInitialPlacedMadState` (from
/// `univCreateShip`): the start animation, paused at its first frame.
pub fn set_up_initial(u: &mut Universe, id: ShipId, built: bool) {
    let Some(st) = statics(u, id) else { return };
    if st.remap.is_some() {
        return;
    }
    let first = if built { st.start_built } else { st.start_placed };
    let Some(a) = first else { return };
    start(u, id, a, 0);
    pause(u, id, true);
}

/// `madLinkInUpdateMeshAnimations` (0x4a0800).
fn update_link_layer(u: &mut Universe, id: ShipId, dt: f32) {
    let Some(m) = u.ships.get(id).and_then(|s| s.base.mad) else { return };
    if m.bits & BIT_START != 0 {
        start(u, id, m.pending, 0);
        if let Some(m) = state_mut(u, id) {
            m.bits &= !BIT_START;
        }
        return;
    }
    if !advance(u, id, dt) {
        return;
    }
    let Some(st) = statics(u, id).cloned() else { return };
    let after = m.after;
    if let Some(m) = state_mut(u, id) {
        m.after = AFTER_NOTHING;
        match after {
            AFTER_OPEN => m.gun_state = gun::OPEN,
            AFTER_CLOSE => m.gun_state = gun::CLOSED,
            _ => {}
        }
    }
    if after == AFTER_CLOSE {
        // Closed guns hold the first frame of the opening.
        let was_paused = paused(u, id);
        if let Some(&a) = st.gun_open.first() {
            start(u, id, a, 0);
            pause(u, id, !was_paused);
        }
    }
    let Some(m) = state_mut(u, id) else { return };
    m.bits &= !BIT_BUSY;
    let next = std::mem::take(&mut m.next);
    match next {
        AFTER_OPEN => open_guns(u, id),
        AFTER_CLOSE => close_guns(u, id),
        _ => {}
    }
}

/// The animation part of `univUpdateAllPosVelShips` (0x53f...): one step of the clock.
pub fn update(u: &mut Universe, id: ShipId, dt: f32) {
    let Some(st) = statics(u, id) else { return };
    let link_layer = st.remap.is_none();
    let Some(m) = u.ships.get(id).and_then(|s| s.base.mad) else { return };
    if link_layer && m.bits & BIT_BUSY != 0 {
        if m.current != -1 || m.bits & BIT_START != 0 {
            update_link_layer(u, id, dt);
        }
        return;
    }
    if m.current != -1 {
        advance(u, id, dt);
    }
}

/// `CarrierHouseKeep` (0x5a9d00) for the Somtaaw carrier, the only one with doors: open while a
/// bay point is in use, shut a while after.
pub fn carrier_doors(u: &mut Universe, id: ShipId) {
    use crate::statics::DockPointType as P;
    let Some(s) = u.ships.get(id) else { return };
    let Some(dock) = u.statics.ship(s.ship_type).and_then(|st| st.dock.as_ref()) else { return };
    let used = s.base.occupancy.as_ref().is_some_and(|o| {
        dock.points.iter().zip(o.reserved.iter()).any(|(p, &r)| r && matches!(p.dock_type, Some(P::MedIn | P::MedOut)))
    });
    let m = u.ships.ship(id).base.mad.unwrap_or_default();
    if used {
        if m.door_closed && finished(u, id) {
            start(u, id, 0, 0);
            if let Some(m) = state_mut(u, id) {
                m.door_closed = false;
            }
        }
        if let Some(m) = state_mut(u, id) {
            m.door_time = -1.0;
        }
    } else if !m.door_closed && finished(u, id) {
        let now = u.total_time_elapsed;
        if m.door_time < 0.0 {
            let r = u.randy.random(RAN_GAME);
            let at = r as f32 * 1.164_153_2e-9 + 5.0 + now;
            if let Some(m) = state_mut(u, id) {
                m.door_time = at;
            }
        } else if m.door_time <= now {
            start(u, id, 1, 0);
            if let Some(m) = state_mut(u, id) {
                m.door_closed = true;
            }
        }
    }
}

/// `CarrierDockDoorCheck` (0x5a9c20): a ship leaving by a bay waits for the doors.
pub fn carrier_door_open(u: &mut Universe, id: ShipId, kind: Option<crate::statics::DockPointType>) -> bool {
    use crate::statics::DockPointType as P;
    if !matches!(kind, Some(P::MedIn | P::MedOut)) {
        return true;
    }
    let m = u.ships.ship(id).base.mad.unwrap_or_default();
    if !m.door_closed && finished(u, id) {
        return true;
    }
    if m.door_closed && finished(u, id) {
        start(u, id, 0, 0);
        if let Some(m) = state_mut(u, id) {
            m.door_closed = false;
            m.door_time = -1.0;
        }
    }
    false
}

/// Starts the worker's jack when no animation holds it (`WorkerRepairUpdate`, the harvesting
/// states of `WorkerHouseKeep`): the jack is out once animation 0 has stopped.
pub fn worker_jack_out(u: &mut Universe, id: ShipId) -> bool {
    let cur = current(u, id);
    if cur == 0 {
        return paused(u, id);
    }
    if cur == -1 || paused(u, id) {
        start(u, id, 0, 0);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ship_file_without_animations_has_none() {
        assert!(MadStatic::load(&[], None).is_none());
        let e = vec![("NUMBER_OF_ANIMS".to_string(), "0".to_string())];
        assert!(MadStatic::load(&e, None).is_none());
    }

    #[test]
    fn numbers_go_through_the_ship_files_table() {
        let s = MadStatic {
            windows: vec![Window { start: 0.0, end: 1.0 }, Window { start: 1.0, end: 3.0 }],
            remap: Some(vec![1, 0, -1]),
            ..MadStatic::default()
        };
        assert_eq!(s.file_animation(0).map(|w| w.end), Some(3.0));
        assert_eq!(s.file_animation(1).map(|w| w.end), Some(1.0));
        assert_eq!(s.file_animation(2), None);
        assert_eq!(s.file_animation(9), None);
    }
}
