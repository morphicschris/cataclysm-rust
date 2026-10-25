//! The effect runtime: effect instances, the ETG interpreter and the functions scripts call.
//!
//! [`FxWorld`] owns everything the original kept in globals: the effect list, the particle
//! systems, the particle attribute block, the spawn offsets (`etgEffOffset`), the execution
//! and call stacks, the random streams and the universe clock.
//!
//! Timing follows the original: [`FxWorld::update`] runs once per universe update
//! (`univUpdateAllPosVelEffects`, normally with `UNIVERSE_UPDATE_PERIOD` = 1/16 s) and executes
//! the effect scripts; particles advance in [`FxWorld::draw`] (`etgEffectDraw`) by the universe
//! time elapsed since each system was last drawn. Script randomness uses stream 26, particle
//! creation stream 25, and the random offset array and bolt jitter stream 0.
//!
//! Effects can be attached to game objects ("owners") that the caller registers and updates
//! with [`FxWorld::set_owner`]; requests that reach outside the effect system (sounds, battle
//! chatter, deleting or hiding the parent ship) are queued as [`FxEvent`]s.

use std::collections::HashMap;

use hwc_math::fastmath::fmath_sqrt_double;
use hwc_math::fpu::{ftol_i32, ld, lt_or_unordered, st, Ext};
use hwc_math::random::{Randy, RAN_ETG_CAT};
use hwc_math::trig::{fcos, fsin};
use hwc_math::{Matrix, Vector};

use crate::bytecode::{self, evt, op, BLOCK_EACH_FRAME, BLOCK_STARTUP, BLOCK_TIME_INDEX};
use crate::cfmt::{col_real_to_ubyte, col_rgba, col_ubyte_to_real};
use crate::compiler::{Library, ESO_SORT_FORWARD, ESO_WORLD_RENDER};
use crate::functions::Fun;
use crate::particle::{self, sysflag, CreateEnv, DataRef, DataSource, LensFlare, PartSystem, PartType, Pat};

/// Space-object flags used by effects (`SOF_*`).
pub mod sof {
    pub const ROTATABLE: u32 = 0x1;
    pub const ATTACH_POSITION: u32 = 0x8;
    pub const ATTACH_COORDSYS: u32 = 0x10;
    pub const ATTACH_VELOCITY: u32 = 0x20;
    pub const ATTACH_NLIPS: u32 = 0x40;
    pub const HIDE: u32 = 0x8000;
    pub const ALWAYS_SORT_FRONT: u32 = 0x400_0000;
    /// Cataclysm: render particle systems in the owner's coordinate system.
    pub const OWNER_COORDSYS: u32 = 0x2000_0000;
}

/// Attachment flags accepted by [`FxWorld::create_effect`] (`EAF_*`).
pub mod eaf {
    pub const POSITION: u32 = 0x8;
    pub const COORDSYS: u32 = 0x10;
    pub const VELOCITY: u32 = 0x20;
    pub const NLIPS: u32 = 0x40;
    pub const FULL: u32 = POSITION | COORDSYS | VELOCITY | NLIPS;
    pub const OWNER_COORDSYS: u32 = 0x100_0000;
}

const EF_RENDERED: u32 = 0x10;
/// `partSetDeltaRDist` substitutes this for a zero angle spread (the source's `6.28f`).
#[allow(clippy::approx_constant)]
const DELTA_R_DIST_DEFAULT: f32 = 6.28;
/// The double constants `etgFDegSin` multiplies by (0x720800, 0x7207f8).
#[allow(clippy::approx_constant)]
const DEG_SIN_TWO_PI: f64 = 6.283_185_307;
const DEG_SIN_ONE_OVER_360: f64 = 0.002_777_777_777_777_778;
const EM_TO_BE_DELETED: u32 = 0xf;
const EAF_PLAYING_SPEECH: u32 = 0x2000_0000;
/// `REALlyBig` as the original stores it for an owner-less effect's camera distance.
const REALLY_BIG_BITS: u32 = 0x6258_d727;

/// Identifies an effect instance.
pub type EffectId = u32;
/// Identifies a caller-side object effects can attach to.
pub type OwnerId = u32;

/// The state of an object effects can be attached to.
#[derive(Clone, Debug, PartialEq)]
pub struct Owner {
    pub position: Vector,
    pub velocity: Vector,
    pub coordsys: Matrix,
    /// The object's "N-Lips" scale (`magnitudeSquared`).
    pub nlips: f32,
    pub color_scheme: u8,
    /// A ship (vs. a derelict or another effect).
    pub is_ship: bool,
    pub is_derelict: bool,
    /// Cataclysm skips integrating an attached effect's position while its ship owner is in
    /// a particular state (`owner+0x2b0 == 3`); set this to reproduce that.
    pub hold_effects: bool,
    /// The ship's hyperspace colour (`staticinfo->hyperspaceColor`).
    pub hyperspace_color: u32,
    pub camera_distance_sq: f32,
}

impl Default for Owner {
    fn default() -> Self {
        Self {
            position: Vector::ZERO,
            velocity: Vector::ZERO,
            coordsys: Matrix::IDENTITY,
            nlips: 1.0,
            color_scheme: 0,
            is_ship: true,
            is_derelict: false,
            hold_effects: false,
            hyperspace_color: 0,
            camera_distance_sq: f32::from_bits(REALLY_BIG_BITS),
        }
    }
}

/// Requests from effect scripts to the rest of the game.
#[derive(Clone, Debug, PartialEq)]
pub enum FxEvent {
    /// `playSound(event)` (`soundEffect`: the effect itself is the sound's object, so it plays
    /// at the effect's position).
    Sound { effect: EffectId, owner: Option<OwnerId>, event: u32, position: Vector },
    /// `playChatter(event)` (only with an owner).
    Chatter { effect: EffectId, owner: OwnerId, event: u32 },
    /// `deathCries()`.
    DeathCries { owner: OwnerId },
    /// `deleteParentShip()`; `speech_playing` asks for `soundEventShipDied` first.
    DeleteParent { owner: OwnerId, derelict: bool, speech_playing: bool },
    /// `hideParentShip()`.
    HideParent { owner: OwnerId },
    /// `omitParentShipFromRenderList()`.
    OmitParentFromRenderList { owner: OwnerId },
    /// `damageDone()` with an owner (`dmgStopSingleEffect`).
    DamageDone { effect: EffectId, owner: OwnerId },
}

/// An effect instance (`Effect`).
#[derive(Clone, Debug)]
pub struct Effect {
    pub id: EffectId,
    /// Index of its static in the library.
    pub stat: usize,
    pub flags: u32,
    pub owner: Option<OwnerId>,
    pub position: Vector,
    pub velocity: Vector,
    pub coordsys: Matrix,
    /// "N-Lips" scale (`magnitudeSquared`).
    pub nlips: f32,
    pub time_elapsed: f32,
    pub drag: f32,
    pub spin: f32,
    pub delta_spin: f32,
    pub effect_flags: u32,
    pub current_lod: u8,
    pub camera_distance_sq: f32,
    pub n_spawned_attached: i32,
    pub variables: Vec<u32>,
    pub rates: Vec<u32>,
    pub effector_ids: Vec<u8>,
    pub n_particle_blocks: u32,
    /// Particle system handles, in creation order (`particleBlock[0..iParticleBlock]`).
    pub blocks: Vec<u32>,
}

/// Spawn parameters set by `setEff*` (`etgEffOffset`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EffOffset {
    pub offset_lof: f32,
    pub offset_r: f32,
    pub offset_theta: f32,
    pub change_lof_theta: f32,
    pub change_lof_mu: f32,
    pub offset_xyz: Vector,
    pub offset_vel_lof: f32,
    pub offset_spin: f32,
    pub offset_time: f32,
    pub drag: f32,
    pub attach_parent: u32,
    pub spin: u32,
    pub delta_spin: u32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Exec {
    stat: usize,
    block: usize,
    off: [u32; 3],
    delete: bool,
}

/// How a function argument was produced, for pointer-typed parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArgKind {
    Value,
    ConstPtr,
    VarPtr,
}

/// The effect world.
#[derive(Clone)]
pub struct FxWorld {
    pub lib: Library,
    pub pat: Pat,
    pub eff_offset: EffOffset,
    pub randy: Randy,
    /// `universe.totaltimeelapsed`.
    pub universe_time: f32,
    /// Use effects' software versions (`RGLtype == SWtype`).
    pub software_renderer: bool,
    /// `etgHistoryScalar` (256 = no reduction).
    pub history_scalar: i32,
    pub effects: Vec<Effect>,
    pub systems: Vec<Option<PartSystem>>,
    pub lens_flares: Vec<LensFlare>,
    pub random_offsets: Vec<Vector>,
    pub owners: HashMap<OwnerId, Owner>,
    pub events: Vec<FxEvent>,
    pub warnings: Vec<String>,
    /// Treat every effect as drawn when deciding how long a deleted effect lingers (the
    /// original deletes a drawn effect at once and an undrawn one after four updates). Off by
    /// default; a simulation that must not depend on what was drawn turns it on, which gives
    /// every effect the on-screen behaviour.
    pub assume_drawn: bool,
    /// When set, the render-time random draws (electric-bolt jitter) use this generator
    /// instead of [`FxWorld::randy`], so drawing does not move the streams the effect scripts
    /// use. Off (`None`) by default, as in the original, where both share stream 0.
    pub draw_randy: Option<Randy>,
    next_id: EffectId,
    exec: Exec,
    call_stack: Vec<(usize, u32)>,
    time_elapsed: f32,
    total_time: f32,
    effector_duration: f32,
    create_callback: Option<(u32, EffectId)>,
    current_effect: usize,
}

struct WorldData<'a> {
    lib: &'a Library,
    effects: &'a [Effect],
    random_offsets: &'a [Vector],
}

