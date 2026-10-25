//! Playing a loaded NIS (`nisStart` 0x4c86e0, `nisUpdate` 0x4c9bb0, `nisCamPathUpdate`
//! 0x4c9a30, `nisSeek` 0x4c9710, `nisStop` 0x4c9190 and the NIS task `nisUpdateTask`
//! 0x4c7a30).
//!
//! [`NisPlayer`] holds what the original keeps in its `nisplaying` structure and the NIS
//! globals: the six curves of every moving object and camera, the object poses it computes,
//! the NIS camera, the fade to black and the event cursor. It does not touch the universe: the
//! poses are read back by the caller ([`crate::stage`] puts them on the simulation's objects)
//! and every event is handed back as a [`Fired`] for the stage and the presentation layer.
//!
//! Each NIS task step ([`NisPlayer::tick`], every 1/16 s) first moves the fade, then advances
//! every curve by 1/16 s, poses the objects and the camera, and then runs the events whose time
//! has come (`time <= elapsed + dt`). Cataclysm's curves hold their last key instead of
//! ending, so the NIS ends only when its time passes its length.

use hwc_math::fpu::{ld, st};
use hwc_math::spline::{SplineCache, SplineCurve};
use hwc_math::{Matrix, Vector};

use crate::data::{NisData, FIRST_KEYFRAME};
use crate::euler::ship_euler_to_matrix;
use crate::events::{Action, Event};

/// The NIS task's step (`UNIVERSE_UPDATE_PERIOD`).
pub const TICK: f32 = 0.0625;
/// `NIS_LookScalar`: the camera looks at a point this far along its forward axis.
pub const LOOK_SCALAR: f32 = 10000.0;
/// `NIS_DefaultFOV`.
pub const DEFAULT_FOV: f32 = 75.0;
/// `NIS_JumpDistance`: an object moving this far in one step has its trails cut.
pub const JUMP_DISTANCE: f32 = 300.0;
/// `NIS_CameraLengthFactor`.
pub const CAMERA_LENGTH_FACTOR: f32 = 0.1;

/// `objectmotion.flags`.
pub mod omf {
    pub const OBJECT_DIED: u32 = 0x01;
    pub const REMAIN_AT_END: u32 = 0x02;
    pub const KEEP_ON_MOVING: u32 = 0x10;
    pub const CURVES_DELETED: u32 = 0x20;
}

/// One object of the NIS and where the NIS last put it.
#[derive(Debug, Clone)]
pub struct ObjectState {
    pub position: Vector,
    pub coordsys: Matrix,
    /// Movement over the last step (the original stores the displacement, not a speed).
    pub velocity: Vector,
    pub moving: bool,
    /// Moved at least [`JUMP_DISTANCE`] in the last step (the ship's trails are cut).
    pub jumped: bool,
    /// [`omf`] flags.
    pub flags: u32,
    /// The pose changed in the last step (static paths are posed once, at the start).
    pub posed: bool,
    curves: Option<Vec<SplineCurve>>,
}

/// The NIS camera (`Camera` fields the NIS sets).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NisCamera {
    pub eye: Vector,
    pub look_at: Vector,
    pub up: Vector,
    /// Field of view in degrees.
    pub fov: f32,
    pub clip_near: Option<f32>,
    pub clip_far: Option<f32>,
}

/// The fade to black or white (`nisBlackFade`, `nisBlackFadeDest`, `nisBlackFadeRate`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BlackFade {
    pub level: f32,
    pub dest: f32,
    pub rate: f32,
    /// The fade colour is white (`fadeSet`'s second argument).
    pub white: bool,
}

impl BlackFade {
    /// The fade part of `nisUpdateTask`: it moves at its rate until it is within one step's
    /// worth (1/16, whatever the rate) of its destination, then snaps.
    pub fn tick(&mut self) {
        if self.level != self.dest {
            let d = st(ld(self.level) - ld(self.dest));
            let d = if 0.0 <= d { d } else { -d };
            if TICK < d {
                self.level = st(ld(self.rate) * ld(TICK) + ld(self.level));
            } else {
                self.level = self.dest;
                self.rate = 0.0;
            }
        }
    }
}

/// An event that ran.
#[derive(Debug, Clone, PartialEq)]
pub struct Fired {
    pub object: Option<usize>,
    pub action: Action,
    /// It ran while seeking (skipping to the end): sounds, speech and deaths are skipped by
    /// their handlers.
    pub seeking: bool,
}

