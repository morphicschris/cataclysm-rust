//! Sensors-manager pings (`Ping.c`, Cataclysm's version): expanding circles that mark
//! objectives (anomaly pings), proximity-sensor alerts, newly built ships and battles, with a
//! sound as each one starts to expand and a legend in the tactical overlay.
//!
//! A ping grows from its minimum to its full size over its duration, rests for its pause and
//! starts again. The list is evaluated twice a second (`pingUpdateTask`): each ping's own test,
//! at most once per evaluation period, says whether it has expired. Pings are drawn only in
//! the sensors manager (`smSceneRender` → `pingListDraw`), farthest first.

use hwc_formats::script::ScriptFile;
use hwc_math::Vector;
use hwc_sim::object::BulletId;
use hwc_sim::{ShipId, Universe};

/// `PNG_TaskPeriod`: the list is evaluated this often.
pub const TASK_PERIOD: f32 = 0.5;

/// The TO-legend kind of a ping (`PTOM_*`).
pub mod mask {
    pub const PROXIMITY: u32 = 0x01;
    pub const NEW_SHIPS: u32 = 0x02;
    pub const ANOMALY: u32 = 0x04;
    pub const BATTLE: u32 = 0x08;
    pub const HYPERSPACE: u32 = 0x10;
}

/// The sound each kind plays as it starts to expand (`soundEventPlay` ids).
pub mod sound {
    pub const PROXIMITY: u32 = 0x131;
    pub const NEW_SHIPS: u32 = 0x132;
    pub const ANOMALY: u32 = 0x12c;
    pub const BATTLE: u32 = 0x133;
    pub const HYPERSPACE: u32 = 0x134;
}

/// The label of the anomaly ping on a big-gun shell (`FUN_00487120`).
pub const BIG_GUN_PING: &str = "BulletBigGun";
/// The label of the anomaly ping on a level derelict with attribute 0x20
/// (`scriptSetDerelictCB`).
pub const DERELICT_PING: &str = "Anomaly";
/// Bullet type of a big-gun shell.
pub const BIG_GUN_BULLET: u32 = 0x12;

pub type Rgb = [u8; 4];

/// `ping.script`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Tweaks {
    pub anomaly: KindTweaks,
    pub battle: KindTweaks,
    pub new_ship: KindTweaks,
    /// `pingTOLingerTime`: a legend line stays this long after its last ping.
    pub to_linger: f32,
    /// `PING_TONSegments`.
    pub to_segments: i32,
}

/// One kind's look (`Anomaly*`, `Battle*`, `NewShip*`). `min_screen` is in pixels.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KindTweaks {
    pub color: Rgb,
    pub size: f32,
    pub min_size: f32,
    pub duration: f32,
    pub pause: f32,
    pub evaluate: f32,
    pub min_screen: f32,
}

impl Default for Tweaks {
    /// The executable's values before `ping.script` is read (0x883780..).
    fn default() -> Self {
        Tweaks {
            anomaly: KindTweaks {
                color: [255, 255, 255, 255],
                size: 1000.0,
                min_size: 100.0,
                duration: 1.0,
                pause: 0.25,
                evaluate: 2.0,
                min_screen: 0.0,
            },
            battle: KindTweaks {
                color: [255, 0, 0, 255],
                size: 1000.0,
                min_size: 100.0,
                duration: 0.5,
                pause: 0.15,
                evaluate: 2.0,
                min_screen: 0.0,
            },
            new_ship: KindTweaks {
                color: [255, 0, 0, 255],
                size: 1000.0,
                min_size: 100.0,
                duration: 0.5,
                pause: 0.15,
                evaluate: 2.0,
                min_screen: 0.0,
            },
            to_linger: 2.0,
            to_segments: 12,
        }
    }
}