impl DataSource for WorldData<'_> {
    fn dword(&self, r: DataRef, byte_offset: u32) -> u32 {
        match r {
            DataRef::Const { stat, offset } => self
                .lib
                .statics
                .get(stat as usize)
                .map(|s| s.const_u32(offset.wrapping_add(byte_offset)))
                .unwrap_or(0),
            DataRef::Var { effect, offset } => self
                .effects
                .iter()
                .find(|e| e.id == effect)
                .and_then(|e| e.variables.get((offset.wrapping_add(byte_offset) / 4) as usize).copied())
                .unwrap_or(0),
            DataRef::RandomOffsets => {
                let i = (byte_offset / 12) as usize;
                let k = (byte_offset % 12) / 4;
                self.random_offsets
                    .get(i)
                    .map(|v| [v.x, v.y, v.z][k as usize].to_bits())
                    .unwrap_or(0)
            }
        }
    }
}

fn f(v: u32) -> f32 {
    f32::from_bits(v)
}

impl FxWorld {
    /// A world over a compiled library. Random streams start from their defaults.
    pub fn new(lib: Library) -> Self {
        Self {
            lib,
            pat: Pat::zeroed(),
            eff_offset: EffOffset::default(),
            randy: Randy::default(),
            universe_time: 0.0,
            software_renderer: false,
            history_scalar: 256,
            effects: Vec::new(),
            systems: Vec::new(),
            lens_flares: Vec::new(),
            random_offsets: vec![Vector::ZERO; particle::RANDOM_OFFSET_COUNT],
            owners: HashMap::new(),
            events: Vec::new(),
            warnings: Vec::new(),
            assume_drawn: false,
            draw_randy: None,
            next_id: 1,
            exec: Exec::default(),
            call_stack: Vec::new(),
            time_elapsed: 0.0,
            total_time: 0.0,
            effector_duration: 0.0,
            create_callback: None,
            current_effect: 0,
        }
    }

    /// Register or update an owner object.
    pub fn set_owner(&mut self, id: OwnerId, owner: Owner) {
        self.owners.insert(id, owner);
    }

    /// `etgShipDied`: detach every effect from `id` and forget the owner.
    pub fn owner_died(&mut self, id: OwnerId) {
        for e in &mut self.effects {
            if e.owner == Some(id) {
                e.owner = None;
            }
        }
        self.owners.remove(&id);
    }

    /// `etgDeleteEffectsOwnedBy`.
    pub fn delete_effects_owned_by(&mut self, id: OwnerId) -> usize {
        let doomed: Vec<EffectId> = self.effects.iter().filter(|e| e.owner == Some(id)).map(|e| e.id).collect();
        for d in &doomed {
            self.delete_effect(*d);
        }
        doomed.len()
    }

    /// The effect with `id`.
    pub fn effect(&self, id: EffectId) -> Option<&Effect> {
        self.effects.iter().find(|e| e.id == id)
    }

    /// The particle system behind a handle.
    pub fn system(&self, handle: u32) -> Option<&PartSystem> {
        self.systems.get((handle as usize).wrapping_sub(1))?.as_ref()
    }

    /// Number of live particles over all systems.
    pub fn particle_count(&self) -> usize {
        self.systems.iter().flatten().map(|s| s.particles.len()).sum()
    }

    fn new_effect(&mut self, stat: usize) -> Effect {
        let id = self.next_id;
        self.next_id += 1;
        Effect {
            id,
            stat,
            flags: sof::ROTATABLE,
            owner: None,
            position: Vector::ZERO,
            velocity: Vector::ZERO,
            coordsys: Matrix::IDENTITY,
            nlips: 1.0,
            time_elapsed: 0.0,
            drag: 1.0,
            spin: 0.0,
            delta_spin: 0.0,
            effect_flags: 0,
            current_lod: 0,
            camera_distance_sq: f32::from_bits(REALLY_BIG_BITS),
            n_spawned_attached: 0,
            variables: Vec::new(),
            rates: Vec::new(),
            effector_ids: Vec::new(),
            n_particle_blocks: 0,
            blocks: Vec::new(),
        }
    }

    fn pick_version(&self, stat: usize) -> usize {
        if self.software_renderer {
            if let Some(s) = self.lib.statics[stat].software_version {
                return s;
            }
        }
        stat
    }

    /// `etgEffectCodeStart`.
    fn code_start(&mut self, e: &mut Effect, params: &[u32]) {
        let s = &self.lib.statics[e.stat];
        let words = (s.variable_size / 4) as usize;
        e.variables = vec![0; words.max(params.len())];
        e.rates = vec![0; (s.rate_size / 4) as usize];
        e.effector_ids = vec![0xff; words];
        e.n_particle_blocks = s.n_particle_blocks;
        e.blocks.clear();
        for &(idx, v) in &s.var_init {
            if let Some(slot) = e.variables.get_mut(idx as usize) {
                *slot = v;
            }
        }
        for (i, &p) in params.iter().enumerate() {
            e.variables[i] = p;
        }
        e.effect_flags = 0;
        e.spin = 0.0;
        e.delta_spin = 0.0;
        let now = self.universe_time;
        let s = &mut self.lib.statics[e.stat];
        if !s.history.is_empty() {
            if s.i_history == s.n_history {
                s.i_history = 0;
            }
            let i = s.i_history as usize;
            s.history[i] = now;
            s.i_history += 1;
        }
    }

    /// `etgEffectCreate`: start effect `stat` (an index into the library).
    #[allow(clippy::too_many_arguments)]
    pub fn create_effect(
        &mut self,
        stat: usize,
        owner: Option<OwnerId>,
        position: Option<Vector>,
        velocity: Option<Vector>,
        coordsys: Option<Matrix>,
        nlips: f32,
        flags: u32,
        params: &[u32],
    ) -> EffectId {
        let stat = self.pick_version(stat);
        let mut e = self.new_effect(stat);
        let mut fl = sof::ROTATABLE;
        if flags & eaf::POSITION != 0 {
            fl |= sof::ATTACH_POSITION;
        }
        if flags & eaf::VELOCITY != 0 {
            fl |= sof::ATTACH_VELOCITY;
        }
        if flags & eaf::COORDSYS != 0 {
            fl |= sof::ATTACH_COORDSYS;
        }
        if flags & eaf::NLIPS != 0 {
            fl |= sof::ATTACH_NLIPS;
        }
        if flags & eaf::OWNER_COORDSYS != 0 {
            fl |= sof::OWNER_COORDSYS;
        }
        e.flags = fl;
        let o = owner.and_then(|id| self.owners.get(&id).cloned());
        e.camera_distance_sq = o.as_ref().map(|o| o.camera_distance_sq).unwrap_or(f32::from_bits(REALLY_BIG_BITS));
        e.position = position.or(o.as_ref().map(|o| o.position)).unwrap_or(Vector::ZERO);
        e.velocity = velocity.or(o.as_ref().map(|o| o.velocity)).unwrap_or(Vector::ZERO);
        e.coordsys = coordsys.or(o.as_ref().map(|o| o.coordsys)).unwrap_or(Matrix::IDENTITY);
        e.owner = owner;
        e.nlips = if nlips == 0.0 { 1.0 } else { nlips };
        self.code_start(&mut e, params);
        let ops = self.lib.statics[stat].special_ops;
        if ops & ESO_SORT_FORWARD != 0 {
            e.flags |= sof::ALWAYS_SORT_FRONT;
        }
        if ops & ESO_WORLD_RENDER != 0 {
            e.flags |= sof::HIDE;
        }
        let id = e.id;
        self.effects.push(e);
        id
    }

    /// Start an effect by file name with no owner (convenience for tools and tests).
    pub fn spawn_named(&mut self, name: &str, position: Vector, coordsys: Matrix, params: &[u32]) -> Option<EffectId> {
        let stat = self.lib.get(name)?;
        Some(self.create_effect(stat, None, Some(position), Some(Vector::ZERO), Some(coordsys), 1.0, 0, params))
    }

    /// `etgEffectDelete` + `univDeleteEffect`: remove an effect and free its particle systems.
    pub fn delete_effect(&mut self, id: EffectId) {
        if let Some(i) = self.effects.iter().position(|e| e.id == id) {
            let e = self.effects.remove(i);
            self.free_blocks(&e);
        }
    }

    fn free_blocks(&mut self, e: &Effect) {
        for &h in &e.blocks {
            if let Some(slot) = self.systems.get_mut((h as usize).wrapping_sub(1)) {
                if let Some(sys) = slot.take() {
                    for p in &sys.particles {
                        if let Some(fi) = p.lens_flare {
                            if let Some(fl) = self.lens_flares.get_mut(fi) {
                                fl.enabled = 0;
                                fl.active = 0;
                            }
                        }
                    }
                }
            }
        }
    }

    /// `etgFrequencyExceeded` (consumes a stream-26 number unless the limit is reached).
    pub fn frequency_exceeded(&mut self, stat: usize) -> bool {
        let now = self.universe_time;
        let s = &self.lib.statics[stat];
        if s.history.is_empty() {
            return false;
        }
        let max = s.n_history;
        let mut idx = s.i_history;
        let mut count = 0i32;
        if max > 0 {
            loop {
                idx -= 1;
                if idx < 0 {
                    idx = max - 1;
                }
                if f64::from(now) - f64::from(s.history[idx as usize]) > 1.0 {
                    break;
                }
                count += 1;
                if count >= max {
                    break;
                }
            }
        }
        let scaled = (max * self.history_scalar) / 256;
        if scaled <= count {
            return true;
        }
        let r = self.randy.random(RAN_ETG_CAT);
        let span = (scaled - count + 1) as u32;
        (r % span).wrapping_add(count as u32) < count as u32
    }

    // ------------------------------------------------------------------ update