/// What one step did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Step {
    pub fired: Vec<Fired>,
    /// The NIS is over (`nisUpdate` returned `REALlyBig`): call [`NisPlayer::stop`].
    pub finished: bool,
}

/// An NIS playing (`nisplaying`).
#[derive(Debug, Clone)]
pub struct NisPlayer {
    pub data: NisData,
    /// Where the NIS plays and its attitude (`nisPosition`, `nisMatrix`).
    pub position: Vector,
    pub matrix: Matrix,
    /// Seconds played.
    pub time: f32,
    pub objects: Vec<ObjectState>,
    cameras: Vec<Option<Vec<SplineCurve>>>,
    pub camera: NisCamera,
    pub fade: BlackFade,
    next_event: usize,
    seeked: bool,
    /// The spline module's shared cache (`bsCurveUpdate`'s statics).
    cache: SplineCache,
    /// `nisMusicPlaying`.
    pub music_playing: bool,
    /// `nisUniverseHidden`.
    pub universe_hidden: bool,
    /// `nisUniversePause`.
    pub universe_paused: bool,
    /// Script focus at the end (`bCameraFocussed`); never set by the shipped scripts.
    pub camera_focussed: bool,
}

/// `x + y` stored to a `float`.
fn add(a: Vector, b: Vector) -> Vector {
    Vector::new(st(ld(a.x) + ld(b.x)), st(ld(a.y) + ld(b.y)), st(ld(a.z) + ld(b.z)))
}

fn sub(a: Vector, b: Vector) -> Vector {
    Vector::new(st(ld(a.x) - ld(b.x)), st(ld(a.y) - ld(b.y)), st(ld(a.z) - ld(b.z)))
}

/// Six curve values (each rounded to a `float` as the caller stores it).
fn sample(curves: &mut [SplineCurve], cache: &mut SplineCache, dt: f32) -> [f32; 6] {
    std::array::from_fn(|c| st(curves[c].update(cache, dt)))
}

impl NisPlayer {
    /// `nisStart`: creates the motion state at `position` with attitude `matrix`, poses the
    /// one-keyframe objects and the camera, and runs the events at time 0. `main_eye` is the
    /// game camera's eye (the first frame's look-at distance depends on it).
    pub fn start(data: NisData, position: Vector, matrix: Matrix, main_eye: Vector) -> (NisPlayer, Step) {
        let mut objects = Vec::with_capacity(data.objects.len());
        for path in &data.objects {
            let c = &path.curves;
            // `nisNewObjectCreate`: created at the first sample (unrotated) plus the position.
            let start = add(Vector::new(c.value(0, 0), c.value(1, 0), c.value(2, 0)), position);
            let mut st_ = ObjectState {
                position: start,
                coordsys: crate::euler::HPB_IDENTITY_SHIP,
                velocity: Vector::ZERO,
                moving: false,
                jumped: false,
                flags: 0,
                posed: true,
                curves: None,
            };
            if c.is_static() {
                let k = FIRST_KEYFRAME;
                let local = Vector::new(-c.value(2, k), c.value(0, k), c.value(1, k));
                st_.position = add(position, matrix.mul_vec(local));
                let rot = ship_euler_to_matrix(c.value(3, k), c.value(4, k), c.value(5, k));
                st_.coordsys = matrix.mul_mat(&rot);
            } else {
                st_.curves = Some((0..6).map(|ch| c.curve(ch)).collect());
            }
            objects.push(st_);
        }
        let cameras: Vec<Option<Vec<SplineCurve>>> = data
            .cameras
            .iter()
            .map(|cam| (!cam.curves.is_static()).then(|| (0..6).map(|ch| cam.curves.curve(ch)).collect()))
            .collect();
        let mut camera = NisCamera {
            eye: position,
            look_at: add(position, Vector::new(0.0, LOOK_SCALAR, 0.0)),
            up: Vector::new(0.0, 0.0, 1.0),
            fov: DEFAULT_FOV,
            clip_near: None,
            clip_far: None,
        };
        // The starting camera from camera 0's first sample; its look-at distance is a tenth
        // of the distance from the game camera until the first update.
        if let Some(cam) = data.cameras.first() {
            let c = &cam.curves;
            let local = Vector::new(-c.value(2, 0), c.value(0, 0), c.value(1, 0));
            camera.eye = add(position, matrix.mul_vec(local));
            let rot = ship_euler_to_matrix(c.value(3, 0), c.value(4, 0), c.value(5, 0));
            let m = matrix.mul_mat(&rot);
            let d = sub(camera.eye, main_eye).dist_sloppy();
            let len = ld(d) * ld(CAMERA_LENGTH_FACTOR);
            let f = m.col(1);
            let dir = Vector::new(st(len * ld(f.x)), st(len * ld(f.y)), st(len * ld(f.z)));
            camera.up = m.col(0);
            camera.look_at = add(dir, camera.eye);
        }
        let mut p = NisPlayer {
            data,
            position,
            matrix,
            time: 0.0,
            objects,
            cameras,
            camera,
            fade: BlackFade::default(),
            next_event: 0,
            seeked: false,
            cache: SplineCache::default(),
            music_playing: false,
            universe_hidden: false,
            universe_paused: false,
            camera_focussed: false,
        };
        let mut step = Step::default();
        while p.next_event < p.data.events.len() && p.data.events[p.next_event].time == 0.0 {
            let e = p.data.events[p.next_event].clone();
            p.dispatch(&e, false, &mut step);
            p.next_event += 1;
        }
        (p, step)
    }