impl Tweaks {
    pub fn from_script(s: Option<&ScriptFile>) -> Tweaks {
        let mut t = Tweaks::default();
        let Some(s) = s else { return t };
        let f = |k: &str, d: f32| s.get(k).and_then(|e| e.f32()).unwrap_or(d);
        let rgb = |k: &str, d: Rgb| {
            s.get(k)
                .and_then(|e| {
                    let v: Vec<i32> = e.values.iter().filter_map(|v| v.as_i32()).collect();
                    (v.len() >= 3).then(|| [v[0] as u8, v[1] as u8, v[2] as u8, 255])
                })
                .unwrap_or(d)
        };
        let kind = |p: &str, d: KindTweaks| KindTweaks {
            color: rgb(&format!("{p}Color"), d.color),
            size: f(&format!("{p}Size"), d.size),
            min_size: f(&format!("{p}MinSize"), d.min_size),
            duration: f(&format!("{p}PingDuration"), d.duration),
            pause: f(&format!("{p}InterPingPause"), d.pause),
            evaluate: f(&format!("{p}EvaluatePeriod"), d.evaluate),
            min_screen: f(&format!("{p}MinScreenSize"), d.min_screen),
        };
        t.anomaly = kind("Anomaly", t.anomaly);
        t.battle = kind("Battle", t.battle);
        t.new_ship = kind("NewShip", t.new_ship);
        t.to_linger = f("pingTOLingerTime", t.to_linger);
        t
    }
}

/// The `pingBob*` blob properties of the game's `ping.script`.
pub fn blob_properties(source: &dyn hwc_ui::AssetSource) -> hwc_sim::tunables::BlobProperties {
    struct Src<'a>(&'a dyn hwc_ui::AssetSource);
    impl hwc_sim::statics::DataSource for Src<'_> {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.0.read(path)
        }
        fn exists(&self, path: &str) -> bool {
            self.0.read(path).is_some()
        }
    }
    hwc_sim::battle_ping::ping_blob_properties(Some(&Src(source)))
}

/// What a ping is centred on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Owner {
    Ship(ShipId),
    Bullet(BulletId),
}

/// The ping's expiry test (`pingeval`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Eval {
    /// `pingAnomalyPingTimeout`: KAS and objective pings. A ping on ships follows their centre
    /// and goes when the last of them dies; an object ping stays where its object died.
    Anomaly {
        name: String,
        ships: Vec<ShipId>,
        object: Option<Owner>,
    },
    /// `FUN_005b9ef0`: while the sensor has found something.
    Proximity(ShipId),
    /// New ships: expires at its first evaluation.
    NewShip,
    /// `pingBattlePingEvaluate`: a battle over the ships of its sub-blob. It expires at its
    /// first evaluation (the battle chatter is the simulation's), or once the last of its
    /// ships has died.
    Battle { ships: Vec<ShipId> },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Ping {
    pub color: Rgb,
    pub size: f32,
    pub min_size: f32,
    /// Smallest on-screen radius, pixels.
    pub min_screen: f32,
    pub centre: Vector,
    pub owner: Option<Owner>,
    pub created: f32,
    pub duration: f32,
    pub pause: f32,
    pub evaluate_period: f32,
    pub last_evaluate: f32,
    pub eval: Eval,
    pub mask: u32,
    camera_dist_sq: f32,
}

impl Ping {
    /// `pingCreate`'s defaults.
    fn new(centre: Vector, owner: Option<Owner>, eval: Eval, now: f32) -> Ping {
        Ping {
            color: [255, 255, 255, 255],
            size: 1.0,
            min_size: 0.0,
            min_screen: 0.0,
            centre,
            owner,
            created: now,
            duration: 1.0,
            pause: 0.0,
            evaluate_period: TASK_PERIOD,
            last_evaluate: now,
            eval,
            mask: 0,
            camera_dist_sq: 0.0,
        }
    }

    fn apply(&mut self, k: &KindTweaks, mask: u32) {
        self.color = k.color;
        self.size = k.size;
        self.min_size = k.min_size;
        self.duration = k.duration;
        self.pause = k.pause;
        self.evaluate_period = k.evaluate;
        self.min_screen = k.min_screen;
        self.mask = mask;
    }

    /// The expanding size at `now`, when the ping is expanding (not in its pause).
    pub fn pulse(&self, now: f32) -> Option<f32> {
        let cycle = self.duration + self.pause;
        let m = (now - self.created).rem_euclid(cycle.max(1e-6));
        (m <= self.duration)
            .then(|| (self.size - self.min_size) * m / self.duration + self.min_size)
    }
}

/// One circle to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Circle {
    pub x: i32,
    pub y: i32,
    pub r: i32,
    pub segments: i32,
    pub color: Rgb,
}

/// The legend lines of the kinds, in order (`pingTOList`): string, kind.
pub const LEGEND: [(&str, u32); 5] = [
    ("strPingTO0", mask::PROXIMITY),
    ("strPingTO1", mask::NEW_SHIPS),
    ("strPingTO2", mask::ANOMALY),
    ("strPingTO3", mask::BATTLE),
    ("strPingTO4", mask::HYPERSPACE),
];