    /// `univUpdateAllPosVelEffects` for one universe update of `dt` seconds (advances the
    /// universe clock first, as `univUpdate` does).
    pub fn update(&mut self, dt: f32) {
        self.universe_time = st(ld(self.universe_time) + ld(dt));
        self.run_update(dt);
    }

    /// The effect pass of `univUpdate` without advancing the clock, for a caller that keeps
    /// [`FxWorld::universe_time`] equal to its own universe clock.
    pub fn run_update(&mut self, dt: f32) {
        let mut i = 0;
        while i < self.effects.len() {
            if self.effect_update(i, dt) {
                let e = self.effects.remove(i);
                self.free_blocks(&e);
                continue;
            }
            i += 1;
        }
    }

    fn code_len(&self, stat: usize, block: usize) -> u32 {
        self.lib.statics[stat].code[block].len() as u32
    }

    fn word(&self, stat: usize, block: usize, at: u32) -> u32 {
        bytecode::rd_or0(&self.lib.statics[stat].code[block], at)
    }

    /// `etgEffectUpdate`. Returns `true` when the effect should be deleted.
    fn effect_update(&mut self, ei: usize, dt: f32) -> bool {
        self.time_elapsed = dt;
        self.exec.delete = false;
        let total = st(ld(dt) + ld(self.effects[ei].time_elapsed));
        self.total_time = total;
        self.effects[ei].time_elapsed = total;
        if total < 0.0 {
            return false;
        }
        let stat = self.effects[ei].stat;
        self.exec.stat = stat;
        if ld(total) <= ld(dt) + 0.005 {
            self.execute(ei, BLOCK_STARTUP);
        }
        self.execute(ei, BLOCK_TIME_INDEX);
        if !self.exec.delete {
            self.execute(ei, BLOCK_EACH_FRAME);
        }
        // Physics.
        let dtx = ld(dt);
        let owner = self.effects[ei].owner.and_then(|id| self.owners.get(&id).cloned());
        let e = &mut self.effects[ei];
        let drag = ld(e.drag);
        let apply_drag = |v: &mut Vector| {
            v.x = st(drag * ld(v.x));
            v.y = st(drag * ld(v.y));
            v.z = st(drag * ld(v.z));
        };
        match &owner {
            None => apply_drag(&mut e.velocity),
            Some(o) => {
                if e.flags & sof::ATTACH_COORDSYS != 0 {
                    e.coordsys = o.coordsys;
                }
                if e.flags & sof::ATTACH_POSITION != 0 {
                    e.position = o.position;
                }
                if e.flags & sof::ATTACH_VELOCITY != 0 {
                    e.velocity = o.velocity;
                } else {
                    apply_drag(&mut e.velocity);
                }
                if e.flags & sof::ATTACH_NLIPS != 0 {
                    e.nlips = o.nlips;
                }
            }
        }
        let hold = matches!(&owner, Some(o) if o.is_ship && o.hold_effects);
        if !hold {
            e.position.x = st(dtx * ld(e.velocity.x) + ld(e.position.x));
            e.position.y = st(dtx * ld(e.velocity.y) + ld(e.position.y));
            e.position.z = st(dtx * ld(e.velocity.z) + ld(e.position.z));
        }
        let spin = ld(e.delta_spin) + ld(e.spin);
        e.spin = st(spin);
        if spin != 0.0 {
            let a = spin * dtx;
            let c = st(fcos(a));
            let s = st(fsin(a));
            // matMakeRotAboutZ(&rot, sin, cos): arguments swapped as in the original.
            let rot = Matrix::rot_about_z(s, c);
            e.coordsys = rot.mul_mat(&e.coordsys);
        }
        let fl = e.effect_flags;
        let del = u32::from(self.exec.delete);
        if fl & EF_RENDERED != 0 || self.assume_drawn {
            return (fl & EM_TO_BE_DELETED) | del != 0;
        }
        if (fl & EM_TO_BE_DELETED) != 0 || del != 0 {
            e.effect_flags = fl + 1;
            if ((fl + 1) & 0xf) > 3 {
                return true;
            }
        }
        false
    }

    /// `etgEffectCodeExecute`.
    fn execute(&mut self, ei: usize, block: usize) {
        self.exec.block = block;
        self.exec.off = [0; 3];
        self.current_effect = ei;
        self.run(ei);
    }

    fn run(&mut self, ei: usize) {
        let stat = self.exec.stat;
        let mut guard = 0u32;
        loop {
            let b = self.exec.block;
            if self.exec.off[b] >= self.code_len(stat, b) {
                break;
            }
            guard += 1;
            if guard > 1_000_000 {
                self.warnings.push(format!("{}: runaway effect code", self.lib.statics[stat].name));
                break;
            }
            let at = self.exec.off[b];
            let opcode = self.word(stat, b, at);
            let size = self.op(ei, stat, b, at, opcode);
            let nb = self.exec.block;
            self.exec.off[nb] = self.exec.off[nb].wrapping_add(size);
        }
    }

    fn var(&self, ei: usize, off: u32) -> u32 {
        self.effects[ei].variables.get((off / 4) as usize).copied().unwrap_or(0)
    }

    fn set_var(&mut self, ei: usize, off: u32, v: u32) {
        if let Some(slot) = self.effects[ei].variables.get_mut((off / 4) as usize) {
            *slot = v;
        }
    }

    fn rate(&self, ei: usize, off: u32) -> u32 {
        self.effects[ei].rates.get((off / 4) as usize).copied().unwrap_or(0)
    }

    fn set_rate(&mut self, ei: usize, off: u32, v: u32) {
        if let Some(slot) = self.effects[ei].rates.get_mut((off / 4) as usize) {
            *slot = v;
        }
    }

    /// Returns true (and records the id) the first time an effector runs.
    fn effector_start(&mut self, ei: usize, var_off: u32, id: u8) -> bool {
        let e = &mut self.effects[ei];
        let slot = (var_off >> 2) as usize;
        match e.effector_ids.get_mut(slot) {
            Some(x) if *x != id => {
                *x = id;
                true
            }
            Some(_) => false,
            None => true,
        }
    }

    fn op(&mut self, ei: usize, stat: usize, b: usize, at: u32, opcode: u32) -> u32 {
        let w = |s: &Self, k: u32| s.word(stat, b, at + 4 * k);
        let total = ld(self.total_time);
        let dt = ld(self.time_elapsed);
        match opcode {
            op::NOP => 4,
            op::BEFORE => {
                if f(w(self, 2)) <= self.total_time {
                    w(self, 1)
                } else {
                    12
                }
            }
            op::BEFORE_VAR => {
                if f(self.var(ei, w(self, 2))) <= self.total_time {
                    w(self, 1)
                } else {
                    12
                }
            }
            op::AFTER => {
                if self.total_time < f(w(self, 2)) {
                    w(self, 1)
                } else {
                    12
                }
            }
            op::AFTER_VAR => {
                if self.total_time < f(self.var(ei, w(self, 2))) {
                    w(self, 1)
                } else {
                    12
                }
            }
            op::AT | op::AT_VAR => {
                let t = if opcode == op::AT { f(w(self, 2)) } else { f(self.var(ei, w(self, 2))) };
                let diff = total - ld(t);
                let eps = f64::from(5.0e-7f32);
                if lt_or_unordered(diff, eps) {
                    return w(self, 1);
                }
                if lt_or_unordered(diff, dt + eps) {
                    12
                } else {
                    w(self, 1)
                }
            }
            op::BETWEEN => {
                let sv = w(self, 3);
                let start = if sv == 0xffff_ffff { f(w(self, 1)) } else { f(self.var(ei, sv)) };
                let evr = w(self, 4);
                let end = if evr == 0xffff_ffff { f(w(self, 2)) } else { f(self.var(ei, evr)) };
                #[allow(clippy::neg_cmp_op_on_partial_ord)]
                let outside = lt_or_unordered(total, ld(start)) || !(total < ld(end));
                if outside {
                    return w(self, 5);
                }
                self.effector_duration = st(ld(end) - ld(start));
                24
            }
            op::EVERY | op::EVERY_VAR => {
                let t = if opcode == op::EVERY { f(w(self, 2)) } else { f(self.var(ei, w(self, 2))) };
                let time = hwc_math::fpu::ftol_u32(ld(t) * 65536.0);
                if time == 0 {
                    return 12;
                }
                if opcode == op::EVERY {
                    let tot = hwc_math::fpu::ftol_u32(total * 65536.0);
                    let el = hwc_math::fpu::ftol_u32(dt * 65536.0);
                    if tot % time < el {
                        12
                    } else {
                        w(self, 1)
                    }
                } else {
                    let neg = hwc_math::fpu::ftol_u32(dt * -65536.0);
                    let tot = hwc_math::fpu::ftol_u32(total * 65536.0);
                    if tot % time <= 1u32.wrapping_sub(neg) {
                        12
                    } else {
                        w(self, 1)
                    }
                }
            }
            op::EFFECTOR_CI => {
                let (end, voff, roff, id) = (w(self, 1), w(self, 2), w(self, 3), (w(self, 4) & 0xff) as u8);
                if self.effector_start(ei, voff, id) {
                    let v = self.var(ei, voff);
                    self.set_rate(ei, roff, v);
                }
                let start = self.rate(ei, roff) as i32;
                let num = (end as i32).wrapping_sub(start).wrapping_mul(16);
                let den = ftol_i32(ld(self.effector_duration) * -16.0);
                let q = if den == 0 { 0 } else { num.wrapping_div(den) };
                self.set_var(ei, voff, start.wrapping_sub(q) as u32);
                20
            }
            op::EFFECTOR_CF | op::EFFECTOR_VF => {
                let (end, voff, roff, id) = (w(self, 1), w(self, 2), w(self, 3), (w(self, 4) & 0xff) as u8);
                if self.effector_start(ei, voff, id) {
                    let endv = if opcode == op::EFFECTOR_CF { f(end) } else { f(self.var(ei, end)) };
                    let cur = f(self.var(ei, voff));
                    let r = st((ld(endv) - ld(cur)) / ld(self.effector_duration));
                    self.set_rate(ei, roff, r.to_bits());
                }
                let r = f(self.rate(ei, roff));
                let cur = f(self.var(ei, voff));
                self.set_var(ei, voff, st(dt * ld(r) + ld(cur)).to_bits());
                20
            }
            op::EFFECTOR_CCO | op::EFFECTOR_VCO | op::EFFECTOR_CCA | op::EFFECTOR_VCA => {
                self.color_effector(ei, opcode, w(self, 1), w(self, 2), w(self, 3), (w(self, 4) & 0xff) as u8);
                20
            }
            op::DELETE => {
                self.exec.delete = true;
                0x10_0000
            }
            op::END => 0x10_0000,
            op::GOTO => {
                let nb = (w(self, 1) as usize).min(2);
                self.exec.block = nb;
                self.exec.off[nb] = w(self, 2);
                0
            }
            op::CALL => {
                self.call_stack.push((self.exec.block, at + 12));
                let nb = (w(self, 1) as usize).min(2);
                self.exec.block = nb;
                self.exec.off[nb] = w(self, 2);
                0
            }
            op::RETURN => {
                if let Some((blk, off)) = self.call_stack.pop() {
                    self.exec.block = blk;
                    self.exec.off[blk] = off;
                } else {
                    self.warnings.push("ETG call stack underflow".into());
                    self.exec.off[self.exec.block] = u32::MAX / 2;
                }
                0
            }
            42..=65 => {
                let (p0, p1, skip) = (w(self, 1), w(self, 2), w(self, 3));
                let a = self.var(ei, p0);
                let pass = match opcode {
                    42..=53 => {
                        let bv = if opcode >= 48 { self.var(ei, p1) } else { p1 };
                        match (opcode - 42) % 6 {
                            0 => a == bv,
                            1 => a != bv,
                            2 => a > bv,
                            3 => a >= bv,
                            4 => a < bv,
                            _ => a <= bv,
                        }
                    }
                    _ => {
                        let bv = if opcode >= 60 { f(self.var(ei, p1)) } else { f(p1) };
                        let av = f(a);
                        match (opcode - 54) % 6 {
                            0 => av == bv,
                            1 => av != bv,
                            2 => av > bv,
                            3 => av >= bv,
                            4 => av < bv,
                            _ => av <= bv,
                        }
                    }
                };
                if pass {
                    16
                } else {
                    skip
                }
            }
            op::VARIABLE_COPY => {
                let v = self.var(ei, w(self, 1));
                self.set_var(ei, w(self, 2), v);
                12
            }
            op::VARIABLE_ASSIGN => {
                let v = w(self, 1);
                self.set_var(ei, w(self, 2), v);
                12
            }
            op::FUNCTION => self.function_call(ei, stat, b, at),
            op::ALTERNATE => {
                let crit = w(self, 1) as usize;
                let skip = w(self, 3);
                let table = w(self, 4) as usize;
                self.call_stack.push((self.exec.block, at.wrapping_add(skip)));
                let d = self.lib.statics[stat].decisions.get(crit).copied().unwrap_or_default();
                let r = self.randy.random(RAN_ETG_CAT);
                let span = d.high.wrapping_sub(d.low);
                let alt = if span == 0 { 0 } else { r % span + d.low };
                self.lib.statics[stat].alternate_offsets.get(table + alt as usize).copied().unwrap_or(0x10_0000)
            }
            other => {
                self.warnings.push(format!("{}: opcode {other} has no handler", self.lib.statics[stat].name));
                0x10_0000
            }
        }
    }