    /// The length of the NIS in seconds.
    pub fn length(&self) -> f32 {
        self.data.length
    }

    /// One NIS task step: the fade, then `nisUpdate` by [`TICK`].
    pub fn tick(&mut self) -> Step {
        self.fade.tick();
        self.update(TICK)
    }

    /// `nisUpdate`.
    pub fn update(&mut self, dt: f32) -> Step {
        let mut step = Step::default();
        for o in &mut self.objects {
            o.posed = false;
            o.jumped = false;
        }
        for i in 0..self.objects.len() {
            if self.objects[i].flags & omf::CURVES_DELETED != 0 {
                continue;
            }
            let Some(curves) = self.objects[i].curves.as_mut() else {
                self.objects[i].moving = false;
                continue;
            };
            let v = sample(curves, &mut self.cache, dt);
            let rot = ship_euler_to_matrix(v[3], v[4], v[5]);
            let coordsys = self.matrix.mul_mat(&rot);
            let o = &mut self.objects[i];
            let last = o.position;
            let xyz = self.matrix.mul_vec(Vector::new(-v[2], v[0], v[1]));
            o.position = add(self.position, xyz);
            o.coordsys = coordsys;
            o.posed = true;
            let delta = sub(o.position, last);
            o.jumped = delta.dist_sloppy() >= JUMP_DISTANCE;
            let sum = st(ld(delta.x.abs()) + ld(delta.y.abs()) + ld(delta.z.abs()));
            o.moving = sum > 0.0;
            o.velocity = delta;
        }
        if let Some(Some(curves)) = self.cameras.first_mut() {
            let v = sample(curves, &mut self.cache, dt);
            let xyz = self.matrix.mul_vec(Vector::new(-v[2], v[0], v[1]));
            self.camera.eye = add(xyz, self.position);
            let rot = ship_euler_to_matrix(v[3], v[4], v[5]);
            let m = self.matrix.mul_mat(&rot);
            let f = m.col(1);
            let scaled = Vector::new(
                st(ld(f.x) * ld(LOOK_SCALAR)),
                st(ld(f.y) * ld(LOOK_SCALAR)),
                st(ld(f.z) * ld(LOOK_SCALAR)),
            );
            self.camera.up = m.col(0);
            self.camera.look_at = add(scaled, self.camera.eye);
        }
        // Other cameras move too but are never looked through.
        for cam in self.cameras.iter_mut().skip(1).flatten() {
            sample(cam, &mut self.cache, dt);
        }

        if self.seeked {
            self.seeked = false;
            while self.next_event < self.data.events.len()
                && self.data.events[self.next_event].time <= self.time
            {
                let e = self.data.events[self.next_event].clone();
                self.dispatch(&e, true, &mut step);
                self.next_event += 1;
            }
        }
        let horizon = ld(dt) + ld(self.time);
        while self.next_event < self.data.events.len()
            && !(horizon < ld(self.data.events[self.next_event].time))
        {
            let e = self.data.events[self.next_event].clone();
            self.dispatch(&e, false, &mut step);
            self.next_event += 1;
        }
        self.time = st(horizon);
        if !(horizon < ld(self.data.length)) {
            if self.data.loop_point < 0.0 {
                step.finished = true;
            } else {
                let lp = self.data.loop_point;
                self.seek(lp);
            }
        }
        step
    }

    /// `nisGoToEnd`: the skip key. Seeks to one step before the end; the next step runs the
    /// skipped events (quietly) and ends the NIS.
    pub fn go_to_end(&mut self) {
        let t = st(ld(self.data.length) - ld(TICK));
        self.seek(t);
    }