/// The ping list and its drawing state.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pings {
    pub list: Vec<Ping>,
    pub tweaks: Tweaks,
    /// `pingListDraw`'s statics: the last size a kind's sound played at.
    last_sound_size: [f32; 5],
    /// Legend: when each line last had a ping.
    legend_drawn: [f32; 5],
    /// Proximity sensors' blip colour, as the last sensor ping set it (legend colour).
    pub proximity_color: Rgb,
    /// When the evaluation task last ran.
    last_task: Option<f32>,
}

fn kind_index(mask: u32) -> Option<usize> {
    LEGEND.iter().position(|&(_, m)| m == mask)
}

impl Pings {
    pub fn new(tweaks: Tweaks) -> Pings {
        Pings {
            list: Vec::new(),
            tweaks,
            last_sound_size: [f32::MAX; 5],
            legend_drawn: [f32::MIN; 5],
            proximity_color: [255, 255, 255, 255],
            last_task: None,
        }
    }

    /// `pingAllPingsDelete`.
    #[allow(dead_code)] // each mission starts a new game, with an empty list
    pub fn clear(&mut self) {
        self.list.clear();
        self.last_task = None;
    }

    /// `pingAnomalySelectionPingAdd`: a ping on the centre of some ships.
    pub fn anomaly_ships(&mut self, u: &Universe, name: &str, ships: &[ShipId], now: f32) {
        let centre = selection_centre(u, ships).unwrap_or(Vector::ZERO);
        let eval = Eval::Anomaly {
            name: name.to_string(),
            ships: ships.to_vec(),
            object: None,
        };
        let mut p = Ping::new(centre, None, eval, now);
        p.apply(&self.tweaks.anomaly, mask::ANOMALY);
        self.list.push(p);
    }

    /// `pingAnomalyObjectPingAdd`: a ping on one object.
    pub fn anomaly_object(&mut self, name: &str, owner: Owner, at: Vector, now: f32) {
        let eval = Eval::Anomaly {
            name: name.to_string(),
            ships: Vec::new(),
            object: Some(owner),
        };
        let mut p = Ping::new(at, Some(owner), eval, now);
        p.apply(&self.tweaks.anomaly, mask::ANOMALY);
        self.list.push(p);
    }

    /// `pingAnomalyPositionPingAdd`: a ping on a point.
    pub fn anomaly_point(&mut self, name: &str, at: Vector, now: f32) {
        let eval = Eval::Anomaly {
            name: name.to_string(),
            ships: Vec::new(),
            object: None,
        };
        let mut p = Ping::new(at, None, eval, now);
        p.apply(&self.tweaks.anomaly, mask::ANOMALY);
        self.list.push(p);
    }

    /// `pingAnomalyPingRemove`: every anomaly ping whose label starts with `name` (ignoring
    /// case); the count removed.
    pub fn anomaly_remove(&mut self, name: &str) -> usize {
        let before = self.list.len();
        let n = name.len();
        self.list.retain(|p| match &p.eval {
            Eval::Anomaly { name: label, .. } => {
                let l = label.as_bytes();
                !(l.len() >= n && l[..n].eq_ignore_ascii_case(name.as_bytes()))
            }
            _ => true,
        });
        before - self.list.len()
    }

    /// `pingNewShipPingCreate`: at the factory that finished its last ship of a type.
    pub fn new_ship(&mut self, at: Vector, now: f32) {
        let mut p = Ping::new(at, None, Eval::NewShip, now);
        p.apply(&self.tweaks.new_ship, mask::NEW_SHIPS);
        self.list.push(p);
    }

    /// A battle ping of `radius` around `centre` over `ships` (`pingAttackPingsCreate`).
    pub fn battle(&mut self, centre: Vector, radius: f32, ships: Vec<ShipId>, now: f32) {
        let mut p = Ping::new(centre, None, Eval::Battle { ships }, now);
        p.apply(&self.tweaks.battle, mask::BATTLE);
        p.size = radius;
        self.list.push(p);
    }