    fn color_effector(&mut self, ei: usize, opcode: u32, end: u32, voff: u32, roff: u32, id: u8) {
        let alpha = opcode == op::EFFECTOR_CCA || opcode == op::EFFECTOR_VCA;
        let cur = self.var(ei, voff);
        let [r, g, b, a] = [cur & 0xff, (cur >> 8) & 0xff, (cur >> 16) & 0xff, cur >> 24];
        let red = col_ubyte_to_real(r);
        let green = st(col_ubyte_to_real(g));
        let blue = st(col_ubyte_to_real(b));
        let al = st(col_ubyte_to_real(a));
        if self.effector_start(ei, voff, id) {
            let target = if opcode == op::EFFECTOR_CCO || opcode == op::EFFECTOR_CCA { end } else { self.var(ei, end) };
            let d = ld(self.effector_duration);
            let t = |k: u32| col_ubyte_to_real((target >> (8 * k)) & 0xff);
            self.set_rate(ei, roff, st((t(0) - red) / d).to_bits());
            self.set_rate(ei, roff + 4, st((t(1) - ld(green)) / d).to_bits());
            self.set_rate(ei, roff + 8, st((t(2) - ld(blue)) / d).to_bits());
            if alpha {
                self.set_rate(ei, roff + 12, st((t(3) - ld(al)) / d).to_bits());
            }
        }
        let dt = ld(self.time_elapsed);
        let rr = |s: &Self, k: u32| ld(f(s.rate(ei, roff + 4 * k)));
        let clamp = |v: Ext| -> Ext {
            if v >= 0.0 {
                if v > 1.0 {
                    1.0
                } else {
                    v
                }
            } else {
                0.0
            }
        };
        let nr = st(dt * rr(self, 0) + red);
        // EffectorCCo keeps the original's bug: green and blue advance from the red value.
        let (ng, nb) = if opcode == op::EFFECTOR_CCO {
            (st(dt * rr(self, 1) + ld(nr)), st(dt * rr(self, 2) + ld(nr)))
        } else {
            (st(dt * rr(self, 1) + ld(green)), st(dt * rr(self, 2) + ld(blue)))
        };
        let c = |v: f32| col_real_to_ubyte(clamp(ld(v)));
        let out = if alpha {
            let na = st(dt * rr(self, 3) + ld(al));
            col_rgba(c(nr), c(ng), c(nb), c(na))
        } else {
            col_rgba(c(nr), c(ng), c(nb), 0xff)
        };
        self.set_var(ei, voff, out);
    }

    // ------------------------------------------------------------------ function calls

    fn function_call(&mut self, ei: usize, stat: usize, b: usize, at: u32) -> u32 {
        let w = |s: &Self, k: u32| s.word(stat, b, at + 4 * k);
        let fun_id = w(self, 1);
        let n = w(self, 2);
        let ret = w(self, 3);
        let mut args = Vec::with_capacity(n as usize + 1);
        let mut kinds = Vec::with_capacity(n as usize + 1);
        for i in 0..n {
            let ty = w(self, 5 + 2 * i);
            let v = w(self, 6 + 2 * i);
            let (val, kind) = match ty as u8 {
                evt::CONSTANT | evt::LABEL => (v, ArgKind::Value),
                evt::CONST_LABEL => (v, ArgKind::ConstPtr),
                evt::VAR_LABEL => (v, ArgKind::VarPtr),
                _ => (self.var(ei, v), ArgKind::Value),
            };
            args.push(val);
            kinds.push(kind);
        }
        while args.len() < 10 {
            args.push(0);
            kinds.push(ArgKind::Value);
        }
        let size = bytecode::function_size(n);
        let Some(fun) = Fun::from_u32(fun_id) else {
            self.warnings.push(format!("bad function id {fun_id}"));
            return size;
        };
        let result = self.call(ei, stat, fun, &args, &kinds);
        if ret != 0xffff_ffff {
            self.set_var(ei, ret, result);
        }
        size
    }

    fn data_ref(&self, ei: usize, stat: usize, value: u32, kind: ArgKind) -> Option<DataRef> {
        match kind {
            ArgKind::ConstPtr => Some(DataRef::Const { stat: stat as u32, offset: value }),
            ArgKind::VarPtr => Some(DataRef::Var { effect: self.effects[ei].id, offset: value }),
            ArgKind::Value => None,
        }
    }

    fn sys_mut(&mut self, handle: u32) -> Option<&mut PartSystem> {
        self.systems.get_mut((handle as usize).wrapping_sub(1))?.as_mut()
    }

    fn for_particles(&mut self, handle: u32, mut fun: impl FnMut(&mut particle::Particle)) {
        if let Some(s) = self.sys_mut(handle) {
            for p in &mut s.particles {
                fun(p);
            }
        }
    }