    /// `nisSeek`: forward seeks advance the curves by the difference; backward seeks restart
    /// them and replay from the start.
    pub fn seek(&mut self, to: f32) {
        let back = to <= self.time;
        let delta = st(ld(to) - ld(self.time));
        let cache = &mut self.cache;
        let advance = |curves: &mut Vec<SplineCurve>, cache: &mut SplineCache| {
            for c in curves.iter_mut() {
                if back {
                    c.restart();
                    c.update(cache, to);
                } else {
                    c.update(cache, delta);
                }
            }
        };
        for o in &mut self.objects {
            if let Some(c) = o.curves.as_mut() {
                advance(c, cache);
            }
        }
        for c in self.cameras.iter_mut().flatten() {
            advance(c, cache);
        }
        if back {
            self.next_event = 0;
        }
        self.seeked = true;
        self.time = to;
    }

    /// Whether the skip key works now (`kbcbEscape`): not while the letterbox is still coming
    /// in (`scissor_fading_in`) or a camera cut is flying.
    pub fn can_skip(&self, scissor_fading_in: bool) -> bool {
        !scissor_fading_in
    }

    fn dispatch(&mut self, e: &Event, seeking: bool, step: &mut Step) {
        let obj = e.object;
        match &e.action {
            Action::Death { .. } => {
                let Some(i) = obj else { return };
                if seeking || self.objects[i].flags & omf::OBJECT_DIED != 0 {
                    return;
                }
                self.objects[i].flags |= omf::OBJECT_DIED;
            }
            Action::RemainAtEnd { .. } => {
                if let Some(i) = obj {
                    self.objects[i].flags |= omf::REMAIN_AT_END;
                }
            }
            Action::KeepMovingAtEnd => {
                if let Some(i) = obj {
                    self.objects[i].flags |= omf::KEEP_ON_MOVING;
                }
            }
            Action::CameraFov(f) => self.camera.fov = *f,
            Action::CameraClipNear(f) => self.camera.clip_near = Some(*f),
            Action::CameraClipFar(f) => self.camera.clip_far = Some(*f),
            Action::FadeSet { level, white } => {
                self.fade.dest = *level;
                self.fade.level = *level;
                self.fade.white = *white;
            }
            Action::FadeTo { level, seconds } => {
                self.fade.dest = *level;
                self.fade.rate = st((ld(*level) - ld(self.fade.level)) / ld(*seconds));
            }
            Action::StartMusic(_) => self.music_playing = true,
            Action::StopMusic(_) => self.music_playing = false,
            Action::HideUniverseToggle => self.universe_hidden ^= true,
            Action::PauseUniverseToggle => self.universe_paused ^= true,
            _ => {}
        }
        step.fired.push(Fired {
            object: obj,
            action: e.action.clone(),
            seeking,
        });
    }

    /// `nisStop`: what the stage must do with each object. The fade resets; the music the NIS
    /// started stops; the universe is unhidden.
    pub fn stop(&mut self) -> StopReport {
        let mut report = StopReport {
            stop_music: self.music_playing,
            unhide_universe: self.universe_hidden,
            ..Default::default()
        };
        for (i, o) in self.objects.iter().enumerate() {
            if o.flags & omf::OBJECT_DIED != 0 {
                continue;
            }
            if o.flags & omf::REMAIN_AT_END != 0 {
                report.remain.push((i, o.flags & omf::KEEP_ON_MOVING != 0));
            } else {
                report.delete.push(i);
            }
        }
        self.music_playing = false;
        self.universe_hidden = false;
        self.universe_paused = false;
        self.fade = BlackFade::default();
        report
    }
}