    /// `pingBattlePingsCreate` on a battle-ping frame: the simulation's battle sub-blobs
    /// become pings, unless a battle ping is still in the list (then none are made).
    pub fn battles(&mut self, battles: Vec<hwc_sim::battle_ping::BattlePing>, now: f32) {
        if self
            .list
            .iter()
            .any(|p| matches!(p.eval, Eval::Battle { .. }))
        {
            return;
        }
        for b in battles {
            self.battle(b.centre, b.radius, b.ships, now);
        }
    }

    /// The mission script's ping calls (`kasfPingAddSingleShip`, `kasfPingAddShips`,
    /// `kasfPingAddPoint`, `kasfPingRemove`), in the order it made them.
    pub fn kas_orders(&mut self, u: &Universe, orders: Vec<hwc_sim::mission::PingOrder>, now: f32) {
        use hwc_sim::mission::PingOrder;
        for o in orders {
            match o {
                PingOrder::Ship { label, ship } => {
                    // `pingAnomalyObjectPingAdd` on the ship, wherever it is now.
                    let Some(at) = u.ship(ship).map(|s| s.posinfo.position) else {
                        continue;
                    };
                    self.anomaly_object(&label, Owner::Ship(ship), at, now);
                }
                PingOrder::Ships { label, ships } => self.anomaly_ships(u, &label, &ships, now),
                PingOrder::Point { label, at } => self.anomaly_point(&label, at, now),
                PingOrder::Remove { label } => {
                    self.anomaly_remove(&label);
                }
            }
        }
    }

    /// `ProximitySensorHouseKeep`: the local player's sensor found something.
    pub fn proximity(
        &mut self,
        sensor: ShipId,
        at: Vector,
        radius: f32,
        blink_rate: f32,
        color: Rgb,
        now: f32,
    ) {
        let mut p = Ping::new(at, Some(Owner::Ship(sensor)), Eval::Proximity(sensor), now);
        p.color = color;
        p.size = radius;
        p.min_screen = 2.0;
        p.mask = mask::PROXIMITY;
        p.pause = 0.25;
        p.duration = 1.0 / blink_rate;
        self.proximity_color = color;
        self.list.push(p);
    }

    /// `pingRemovePingByOwner`: the first ping on this object.
    pub fn remove_by_owner(&mut self, owner: Owner) {
        if let Some(i) = self.list.iter().position(|p| p.owner == Some(owner)) {
            self.list.remove(i);
        }
    }

    /// `pingObjectDied`: pings centred on the object stay where it died; the ping tests drop
    /// their references (and the pings that depended on it).
    pub fn object_died(&mut self, obj: Owner, at: Vector) {
        self.list.retain_mut(|p| {
            if p.owner == Some(obj) {
                p.centre = at;
                p.owner = None;
            }
            match &mut p.eval {
                Eval::Anomaly { ships, .. } => {
                    // The object's own ping lost its owner above, so it stays.
                    if ships.is_empty() {
                        return true;
                    }
                    if let Owner::Ship(id) = obj {
                        ships.retain(|&s| s != id);
                    }
                    !ships.is_empty()
                }
                Eval::Proximity(s) => Owner::Ship(*s) != obj,
                Eval::Battle { ships } => {
                    if let Owner::Ship(id) = obj {
                        if let Some(i) = ships.iter().position(|&s| s == id) {
                            ships.swap_remove(i);
                        }
                    }
                    !ships.is_empty()
                }
                Eval::NewShip => true,
            }
        });
    }

    /// `pingUpdateTask`: runs every [`TASK_PERIOD`] of game time.
    pub fn update(&mut self, u: &Universe, now: f32, sensor_found: impl Fn(ShipId) -> bool) {
        match self.last_task {
            Some(t) if now - t < TASK_PERIOD && now >= t => return,
            _ => self.last_task = Some(now),
        }
        self.list.retain_mut(|p| {
            if now - p.last_evaluate < p.evaluate_period {
                return true;
            }
            p.last_evaluate = now;
            match &p.eval {
                Eval::Anomaly { ships, .. } => {
                    if let Some(c) = selection_centre(u, ships) {
                        p.centre = c;
                    }
                    true
                }
                Eval::Proximity(s) => sensor_found(*s),
                Eval::NewShip | Eval::Battle { .. } => false,
            }
        });
    }