    #[allow(clippy::cognitive_complexity)]
    fn call(&mut self, ei: usize, stat: usize, fun: Fun, a: &[u32], k: &[ArgKind]) -> u32 {
        let fa = |i: usize| f(a[i]);
        let pat = &mut self.pat;
        match fun {
            Fun::SetDefaults => {
                self.eff_offset = EffOffset::default();
                self.pat = Pat::defaults();
            }
            Fun::SetDrag => pat.drag = st(1.0 - ld(fa(0))),
            Fun::SetScale => pat.scale = fa(0),
            Fun::SetXyzScale => {
                pat.flags |= sysflag::XYZSCALE;
                pat.xyz_scale = [fa(0), fa(1), fa(2)];
            }
            Fun::SetScaleDist => {
                pat.scale_dist = fa(0);
                if fa(0) == 0.0 {
                    pat.flags |= sysflag::MONOSCALE;
                } else {
                    pat.flags &= !sysflag::MONOSCALE;
                }
            }
            Fun::SetDeltaScale => pat.delta_scale = fa(0),
            Fun::SetDeltaScaleDist => pat.delta_scale_dist = fa(0),
            Fun::SetOffsetLof => pat.offset_lof = fa(0),
            Fun::SetOffsetR => pat.offset_r = fa(0),
            Fun::SetOffsetTheta => pat.offset_theta = fa(0),
            Fun::SetOffsetArray => {
                let r = self.data_ref(ei, stat, a[0], k[0]);
                self.pat.offset_array = r;
            }
            Fun::SetRandomOffsetArray => {
                let arr = particle::random_offset_array(&mut self.randy, [fa(0), fa(1), fa(2), fa(3), fa(4), fa(5)]);
                self.random_offsets = arr;
                self.pat.offset_array = Some(DataRef::RandomOffsets);
            }
            Fun::SetDeltaLof => pat.delta_lof = fa(0),
            Fun::SetDeltaLofDist => pat.delta_lof_dist = fa(0),
            Fun::SetDeltaR => pat.delta_r = [fa(0), fa(1)],
            Fun::SetDeltaRDist => {
                pat.delta_r_dist[0] = fa(0);
                pat.delta_r_dist[1] = if fa(1) == 0.0 { DELTA_R_DIST_DEFAULT } else { fa(1) };
            }
            Fun::SetVelLof => pat.vel_lof = fa(0),
            Fun::SetVelLofDist => pat.vel_lof_dist = fa(0),
            Fun::SetVelR => pat.vel_r = fa(0),
            Fun::SetVelRDist => pat.vel_r_dist = fa(0),
            Fun::SetAng => pat.ang = fa(0),
            Fun::SetAngDelta => pat.ang_delta = fa(0),
            Fun::SetDeltaVelLof => pat.delta_vel_lof = fa(0),
            Fun::SetDeltaVelLofDist => pat.delta_vel_lof_dist = fa(0),
            Fun::SetDeltaVelR => pat.delta_vel_r = fa(0),
            Fun::SetDeltaVelRDist => pat.delta_vel_r_dist = fa(0),
            Fun::SetAngDist => pat.ang_dist = fa(0),
            Fun::SetAngDeltaDist => pat.ang_delta_dist = fa(0),
            Fun::EtgSetColor => {
                let c = a[0];
                pat.icolor[0] = st(col_ubyte_to_real(c));
                pat.icolor[1] = st(col_ubyte_to_real(c >> 8));
                pat.icolor[2] = st(col_ubyte_to_real(c >> 16));
                pat.flags &= !sysflag::ALPHA;
            }
            Fun::SetColorDist => {
                pat.color_dist = [fa(0), fa(1), fa(2), 0.0];
                pat.flags &= !sysflag::ALPHA;
            }
            Fun::SetDeltaColor => {
                pat.delta_color = [fa(0), fa(1), fa(2), 1.0];
                pat.flags &= !sysflag::ALPHA;
            }
            Fun::SetDeltaColorDist => {
                pat.delta_color_dist = [fa(0), fa(1), fa(2), 0.0];
                pat.flags &= !sysflag::ALPHA;
            }
            Fun::EtgSetColorA => {
                let c = a[0];
                pat.icolor = [
                    st(col_ubyte_to_real(c)),
                    st(col_ubyte_to_real(c >> 8)),
                    st(col_ubyte_to_real(c >> 16)),
                    st(col_ubyte_to_real(c >> 24)),
                ];
                pat.flags |= sysflag::ALPHA;
            }
            Fun::SetColorADist => {
                pat.color_dist = [fa(0), fa(1), fa(2), fa(3)];
                pat.flags |= sysflag::ALPHA;
            }
            Fun::SetDeltaColorA => {
                pat.delta_color = [fa(0), fa(1), fa(2), fa(3)];
                pat.flags |= sysflag::ALPHA;
            }
            Fun::SetDeltaColorADist => {
                pat.delta_color_dist = [fa(0), fa(1), fa(2), fa(3)];
                pat.flags |= sysflag::ALPHA;
            }
            Fun::SetColorBias => pat.bias = [fa(0), fa(1), fa(2)],
            Fun::SetColorBiasDist => pat.bias_dist = [fa(0), fa(1), fa(2)],
            Fun::SetLighting => pat.lit = (a[0] & 0xff) as u8,
            Fun::SetIsWorldspace => {
                if a[0] & 0xff != 0 {
                    pat.flags |= sysflag::WORLDSPACE;
                } else {
                    pat.flags &= !sysflag::WORLDSPACE;
                }
            }
            Fun::SetVelocityInWorldSpace => {
                if a[0] & 0xff != 0 {
                    pat.flags |= sysflag::WORLDSPACE_VELOCITY;
                } else {
                    pat.flags &= !sysflag::WORLDSPACE_VELOCITY;
                }
            }
            Fun::SetIllum => pat.illum = fa(0),
            Fun::SetIllumDist => pat.illum_dist = fa(0),
            Fun::SetDeltaIllum => pat.delta_illum = fa(0),
            Fun::SetDeltaIllumDist => pat.delta_illum_dist = fa(0),
            Fun::SetLifespan => pat.lifespan = fa(0),
            Fun::SetLifespanDist => pat.lifespan_dist = fa(0),
            Fun::SetWaitspan => pat.waitspan = fa(0),
            Fun::SetWaitspanDist => pat.waitspan_dist = fa(0),
            Fun::SetLength => pat.length = fa(0),
            Fun::SetLengthDist => pat.length_dist = fa(0),
            Fun::SetDeltaLength => {
                pat.clamp_length = 0;
                pat.delta_length = fa(0);
            }
            Fun::SetDeltaLengthDist => pat.delta_length_dist = fa(0),
            Fun::SetSlices => pat.slices = if a[0] > 1 && a[0] < 0x41 { a[0] } else { 8 },
            Fun::SetTumble => pat.tumble = [fa(0), fa(1), fa(2)],
            Fun::SetDeltaTumble => pat.delta_tumble = [fa(0), fa(1), fa(2)],
            Fun::EtgSetAnimation => {
                let r = self.data_ref(ei, stat, a[0], k[0]);
                self.pat.tstruct = r;
                self.pat.rate = fa(1);
                self.pat.loop_flag = u8::from(a[2] != 0);
            }
            Fun::EtgSetMorphAnimation => {
                let r = self.data_ref(ei, stat, a[0], k[0]);
                self.pat.mstruct = r;
                self.pat.mesh_rate = fa(1);
                self.pat.loop_count = a[2] as i32;
            }
            Fun::SetMeshStartFrame => pat.start_mesh_frame = fa(0),
            // Also bound to `setFramerate`: the low byte of the float's bits.
            Fun::SetLoopFlag => pat.loop_flag = (a[0] & 0xff) as u8,
            Fun::SetStartFrame => pat.start_frame = (a[0] & 0xffff) as u16,
            Fun::SetAlphaMode => match a[0] {
                1 => {
                    pat.additive_blends = 1;
                    pat.stipple_alpha = 0;
                }
                2 => {
                    pat.additive_blends = 0;
                    pat.stipple_alpha = 1;
                }
                _ => {
                    pat.additive_blends = 0;
                    pat.stipple_alpha = 0;
                }
            },
            Fun::EtgSetSpecular => {
                if fa(0) == 0.0 {
                    pat.specular_only = 0;
                } else {
                    pat.specular_only = 1;
                    pat.exponent = fa(0);
                }
            }
            Fun::SetColorScheme => pat.color_scheme = (a[0] & 0xff) as u8,
            Fun::SetTrueBillboard => pat.true_billboard = a[0],
            Fun::SetPseudoBillboard => pat.pseudo_billboard = a[0],
            Fun::EtgSetDepthWrite => pat.no_depth_write = u32::from(a[0] == 0),
            Fun::SetTurbulance => pat.turbulance = fa(0),
            Fun::SetSegments => pat.segments = (a[0] & 0xffff) as u16,
            Fun::EtgModifyDrag => {
                let d = st(1.0 - ld(fa(1)));
                if let Some(s) = self.sys_mut(a[0]) {
                    if f64::from(d).abs() < 0.0001 {
                        s.drag = 0.0;
                    } else if (0.0..=1.0).contains(&d) {
                        s.drag = d;
                    }
                }
            }
            Fun::ModifyScale => {
                let v = fa(1);
                self.for_particles(a[0], |p| {
                    p.scale_x = v;
                    p.scale_y = v;
                });
            }
            Fun::ModifyScaleX => {
                let v = fa(1);
                self.for_particles(a[0], |p| p.scale_x = v);
            }
            Fun::ModifyScaleY => {
                let v = fa(1);
                self.for_particles(a[0], |p| p.scale_y = v);
            }
            Fun::EmptyStub => {}
            Fun::ModifyDeltaScale => {
                let v = fa(1);
                self.for_particles(a[0], |p| p.delta_scale = v);
            }
            Fun::ModifyDeltaLength => {
                let (v, c) = (fa(1), a[2]);
                self.for_particles(a[0], |p| {
                    p.delta_length = v;
                    p.clamp_length = c;
                });
            }
            Fun::ModifyColorC => {
                let c = a[1];
                let r = st(col_ubyte_to_real(c));
                let g = st(col_ubyte_to_real(c >> 8));
                let b = st(col_ubyte_to_real(c >> 16));
                let al = col_ubyte_to_real(c >> 24);
                if let Some(s) = self.sys_mut(a[0]) {
                    if al <= f64::from(0.996f32) {
                        s.flags |= sysflag::ALPHA;
                    } else {
                        s.flags &= !sysflag::ALPHA;
                    }
                    for p in &mut s.particles {
                        p.icolor = [r, g, b, st(al)];
                    }
                }
            }
            Fun::ModifyAlphaValue => {
                let v = fa(1);
                if let Some(s) = self.sys_mut(a[0]) {
                    if v <= 0.996 {
                        s.flags |= sysflag::ALPHA;
                    } else {
                        s.flags &= !sysflag::ALPHA;
                    }
                    for p in &mut s.particles {
                        p.icolor[3] = v;
                    }
                }
            }
            Fun::ModifyColorBias => {
                let b = [fa(1), fa(2), fa(3)];
                self.for_particles(a[0], |p| p.bias = b);
            }
            Fun::ModifyAddColor => {
                let c = a[1];
                let b = [st(col_ubyte_to_real(c)), st(col_ubyte_to_real(c >> 8)), st(col_ubyte_to_real(c >> 16))];
                self.for_particles(a[0], |p| p.bias = b);
            }
            Fun::ModifyDeltaColor => {
                let d = [fa(1), fa(2), fa(3), fa(4)];
                self.for_particles(a[0], |p| p.delta_color = d);
            }
            Fun::ModifyLighting => {
                let v = (a[1] & 0xff) as u8;
                self.for_particles(a[0], |p| p.lit = v);
            }
            Fun::ModifyIllum => {
                let v = fa(1);
                self.for_particles(a[0], |p| p.illum = v);
            }
            Fun::ModifyDeltaIllum => {
                let v = fa(1);
                self.for_particles(a[0], |p| p.delta_illum = v);
            }
            Fun::ModifyLifespan => {
                let v = fa(1);
                self.for_particles(a[0], |p| p.lifespan = v);
            }
            Fun::ModifyAnimation => {
                let r = self.data_ref(ei, stat, a[1], k[1]);
                if let Some(s) = self.sys_mut(a[0]) {
                    if matches!(s.kind, PartType::Billboard | PartType::Mesh) {
                        for p in &mut s.particles {
                            p.tstruct = r;
                        }
                    }
                }
            }
            Fun::ModifyFramerate => {
                let v = fa(1);
                if let Some(s) = self.sys_mut(a[0]) {
                    if s.kind == PartType::Billboard {
                        for p in &mut s.particles {
                            p.rate = v;
                        }
                    }
                }
            }
            Fun::ModifyVelLof => {
                let v = fa(1);
                self.for_particles(a[0], |p| p.vel_lof = v);
            }
            Fun::ModifyDeltaVelLof => {
                let v = fa(1);
                self.for_particles(a[0], |p| p.delta_vel_lof = v);
            }
            Fun::ModifyVelR => {
                let v = fa(1);
                self.for_particles(a[0], |p| p.vel_r = v);
            }
            Fun::ModifyDeltaVelR => {
                let v = fa(1);
                self.for_particles(a[0], |p| p.delta_vel_r = v);
            }
            Fun::ModifyMesh => {
                let v = a[1];
                self.for_particles(a[0], |p| p.mesh = v);
            }
            Fun::ModifyMorph => {
                let r = self.data_ref(ei, stat, a[1], k[1]);
                if let Some(s) = self.sys_mut(a[0]) {
                    if s.kind == PartType::Mesh {
                        for p in &mut s.particles {
                            p.mstruct = r;
                        }
                    }
                }
            }
            Fun::ModifyMorphFramerate => {
                let v = fa(1);
                if let Some(s) = self.sys_mut(a[0]) {
                    if s.kind == PartType::Mesh {
                        for p in &mut s.particles {
                            p.mesh_rate = v;
                        }
                    }
                }
            }
            Fun::ModifyLength => {
                let v = fa(1);
                self.for_particles(a[0], |p| p.length = v);
            }
            Fun::ModifyAlphaMode => {
                let mode = a[1];
                self.for_particles(a[0], |p| match mode {
                    1 => p.flags = (p.flags | particle::pflag::ADDITIVE) & !particle::pflag::STIPPLE,
                    2 => p.flags = (p.flags & !particle::pflag::ADDITIVE) | particle::pflag::STIPPLE,
                    _ => p.flags &= !(particle::pflag::ADDITIVE | particle::pflag::STIPPLE),
                });
            }
            Fun::ModifyExponent => {
                let v = fa(1);
                self.for_particles(a[0], |p| {
                    p.exponent = v;
                    p.flags = (p.flags & !particle::pflag::SPECULAR) | u32::from(v != 0.0);
                });
            }
            Fun::ModifyColorScheme => {
                let v = (a[1] & 0xff) as u8;
                self.for_particles(a[0], |p| p.color_scheme = v);
            }
            Fun::ModifyPseudoBillboard => {
                let on = a[1] != 0;
                self.for_particles(a[0], |p| {
                    if on {
                        p.flags |= particle::pflag::PSEUDO_BILLBOARD;
                    } else {
                        p.flags &= !particle::pflag::PSEUDO_BILLBOARD;
                    }
                });
            }
            Fun::ModifyRotAngle => {
                let v = fa(1);
                self.for_particles(a[0], |p| p.rot = v);
            }
            Fun::ModifyTumble => {
                let t = [fa(1), fa(2), fa(3)];
                self.for_particles(a[0], |p| p.tumble = t);
            }
            Fun::CreateSprites | Fun::CreateCircles => {
                if fun == Fun::CreateCircles {
                    self.pat.tex = particle::TR_INVALID;
                    self.pat.uv = [[0, 0], [0, 0]];
                }
                return self.create_system(ei, PartType::Billboard, a[0], a[1]);
            }
            Fun::CreatePoints => return self.create_system(ei, PartType::Points, a[0], a[1]),
            Fun::CreateLines => return self.create_system(ei, PartType::Lines, a[0], a[1]),
            Fun::CreateMeshes => return self.create_system(ei, PartType::Mesh, a[0], a[1]),
            Fun::CreateEBolts => return self.create_system(ei, PartType::EBolt, a[0], a[1]),
            Fun::CreateLensFlares => return self.create_system(ei, PartType::LensFlare, a[0], a[1]),
            Fun::Inc => return a[0].wrapping_add(1),
            Fun::Dec => return a[0].wrapping_sub(1),
            Fun::Fdiv => return st(ld(fa(0)) / ld(fa(1))).to_bits(),
            Fun::Fmult => return st(ld(fa(1)) * ld(fa(0))).to_bits(),
            Fun::FDegSin => return st(fsin(ld(fa(0)) * DEG_SIN_TWO_PI * DEG_SIN_ONE_OVER_360)).to_bits(),
            Fun::Fadd => return st(ld(fa(0)) + ld(fa(1))).to_bits(),
            Fun::Fsub => return st(ld(fa(0)) - ld(fa(1))).to_bits(),
            Fun::Div => {
                let (x, y) = (a[0] as i32, a[1] as i32);
                return if y == 0 { 0 } else { x.wrapping_div(y) as u32 };
            }
            Fun::Mult => return (a[0] as i32).wrapping_mul(a[1] as i32) as u32,
            Fun::Add => return a[0].wrapping_add(a[1]),
            Fun::Sub => return a[0].wrapping_sub(a[1]),
            Fun::Sin => return st(fsin(ld(fa(0)))).to_bits(),
            Fun::Cos => return st(fcos(ld(fa(0)))).to_bits(),
            Fun::FRandom => {
                let (lo, hi) = (fa(0), fa(1));
                #[allow(clippy::neg_cmp_op_on_partial_ord)]
                let empty = !(lo < hi);
                if empty {
                    return a[0];
                }
                let u = f64::from(self.randy.random(RAN_ETG_CAT));
                let v = st((ld(hi) - ld(lo)) * u * f64::from(2.328_306_4e-10f32) + ld(lo));
                if lt_or_unordered(ld(v), ld(lo)) {
                    return a[0];
                }
                return if v > hi { a[1] } else { v.to_bits() };
            }
            Fun::IRandom => {
                let (lo, hi) = (a[0], a[1]);
                if hi <= lo {
                    return lo;
                }
                return self.randy.random(RAN_ETG_CAT) % (hi - lo) + lo;
            }
            Fun::CRandom => {
                let (lo_r, mut hi_r, lo_g, lo_b, mut hi_b) = (a[0], a[1], a[2], a[4], a[5]);
                if lo_b <= hi_b {
                    hi_b = lo_b.wrapping_add(1);
                }
                if lo_r <= hi_r {
                    hi_r = lo_r.wrapping_add(1);
                }
                let r1 = self.randy.random(RAN_ETG_CAT);
                let r2 = self.randy.random(RAN_ETG_CAT);
                let r3 = self.randy.random(RAN_ETG_CAT);
                let m = |r: u32, lo: u32, hi: u32| (r % hi.wrapping_sub(lo).wrapping_add(1)).wrapping_add(lo) & 0xff;
                // Note the original's channel mix-ups: green uses the blue upper bound.
                let blue = m(r1, lo_b, hi_b);
                let green = m(r2, lo_g, hi_b);
                let red = m(r3, lo_r, hi_r);
                return col_rgba(red, green, blue, 0xff);
            }
            Fun::CaRandom => {
                let (lo_r, mut hi_r, lo_g, lo_b, mut hi_b, lo_a, mut hi_a) = (a[0], a[1], a[2], a[4], a[5], a[6], a[7]);
                if lo_a <= hi_a {
                    hi_a = lo_a.wrapping_add(1);
                }
                if lo_b <= hi_b {
                    hi_b = lo_b.wrapping_add(1);
                }
                if lo_r <= hi_r {
                    hi_r = lo_r.wrapping_add(1);
                }
                let r1 = self.randy.random(RAN_ETG_CAT);
                let r2 = self.randy.random(RAN_ETG_CAT);
                let r3 = self.randy.random(RAN_ETG_CAT);
                let r4 = self.randy.random(RAN_ETG_CAT);
                let m = |r: u32, lo: u32, hi: u32| (r % hi.wrapping_sub(lo).wrapping_add(1)).wrapping_add(lo);
                return m(r1, lo_a, hi_a).wrapping_mul(0x100_0000)
                    | m(r2, lo_g, hi_b).wrapping_mul(0x100)
                    | m(r3, lo_b, hi_b).wrapping_mul(0x1_0000)
                    | m(r4, lo_r, hi_r);
            }
            Fun::ParentShipDelete => {
                let e = &self.effects[ei];
                if let Some(oid) = e.owner {
                    let o = self.owners.get(&oid).cloned().unwrap_or_default();
                    let speech = e.effect_flags & EAF_PLAYING_SPEECH != 0 && o.is_ship;
                    if o.is_ship || o.is_derelict {
                        self.events.push(FxEvent::DeleteParent { owner: oid, derelict: !o.is_ship, speech_playing: speech });
                    }
                    if speech {
                        self.effects[ei].effect_flags &= !EAF_PLAYING_SPEECH;
                    }
                    let n = self.effects[ei].n_spawned_attached;
                    if n > 0 {
                        self.detach_owner(ei, oid, n);
                    }
                    self.effects[ei].owner = None;
                }
            }
            Fun::ParentShipHide => {
                if let Some(o) = self.effects[ei].owner {
                    self.events.push(FxEvent::HideParent { owner: o });
                }
            }
            Fun::ParentShipOmitFromRenderList => {
                if let Some(o) = self.effects[ei].owner {
                    self.events.push(FxEvent::OmitParentFromRenderList { owner: o });
                }
            }
            Fun::SoundEffect => {
                let e = &self.effects[ei];
                self.events.push(FxEvent::Sound { effect: e.id, owner: e.owner, event: a[0], position: e.position });
            }
            Fun::ChatterEventPlay => {
                if let Some(o) = self.effects[ei].owner {
                    let id = self.effects[ei].id;
                    self.events.push(FxEvent::Chatter { effect: id, owner: o, event: a[0] });
                    self.effects[ei].effect_flags |= EAF_PLAYING_SPEECH;
                }
            }
            Fun::DeathCriesPlay => {
                if let Some(o) = self.effects[ei].owner {
                    self.events.push(FxEvent::DeathCries { owner: o });
                }
            }
            Fun::OwnerColorSchemeGet => {
                return self.effects[ei]
                    .owner
                    .and_then(|o| self.owners.get(&o))
                    .map(|o| u32::from(o.color_scheme))
                    .unwrap_or(0);
            }
            Fun::EffectVelocityGet => {
                let e = &self.effects[ei];
                let v = e.owner.and_then(|o| self.owners.get(&o)).map(|o| o.velocity).unwrap_or(e.velocity);
                let sq = ld(v.z) * ld(v.z) + ld(v.y) * ld(v.y) + ld(v.x) * ld(v.x);
                return st(fmath_sqrt_double(sq)).to_bits();
            }
            Fun::DamageDone => {
                if let Some(o) = self.effects[ei].owner {
                    let id = self.effects[ei].id;
                    self.events.push(FxEvent::DamageDone { effect: id, owner: o });
                }
                self.exec.delete = true;
            }
            Fun::EffOffsetLof => self.eff_offset.offset_lof = fa(0),
            Fun::EffChangeLof => {
                self.eff_offset.change_lof_theta = fa(0);
                self.eff_offset.change_lof_mu = fa(1);
            }
            Fun::EffOffsetR => {
                self.eff_offset.offset_r = fa(0);
                self.eff_offset.offset_theta = fa(1);
            }
            Fun::EffOffsetXyz => self.eff_offset.offset_xyz = Vector::new(fa(0), fa(1), fa(2)),
            Fun::EffOffsetVelLof => self.eff_offset.offset_vel_lof = fa(0),
            Fun::EffOffsetSpin => self.eff_offset.offset_spin = fa(0),
            Fun::EffOffsetTime => self.eff_offset.offset_time = fa(0),
            Fun::EffOffsetDrag => {
                let d = self.effects[ei].drag;
                self.eff_offset.drag = st((ld(d) + 1.0) - ld(fa(0)));
            }
            Fun::EffDrag => self.eff_offset.drag = st(1.0 - ld(fa(0))),
            Fun::EffDefaults => self.eff_offset = EffOffset::default(),
            Fun::EffAttachParent => self.eff_offset.attach_parent = a[0],
            Fun::EffSpin => self.eff_offset.spin = a[0],
            Fun::EffDeltaSpin => self.eff_offset.delta_spin = a[0],
            Fun::ThisOffsetXyz => {}
            Fun::ThisScaleVelocity => {
                let k = ld(fa(0));
                let e = &mut self.effects[ei];
                e.velocity = Vector::new(st(k * ld(e.velocity.x)), st(k * ld(e.velocity.y)), st(k * ld(e.velocity.z)));
            }
            Fun::ThisOffsetSpin => self.effects[ei].spin = fa(0),
            Fun::ThisOffsetDeltaSpin => self.effects[ei].delta_spin = fa(0),
            Fun::ThisOffsetTime => {
                let e = &mut self.effects[ei];
                e.time_elapsed = st(ld(e.time_elapsed) - ld(fa(0)));
            }
            Fun::ThisSetDrag => self.effects[ei].drag = st(1.0 - ld(fa(0))),
            Fun::Float2Int => return ftol_i32(ld(fa(0))) as u32,
            Fun::Int2Float => return (a[0] as i32 as f32).to_bits(),
            Fun::Ints2Color => return col_rgba(a[0], a[1], a[2], 0xff),
            Fun::Ints2ColorA => {
                return ((a[3] << 8 | a[2]) << 8 | a[1]) << 8 | a[0];
            }
            Fun::Floats2Color => {
                let c = |v: f32| col_real_to_ubyte(ld(v));
                return col_rgba(c(fa(0)), c(fa(1)), c(fa(2)), 0xff);
            }
            Fun::Floats2ColorA => {
                let c = |v: f32| col_real_to_ubyte(ld(v));
                return col_rgba(c(fa(0)), c(fa(1)), c(fa(2)), c(fa(3)));
            }
            Fun::ColRed => return st(col_ubyte_to_real(a[0])).to_bits(),
            Fun::ColGreen => return st(col_ubyte_to_real(a[0] >> 8)).to_bits(),
            Fun::ColBlue => return st(col_ubyte_to_real(a[0] >> 16)).to_bits(),
            Fun::SetTrHandle => {
                self.pat.tex = a[0];
                self.pat.uv = [[a[1], a[2]], [a[3], a[4]]];
            }
            Fun::ModifyTexture => {
                // Called with the texture handle where the system should be (see compiler).
                let (h, tex, uv) = (a[0], a[1], [a[2], a[3], a[4], a[5]]);
                if let Some(s) = self.sys_mut(h) {
                    particle::modify_texture(s, tex, uv);
                }
            }
            Fun::SetMeshdata => self.pat.mesh = a[0],
            Fun::SpawnNewEffect => return self.spawn_new_effect(ei, a[0] as usize, a[1], &a[2..]),
            Fun::CreateEffects => {
                self.create_effects(ei, a[0] as usize, a[1], a[2], a[3], &a[4..]);
            }
            Fun::CreateCallbackSetup => self.create_callback = Some((a[0], self.effects[ei].id)),
        }
        0
    }