/// The end of an NIS for the stage.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StopReport {
    /// Objects that stay (`remainAtEnd`): index and whether they keep moving.
    pub remain: Vec<(usize, bool)>,
    /// Objects deleted from the universe.
    pub delete: Vec<usize>,
    pub stop_music: bool,
    pub unhide_universe: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{CameraPath, Category, Curves, ObjectPath};
    use hwc_math::spline::Tcb;

    /// A path along x from 0 to 100 over 2 s (two end-condition keys at each end).
    fn curves() -> Curves {
        let x = vec![0.0, 0.0, 0.0, 50.0, 100.0, 100.0, 100.0];
        let zero = vec![0.0; 7];
        Curves {
            samples: 6,
            times: vec![0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 0.0],
            params: vec![Tcb::default(); 6],
            channels: [x, zero.clone(), zero.clone(), zero.clone(), zero.clone(), zero],
        }
    }

    fn data(events: Vec<Event>) -> NisData {
        NisData {
            length: 2.0,
            loop_point: -1.0,
            objects: vec![ObjectPath {
                name: "sRecon(1)".into(),
                type_name: "sRecon".into(),
                instance: 1,
                category: Category::Ship(0),
                centre: false,
                parent: None,
                time_offset: -1.0,
                curves: curves(),
            }],
            cameras: vec![CameraPath {
                time_offset: -1.0,
                curves: curves(),
            }],
            events,
            scissor_fade_in: None,
            looky_object: None,
            ignored: Vec::new(),
        }
    }

    fn ev(time: f32, order: u32, action: Action) -> Event {
        Event {
            time,
            object: Some(0),
            order,
            action,
        }
    }

    #[test]
    fn events_run_when_due_and_the_nis_ends_at_its_length() {
        let d = data(vec![
            ev(0.0, 1, Action::Show),
            ev(0.1, 2, Action::Hide),
            ev(0.125, 3, Action::Show),
            ev(1.0, 4, Action::FleetSpeechEvent { event: 1, variable: 0 }),
        ]);
        let (mut p, s) = NisPlayer::start(d, Vector::ZERO, Matrix::IDENTITY, Vector::ZERO);
        assert_eq!(s.fired.len(), 1);
        // Step 1 covers events up to 1/16 s, step 2 up to 1/8 s (inclusive).
        assert!(p.tick().fired.is_empty());
        let f = p.tick().fired;
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].action, Action::Hide);
        let mut steps = 2;
        let mut speech = 0;
        loop {
            let s = p.tick();
            steps += 1;
            speech += s.fired.iter().filter(|f| matches!(f.action, Action::FleetSpeechEvent { .. })).count();
            if s.finished {
                break;
            }
            assert!(steps < 100);
        }
        assert_eq!(steps, 32);
        assert_eq!(speech, 1);
        // The object follows the swizzled x curve: NIS x is world y.
        let o = &p.objects[0];
        assert!((o.position.y - 100.0).abs() < 1e-3, "{:?}", o.position);
        assert!(o.position.x.abs() < 1e-6 && o.position.z.abs() < 1e-6);
        // The camera looks 10000 along its forward axis.
        let c = p.camera;
        let d = Vector::new(c.look_at.x - c.eye.x, c.look_at.y - c.eye.y, c.look_at.z - c.eye.z);
        assert!((d.dist_sloppy() - LOOK_SCALAR).abs() < LOOK_SCALAR * 0.1);
    }

    #[test]
    fn skipping_runs_the_rest_quietly_then_ends() {
        let d = data(vec![
            ev(0.5, 1, Action::FleetSpeechEvent { event: 1, variable: 0 }),
            ev(1.5, 2, Action::Death { kind: 0 }),
            ev(1.9, 3, Action::RemainAtEnd { team: None }),
        ]);
        let (mut p, _) = NisPlayer::start(d, Vector::ZERO, Matrix::IDENTITY, Vector::ZERO);
        p.tick();
        p.go_to_end();
        let s = p.tick();
        assert!(s.finished);
        // Deaths are not done while seeking (and not reported).
        assert_eq!(s.fired.len(), 2);
        assert!(s.fired.iter().all(|f| f.seeking));
        assert_eq!(p.objects[0].flags & omf::OBJECT_DIED, 0);
        assert_ne!(p.objects[0].flags & omf::REMAIN_AT_END, 0);
        let r = p.stop();
        assert_eq!(r.remain, vec![(0, false)]);
    }

    #[test]
    fn fade_moves_in_steps_and_snaps() {
        let d = data(vec![
            ev(0.0, 1, Action::FadeSet { level: 1.0, white: false }),
            ev(0.0, 2, Action::FadeTo { level: 0.0, seconds: 1.0 }),
        ]);
        let (mut p, _) = NisPlayer::start(d, Vector::ZERO, Matrix::IDENTITY, Vector::ZERO);
        assert_eq!(p.fade.level, 1.0);
        assert_eq!(p.fade.rate, -1.0);
        p.tick();
        assert_eq!(p.fade.level, 0.9375);
        for _ in 0..14 {
            p.tick();
        }
        // Within one step of the end it snaps.
        assert_eq!(p.fade.level, 0.0625);
        p.tick();
        assert_eq!(p.fade.level, 0.0);
        assert_eq!(p.fade.rate, 0.0);
    }
}