    /// `pingListDraw`'s circles, farthest first, and the sound to play. `centre_of` gives an
    /// owner's position, `project` a sphere's screen centre and radius (pixels), `hidden` the
    /// fog test; `width` is the viewport width (GL to pixels).
    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        now: f32,
        eye: Vector,
        centre_of: impl Fn(Owner) -> Option<Vector>,
        project: impl Fn(Vector, f32) -> Option<(f32, f32, f32)>,
        hidden: impl Fn(Vector) -> bool,
        zooming: bool,
    ) -> (Vec<Circle>, Option<u32>, u32) {
        let mut masks = 0;
        for p in &mut self.list {
            if let Some(c) = p.owner.and_then(&centre_of) {
                p.centre = c;
            }
            let d = Vector::new(eye.x - p.centre.x, eye.y - p.centre.y, eye.z - p.centre.z);
            p.camera_dist_sq = d.x * d.x + d.y * d.y + d.z * d.z;
            masks |= p.mask;
        }
        self.list
            .sort_by(|a, b| b.camera_dist_sq.total_cmp(&a.camera_dist_sq));
        let mut out = Vec::new();
        let mut played = None;
        for p in &self.list {
            let Some(size) = p.pulse(now) else { continue };
            let full = p.size.max(p.min_size);
            let Some((x, y, r)) = project(p.centre, full) else {
                continue;
            };
            if r <= 0.0 || (p.mask != mask::ANOMALY && hidden(p.centre)) {
                continue;
            }
            let r = r.max(p.min_screen) * size / full;
            out.push(Circle {
                x: x.round() as i32,
                y: y.round() as i32,
                r: r.round() as i32,
                segments: circle_segments(r),
                color: p.color,
            });
            if !zooming && played.is_none() {
                if let Some(k) = kind_index(p.mask) {
                    if size <= self.last_sound_size[k] {
                        self.last_sound_size[k] = size;
                        played = Some(
                            [
                                sound::PROXIMITY,
                                sound::NEW_SHIPS,
                                sound::ANOMALY,
                                sound::BATTLE,
                                sound::HYPERSPACE,
                            ][k],
                        );
                    }
                }
            }
        }
        (out, played, masks)
    }

    /// The legend lines to show (with the tactical overlay): (string name, colour).
    pub fn legend(&mut self, now: f32, masks: u32) -> Vec<(&'static str, Rgb)> {
        let mut out = Vec::new();
        for (i, &(name, m)) in LEGEND.iter().enumerate() {
            if masks & m != 0 {
                self.legend_drawn[i] = now;
            }
            if now - self.legend_drawn[i] <= self.tweaks.to_linger {
                let c = match m {
                    mask::PROXIMITY => self.proximity_color,
                    mask::NEW_SHIPS => self.tweaks.new_ship.color,
                    mask::ANOMALY => self.tweaks.anomaly.color,
                    mask::BATTLE => self.tweaks.battle.color,
                    _ => [255, 255, 255, 255],
                };
                out.push((name, c));
            }
        }
        out
    }
}

/// `pieCircleSegmentsCompute` with the pie plate's levels (radius in pixels).
pub fn circle_segments(r: f32) -> i32 {
    const LEVELS: [(f32, i32); 7] = [
        (200.0, 32),
        (80.0, 20),
        (32.0, 16),
        (10.0, 12),
        (4.0, 8),
        (2.0, 6),
        (0.0, 4),
    ];
    LEVELS
        .iter()
        .find(|&&(min, _)| r > min)
        .map_or(32, |&(_, n)| n)
}

/// `selCentrePointComputeGeneral`: the middle of the ships' bounding box.
pub fn selection_centre(u: &Universe, ships: &[ShipId]) -> Option<Vector> {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let mut any = false;
    for &id in ships {
        let Some(s) = u.ship(id) else { continue };
        let p = s.posinfo.position;
        for (i, v) in [p.x, p.y, p.z].into_iter().enumerate() {
            lo[i] = lo[i].min(v);
            hi[i] = hi[i].max(v);
        }
        any = true;
    }
    any.then(|| {
        Vector::new(
            (lo[0] + hi[0]) * 0.5,
            (lo[1] + hi[1]) * 0.5,
            (lo[2] + hi[2]) * 0.5,
        )
    })
}