    /// `etgDetatchThisOwner`: clear `owner` on up to `n` other effects, walking the list
    /// both ways from `ei` (the forward walk follows `prev` links, as in the original).
    fn detach_owner(&mut self, ei: usize, owner: OwnerId, n: i32) {
        let mut found = 0;
        let mut back = ei.checked_sub(1);
        let mut fwd = ei.checked_sub(1);
        while back.is_some() || fwd.is_some() {
            if let Some(i) = fwd {
                if self.effects[i].owner == Some(owner) {
                    self.effects[i].owner = None;
                    found += 1;
                    if found >= n {
                        break;
                    }
                }
                fwd = i.checked_sub(1);
            }
            if let Some(i) = back {
                if self.effects[i].owner == Some(owner) {
                    self.effects[i].owner = None;
                    found += 1;
                    if found == n {
                        break;
                    }
                }
                back = i.checked_sub(1);
            }
        }
    }

    /// `etgCreate*` + `partCreateSystemWithDelta` + `partCreationHelper`.
    fn create_system(&mut self, ei: usize, kind: PartType, number: u32, delta: u32) -> u32 {
        let (pos, mat, vel) = {
            let e = &self.effects[ei];
            (e.position, e.coordsys, e.velocity)
        };
        self.pat.position = pos;
        self.pat.mat = mat;
        self.pat.world_vel = vel;
        self.pat.effect_vel = vel;
        let rn = (number as f64) as f32;
        let d = (delta as f64) as f32;
        let count = hwc_math::fpu::ftol(particle::real_dist(&mut self.randy, rn, d)) as i32;
        let count = count.max(0) as u32;
        let mut sys = PartSystem::new_header(kind, &self.pat, self.universe_time);
        for i in 0..count {
            if let Some((value, eid)) = self.create_callback {
                self.run_creation_callback(value, eid);
            }
            let data = WorldData { lib: &self.lib, effects: &self.effects, random_offsets: &self.random_offsets };
            let mut env = CreateEnv { randy: &mut self.randy, data: &data, lens_flares: &mut self.lens_flares };
            let p = particle::make_particle(&self.pat, kind, i, &mut env);
            sys.particles.push(p);
        }
        self.create_callback = None;
        // The particle count is a 16-bit field.
        sys.particles.truncate((count & 0xffff) as usize);
        self.systems.push(Some(sys));
        let handle = self.systems.len() as u32;
        let e = &mut self.effects[ei];
        if e.blocks.len() as u32 >= e.n_particle_blocks {
            let msg = format!("{}: exceeded {} particle blocks", self.lib.statics[e.stat].name, e.n_particle_blocks);
            self.warnings.push(msg);
        }
        e.blocks.push(handle);
        handle
    }

    /// `etgCreationCallback`: run the per-particle callback code.
    fn run_creation_callback(&mut self, value: u32, eid: EffectId) {
        let Some(ei) = self.effects.iter().position(|e| e.id == eid) else { return };
        let saved_block = self.exec.block;
        let saved_off = self.exec.off[saved_block];
        self.exec.off[saved_block] = value;
        self.run(ei);
        self.exec.block = saved_block;
        self.exec.off[saved_block] = saved_off;
    }

    /// `etgSpawnNewEffect`.
    fn spawn_new_effect(&mut self, ei: usize, stat: usize, n_params: u32, params: &[u32]) -> u32 {
        if stat >= self.lib.statics.len() || !self.lib.statics[stat].is_loaded() {
            self.warnings.push(format!("spawn of unloaded effect #{stat}"));
            return 0;
        }
        let stat = self.pick_version(stat);
        if self.frequency_exceeded(stat) {
            return 0;
        }
        let off = self.eff_offset;
        let parent = self.effects[ei].clone();
        let mut e = self.new_effect(stat);
        if off.attach_parent != 0 && parent.owner.is_some() {
            e.flags = sof::ROTATABLE | sof::ATTACH_POSITION | sof::ATTACH_VELOCITY;
            e.owner = parent.owner;
            self.effects[ei].n_spawned_attached += 1;
        }
        e.current_lod = parent.current_lod;
        e.camera_distance_sq = parent.camera_distance_sq;
        e.time_elapsed = -off.offset_time;
        let mut position = Vector::new(off.offset_r, 0.0, off.offset_lof);
        if off.offset_r != 0.0 {
            let s = st(fsin(ld(off.offset_theta)));
            let c = st(fcos(ld(off.offset_theta)));
            let rot = Matrix::rot_about_z(c, s);
            position = rot.vec_mul(position);
        }
        let new_pos = Vector::new(
            st(ld(off.offset_xyz.x) + ld(position.x)),
            st(ld(off.offset_xyz.y) + ld(position.y)),
            st(ld(off.offset_xyz.z) + ld(position.z)),
        );
        let world = parent.coordsys.mul_vec(new_pos);
        e.position = Vector::new(
            st(ld(world.x) + ld(parent.position.x)),
            st(ld(world.y) + ld(parent.position.y)),
            st(ld(world.z) + ld(parent.position.z)),
        );
        if off.change_lof_theta == 0.0 && off.change_lof_mu == 0.0 {
            e.coordsys = parent.coordsys;
        } else {
            let st_ = st(fsin(ld(off.change_lof_theta)));
            let ct = st(fcos(ld(off.change_lof_theta)));
            let sm = st(fsin(ld(off.change_lof_mu)));
            let cm = st(fcos(ld(off.change_lof_mu)));
            let rot_mu = Matrix::rot_about_y(cm, sm);
            let rot_theta = Matrix::rot_about_z(ct, st_);
            let temp = rot_theta.mul_mat(&rot_mu);
            e.coordsys = parent.coordsys.mul_mat(&temp);
        }
        e.drag = parent.drag;
        e.velocity = parent.velocity;
        let n = (n_params as usize).min(params.len());
        self.code_start(&mut e, &params[..n]);
        if self.lib.statics[stat].special_ops & ESO_SORT_FORWARD != 0 {
            e.flags |= sof::ALWAYS_SORT_FRONT;
        }
        e.n_spawned_attached = 0;
        e.nlips = parent.nlips;
        e.spin = f(off.spin);
        e.delta_spin = f(off.delta_spin);
        let id = e.id;
        self.effects.push(e);
        id
    }