/// `primOvalArcOutline2` over a whole turn: a polygon whose first vertex is straight above
/// the centre.
pub fn draw_circle(canvas: &mut hwc_ui::canvas::Canvas, c: &Circle) {
    let n = c.segments.max(3);
    let r = c.r as f32;
    let pt = |k: i32| {
        let a = std::f32::consts::TAU * k as f32 / n as f32;
        (
            (c.x as f32 + a.sin() * r).round() as i32,
            (c.y as f32 - a.cos() * r).round() as i32,
        )
    };
    let mut last = pt(0);
    for k in 1..=n {
        let p = pt(k);
        canvas.line(last.0, last.1, p.0, p.1, c.color);
        last = p;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pings() -> Pings {
        let s = ScriptFile::parse(
            "AnomalyColor 255,255,0\nAnomalySize 3000\nAnomalyMinSize 500\nAnomalyPingDuration 2\n\
             AnomalyInterPingPause 1\nAnomalyEvaluatePeriod 0.75\nAnomalyMinScreenSize 10\n\
             NewShipColor 25,255,25\nNewShipSize 2500\nNewShipMinSize 1000\nNewShipPingDuration 2\n\
             NewShipInterPingPause 2\nNewShipEvaluatePeriod 2\npingTOLingerTime 2\n",
        );
        Pings::new(Tweaks::from_script(Some(&s)))
    }

    #[test]
    fn script_tweaks_are_read() {
        let p = pings();
        assert_eq!(p.tweaks.anomaly.color, [255, 255, 0, 255]);
        assert_eq!(p.tweaks.anomaly.size, 3000.0);
        assert_eq!(p.tweaks.anomaly.min_screen, 10.0);
        assert_eq!(p.tweaks.new_ship.pause, 2.0);
        // Not in the script: the executable's defaults.
        assert_eq!(p.tweaks.battle.duration, 0.5);
    }

    #[test]
    fn anomaly_pings_pulse_and_go_by_label_prefix() {
        let mut p = pings();
        p.anomaly_point("Objective1", Vector::ZERO, 10.0);
        p.anomaly_point("objective2", Vector::ZERO, 10.0);
        p.anomaly_point("Other", Vector::ZERO, 10.0);
        let a = &p.list[0];
        // Expands from 500 to 3000 over 2 s, then rests 1 s.
        assert_eq!(a.pulse(10.0), Some(500.0));
        assert_eq!(a.pulse(11.0), Some(1750.0));
        assert_eq!(a.pulse(12.5), None);
        assert_eq!(a.pulse(13.0), Some(500.0));
        assert_eq!(p.anomaly_remove("OBJECTIVE"), 2);
        assert_eq!(p.list.len(), 1);
    }

    #[test]
    fn object_pings_outlive_their_object_and_sound_once_per_pulse() {
        let mut p = pings();
        let ship = Owner::Ship(ShipId(4));
        p.anomaly_object("Target", ship, Vector::new(1.0, 0.0, 0.0), 0.0);
        p.proximity(ShipId(5), Vector::ZERO, 2000.0, 2.0, [0, 255, 0, 255], 0.0);
        p.object_died(ship, Vector::new(5.0, 0.0, 0.0));
        p.object_died(Owner::Ship(ShipId(5)), Vector::ZERO);
        assert_eq!(p.list.len(), 1);
        assert_eq!(p.list[0].owner, None);
        assert_eq!(p.list[0].centre, Vector::new(5.0, 0.0, 0.0));
        let project = |_: Vector, r: f32| Some((100.0, 100.0, r / 100.0));
        let (c, s, m) = p.frame(
            0.0,
            Vector::new(0.0, 0.0, 1000.0),
            |_| None,
            project,
            |_| true,
            false,
        );
        // Anomaly pings show in the fog; 500 m of 3000 m is a sixth of the full 30 px.
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].r, 5);
        assert_eq!(s, Some(sound::ANOMALY));
        assert_eq!(m, mask::ANOMALY);
        // Growing: no sound until it starts again.
        let (_, s, _) = p.frame(1.0, Vector::ZERO, |_| None, project, |_| true, false);
        assert_eq!(s, None);
        let (_, s, _) = p.frame(3.0, Vector::ZERO, |_| None, project, |_| true, false);
        assert_eq!(s, Some(sound::ANOMALY));
        assert_eq!(p.legend(3.0, m), vec![("strPingTO2", [255, 255, 0, 255])]);
        assert!(p.legend(5.5, 0).is_empty());
    }

    #[test]
    fn new_ship_pings_last_one_evaluation_and_hide_in_fog() {
        let mut p = pings();
        p.new_ship(Vector::ZERO, 0.0);
        let project = |_: Vector, r: f32| Some((0.0, 0.0, r / 10.0));
        let (c, ..) = p.frame(0.0, Vector::ZERO, |_| None, project, |_| true, false);
        assert!(c.is_empty());
        let (c, ..) = p.frame(0.0, Vector::ZERO, |_| None, project, |_| false, false);
        assert_eq!(c[0].color, [25, 255, 25, 255]);
        assert_eq!(circle_segments(100.0), 20);
        assert_eq!(circle_segments(0.0), 32);
    }

    /// A universe with no game data: no ships.
    fn empty_universe() -> Universe {
        struct NoData;
        impl hwc_sim::statics::DataSource for NoData {
            fn read(&self, _: &str) -> Option<Vec<u8>> {
                None
            }
            fn exists(&self, _: &str) -> bool {
                false
            }
        }
        let statics = hwc_sim::statics::StaticWorld::load_from(
            &NoData,
            hwc_sim::statics::LoadOptions::default(),
        );
        Universe::new(
            std::sync::Arc::new(statics),
            hwc_sim::SimTweaks::default(),
            &[
                hwc_sim::statics::ShipRace::Somtaaw,
                hwc_sim::statics::ShipRace::Taiidan,
            ],
        )
    }

    #[test]
    fn kas_ping_orders_become_anomaly_pings() {
        use hwc_sim::mission::PingOrder;
        let u = empty_universe();
        let mut p = pings();
        let at = Vector::new(1000.0, 2000.0, 3000.0);
        p.kas_orders(
            &u,
            vec![
                PingOrder::Point {
                    label: "FleetIntel1".into(),
                    at,
                },
                PingOrder::Point {
                    label: "FleetIntel2".into(),
                    at,
                },
                // A ship that is gone makes no ping.
                PingOrder::Ship {
                    label: "Gone".into(),
                    ship: ShipId(77),
                },
                PingOrder::Remove {
                    label: "fleetintel2".into(),
                },
            ],
            5.0,
        );
        assert_eq!(p.list.len(), 1);
        let q = &p.list[0];
        assert_eq!(q.mask, mask::ANOMALY);
        assert_eq!(q.centre, at);
        assert_eq!(q.color, [255, 255, 0, 255]);
        assert!(matches!(&q.eval, Eval::Anomaly { name, .. } if name == "FleetIntel1"));
        // Anomaly pings show whatever the fog hides.
        let project = |_: Vector, r: f32| Some((10.0, 10.0, r / 100.0));
        let (c, ..) = p.frame(5.0, Vector::ZERO, |_| None, project, |_| true, false);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn battle_pings_come_one_batch_at_a_time_and_go_with_their_ships() {
        use hwc_sim::battle_ping::BattlePing;
        let u = empty_universe();
        let mut p = pings();
        let battle = |x: f32, ships: Vec<ShipId>| BattlePing {
            centre: Vector::new(x, 0.0, 0.0),
            radius: 4000.0,
            ships,
        };
        p.battles(
            vec![
                battle(0.0, vec![ShipId(1), ShipId(2)]),
                battle(9000.0, vec![ShipId(3)]),
            ],
            0.0,
        );
        assert_eq!(p.list.len(), 2);
        assert!(p
            .list
            .iter()
            .all(|q| q.mask == mask::BATTLE && q.size == 4000.0));
        // While a battle ping is up no new ones are made.
        p.battles(vec![battle(500.0, vec![ShipId(4)])], 0.1);
        assert_eq!(p.list.len(), 2);
        // The last ship of a battle dying ends its ping.
        p.object_died(Owner::Ship(ShipId(3)), Vector::ZERO);
        assert_eq!(p.list.len(), 1);
        p.object_died(Owner::Ship(ShipId(1)), Vector::ZERO);
        assert_eq!(p.list.len(), 1);
        // Battle pings hide in the fog.
        let project = |_: Vector, r: f32| Some((10.0, 10.0, r / 100.0));
        let (c, ..) = p.frame(0.0, Vector::ZERO, |_| None, project, |_| true, false);
        assert!(c.is_empty());
        // The first evaluation ends it (the battle tweaks' evaluate period is 2 s).
        p.update(&u, 0.5, |_| false);
        assert_eq!(p.list.len(), 1);
        p.update(&u, 2.5, |_| false);
        assert!(p.list.is_empty());
    }
}