    /// `etgCreateEffects`: one effect per particle of a temporary point system.
    fn create_effects(&mut self, ei: usize, stat: usize, number: u32, dist: u32, n_params: u32, params: &[u32]) {
        if stat >= self.lib.statics.len() || !self.lib.statics[stat].is_loaded() {
            self.warnings.push(format!("createEffects of unloaded effect #{stat}"));
            return;
        }
        let stat = self.pick_version(stat);
        let parent = self.effects[ei].clone();
        let up = parent.coordsys.col(0);
        let mut number = number as i32;
        if dist != 0 {
            let r = self.randy.random(RAN_ETG_CAT);
            let span = (dist as i32).wrapping_mul(2) as u32;
            number = number.wrapping_add((r % span) as i32 - dist as i32);
        }
        self.pat.position = parent.position;
        self.pat.mat = parent.coordsys;
        self.pat.world_vel = parent.velocity;
        self.pat.effect_vel = parent.velocity;
        let count = number.max(0) as u32;
        let mut sys = PartSystem::new_header(PartType::Points, &self.pat, self.universe_time);
        for i in 0..count {
            if let Some((value, eid)) = self.create_callback {
                self.run_creation_callback(value, eid);
            }
            let data = WorldData { lib: &self.lib, effects: &self.effects, random_offsets: &self.random_offsets };
            let mut env = CreateEnv { randy: &mut self.randy, data: &data, lens_flares: &mut self.lens_flares };
            sys.particles.push(particle::make_particle(&self.pat, PartType::Points, i, &mut env));
        }
        self.create_callback = None;
        let world = sys.flags & sysflag::WORLDSPACE != 0;
        for p in sys.particles.iter_mut() {
            if self.frequency_exceeded(stat) {
                continue;
            }
            let mut e = self.new_effect(stat);
            e.current_lod = parent.current_lod;
            e.camera_distance_sq = parent.camera_distance_sq;
            e.time_elapsed = -p.waitspan;
            e.drag = sys.drag;
            if p.vel_r != 0.0 {
                let k = ld(p.vel_r);
                p.rvec = Vector::new(st(k * ld(p.rvec.x)), st(k * ld(p.rvec.y)), st(k * ld(p.rvec.z)));
            }
            p.rvec.z = p.vel_lof;
            if world {
                e.velocity = p.w_vel;
                e.position = p.position;
            } else {
                e.velocity = parent.coordsys.mul_vec(p.rvec);
                let wp = parent.coordsys.mul_vec(p.position);
                e.position = Vector::new(
                    st(ld(wp.x) + ld(parent.position.x)),
                    st(ld(parent.position.y) + ld(wp.y)),
                    st(ld(parent.position.z) + ld(wp.z)),
                );
            }
            let lof = parent.coordsys.mul_vec(p.rvec);
            let mut right = lof.cross(up);
            let mut temp = lof.cross(right);
            let mut lofn = lof;
            lofn.normalize();
            right.normalize();
            temp.normalize();
            e.coordsys = Matrix::from_cols(temp, right, lofn);
            let n = (n_params as usize).min(params.len());
            self.code_start(&mut e, &params[..n]);
            let ops = self.lib.statics[stat].special_ops;
            if ops & ESO_SORT_FORWARD != 0 {
                e.flags |= sof::ALWAYS_SORT_FRONT;
            }
            e.owner = None;
            e.n_spawned_attached = 0;
            e.nlips = parent.nlips;
            e.spin = f(self.eff_offset.spin);
            e.delta_spin = f(self.eff_offset.delta_spin);
            if ops & ESO_WORLD_RENDER != 0 {
                e.flags |= sof::HIDE;
            }
            self.effects.push(e);
        }
    }

    // ------------------------------------------------------------------ drawing

    /// `etgEffectDraw` for every effect: advance particle systems by the universe time since
    /// they were last drawn and, when `emit` is set, append render data to `out`.
    pub fn draw(&mut self, mut out: Option<&mut crate::render::RenderList>) {
        for ei in 0..self.effects.len() {
            self.draw_effect(ei, out.as_deref_mut());
        }
    }

    fn draw_effect(&mut self, ei: usize, mut out: Option<&mut crate::render::RenderList>) {
        if self.effects[ei].time_elapsed < 0.0 {
            return;
        }
        if let Some(o) = self.effects[ei].owner.and_then(|id| self.owners.get(&id)) {
            if self.effects[ei].flags & sof::ATTACH_NLIPS != 0 {
                let n = o.nlips;
                self.effects[ei].nlips = n;
            }
        }
        let e = self.effects[ei].clone();
        let scaled = Matrix::copy_and_scale(&e.coordsys, e.nlips);
        let now = self.universe_time;
        for &h in &e.blocks {
            let idx = (h as usize).wrapping_sub(1);
            let Some(mut sys) = self.systems.get_mut(idx).and_then(Option::take) else { continue };
            let dt = st(ld(now) - ld(sys.last_updated));
            if dt >= 0.0 {
                let data = WorldData { lib: &self.lib, effects: &self.effects, random_offsets: &self.random_offsets };
                sys.update(dt, &data, &mut self.lens_flares);
                sys.last_updated = now;
            }
            if let Some(list) = out.as_deref_mut() {
                let owner = e.owner.and_then(|id| self.owners.get(&id)).cloned();
                if sys.kind == PartType::Billboard && sys.flags & sysflag::WORLDSPACE == 0 && e.flags & sof::OWNER_COORDSYS == 0 {
                    sys.position = e.position;
                }
                let data = WorldData { lib: &self.lib, effects: &self.effects, random_offsets: &self.random_offsets };
                let randy = match self.draw_randy.as_mut() {
                    Some(r) => r,
                    None => &mut self.randy,
                };
                let hits = crate::emit::emit_system_with(
                    &mut sys,
                    &data,
                    &e,
                    &scaled,
                    owner.as_ref(),
                    &self.lib,
                    randy,
                    &mut self.lens_flares,
                    list,
                );
                if hits == 0 {
                    sys.is_alive = false;
                }
            }
            self.systems[idx] = Some(sys);
        }
        self.effects[ei].effect_flags |= EF_RENDERED;
    }

    /// One universe update followed by one draw (the usual headless loop).
    pub fn step(&mut self, dt: f32, out: Option<&mut crate::render::RenderList>) {
        self.update(dt);
        self.draw(out);
    }
}

impl std::fmt::Debug for FxWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FxWorld")
            .field("effects", &self.effects.len())
            .field("systems", &self.systems.iter().flatten().count())
            .field("universe_time", &self.universe_time)
            .finish()
    }
}
