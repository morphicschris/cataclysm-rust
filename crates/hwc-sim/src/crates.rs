//! Crates (`MG_CratesEnabled`, `Crate.c`: `cratesUpdate` 0x45d3b0, `cratesGetBestCrateLocation`
//! 0x45ce60, `cratesIsLocationGood` 0x45cd90, `cratesPlayerFoundACrate` 0x45c2f0,
//! `cratesRewardPlayer` 0x45c460, tunables from `Crates.script`): every so often a crate
//! appears between the players; the first ship to come near it gets its player a capital
//! ship, a group of ships, a technology or resources. Unclaimed crates expire.
//!
//! In the original a crate is a derelict and the ships near it are found through its
//! collision blob; the port keeps the crates in [`crate::multiplayer::MpState`] and looks at
//! the ships in ship-list order. Placement keeps away from every ship (the original tests
//! the objects of each blob around the point).

use hwc_math::fpu::{ld, st};
use hwc_math::random::RAN_GAME;
use hwc_math::Vector;

use crate::multiplayer::mg;
use crate::object::sof;
use crate::statics::{DataSource, ShipRace, ShipType};
use crate::universe::Universe;

/// `Crates.script` (keyword table 0x86e6d8) with the compiled defaults.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CrateTweaks {
    /// `CRATES_DoWeNeedToAddCratesCheckTime`: seconds between chances of a new crate.
    pub add_check_time: f32,
    /// `CRATES_ArePlayersNearUpdateMask` / `Frame`.
    pub near_update_mask: u32,
    pub near_update_frame: u32,
    /// `CRATES_PlayerNearRadius`: a ship this close on each axis claims the crate.
    pub player_near_radius: f32,
    /// `CRATES_MaximumCratesInTheWorldPerPlayer`.
    pub max_per_player: f32,
    /// `CRATES_GetResearch`.
    pub get_research: f32,
    /// `CRATES_RandomLowDistance` / `HighDistance`: how far from the players' centre.
    pub random_low_distance: f32,
    pub random_high_distance: f32,
    /// `CRATES_AddACrateProb`: a crate is added when a random number is above this.
    pub add_prob: f32,
    /// `CRATES_MAX_RUS_GIVEN` / `MIN`.
    pub max_rus: u32,
    pub min_rus: u32,
    /// `CRATES_GetMotherShipCarrierIfDontHaveOne` (read, not used by Cataclysm).
    pub mothership_carrier: f32,
    /// `CRATES_GetAShip`.
    pub get_ship: f32,
    /// `CRATE_EXPIRY_TIME`: seconds before an unclaimed crate goes.
    pub expiry_time: f32,
}

impl Default for CrateTweaks {
    fn default() -> Self {
        Self {
            add_check_time: 380.0,
            near_update_mask: 63,
            near_update_frame: 52,
            player_near_radius: 2500.0,
            max_per_player: 1.0,
            get_research: 0.25,
            random_low_distance: 7000.0,
            random_high_distance: 20000.0,
            add_prob: 0.15,
            max_rus: 10000,
            min_rus: 500,
            mothership_carrier: 0.8,
            get_ship: 0.5,
            expiry_time: 300.0,
        }
    }
}

impl CrateTweaks {
    /// `crateInit`: the script over the defaults.
    pub fn load(src: &dyn DataSource) -> Self {
        let mut t = Self::default();
        let Some(lines) = crate::level::read_lines(src, "Crates.script") else { return t };
        for (k, v) in lines {
            let f = || crate::statics::cscan::scan_f32(&v);
            let i = || crate::statics::cscan::scan_i32(&v).map(|x| x as u32);
            match k.as_str() {
                "CRATES_DoWeNeedToAddCratesCheckTime" => t.add_check_time = f().unwrap_or(t.add_check_time),
                "CRATES_ArePlayersNearUpdateMask" => t.near_update_mask = i().unwrap_or(t.near_update_mask),
                "CRATES_ArePlayersNearUpdateFrame" => t.near_update_frame = i().unwrap_or(t.near_update_frame),
                "CRATES_PlayerNearRadius" => t.player_near_radius = f().unwrap_or(t.player_near_radius),
                "CRATES_MaximumCratesInTheWorldPerPlayer" => t.max_per_player = f().unwrap_or(t.max_per_player),
                "CRATES_GetResearch" => t.get_research = f().unwrap_or(t.get_research),
                "CRATES_RandomLowDistance" => t.random_low_distance = f().unwrap_or(t.random_low_distance),
                "CRATES_RandomHighDistance" => t.random_high_distance = f().unwrap_or(t.random_high_distance),
                "CRATES_AddACrateProb" => t.add_prob = f().unwrap_or(t.add_prob),
                "CRATES_MAX_RUS_GIVEN" => t.max_rus = i().unwrap_or(t.max_rus),
                "CRATES_MIN_RUS_GIVEN" => t.min_rus = i().unwrap_or(t.min_rus),
                "CRATES_GetMotherShipCarrierIfDontHaveOne" => t.mothership_carrier = f().unwrap_or(t.mothership_carrier),
                "CRATES_GetAShip" => t.get_ship = f().unwrap_or(t.get_ship),
                "CRATE_EXPIRY_TIME" => t.expiry_time = f().unwrap_or(t.expiry_time),
                _ => {}
            }
        }
        t
    }
}

/// A crate in the world.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Crate {
    pub position: Vector,
    /// When it appeared (`+0x28c`).
    pub created: f32,
}

/// The crates of a game.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Crates {
    pub tweaks: CrateTweaks,
    /// Seconds since the last chance of a new crate (0xa0c86c).
    pub timer: f32,
    pub list: Vec<Crate>,
}

/// What a crate gave (`cratesRewardPlayer`'s result).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reward {
    /// `count` ships of a type, in a formation at the crate.
    Ships { ship_type: ShipType, count: u32 },
    /// Resource units.
    Resources(i32),
    /// A technology (with what it needs).
    Technology(usize),
}

/// Speech for the finder (`STAT_F_Crate*`).
mod speech_event {
    pub const SHIPS: u32 = 0x200_1049;
    pub const RESOURCES: u32 = 0x200_1048;
    pub const TECHNOLOGY: u32 = 0x200_104a;
}

fn unit_random(u: &mut Universe) -> f32 {
    // `(float)ranRandomFn(10) * 2.3283064e-10` in single precision.
    (u.randy.random(RAN_GAME) as f32) * 2.328_306_4e-10
}

fn random_between(u: &mut Universe, lo: f32, hi: f32) -> f32 {
    let r = u.randy.random(RAN_GAME) as f32;
    st((ld(hi) - ld(lo)) * ld(2.328_306_4e-10) * ld(r) + ld(lo))
}

/// `cratesUpdate` (0x45d3b0), once an update in a multiplayer game with crates on.
pub fn update(u: &mut Universe) {
    if u.single_player || !u.mp.active || u.base.options.flags & mg::CRATES_ENABLED == 0 {
        return;
    }
    let tw = u.mp.crates.tweaks;
    u.mp.crates.timer = st(ld(u.mp.crates.timer) + ld(u.phys_time_elapsed));
    let n = u.mp.num_players;
    if tw.add_check_time <= u.mp.crates.timer && (u.mp.crates.list.len() as f32) < st(ld(n as f32) * ld(tw.max_per_player)) {
        u.mp.crates.timer = 0.0;
        if tw.add_prob < unit_random(u) {
            let at = best_location(u);
            u.mp.crates.list.push(Crate { position: at, created: u.total_time_elapsed });
            u.mp.events.push(crate::multiplayer::MpEvent::CrateAppeared { position: at });
        }
    }
    if tw.near_update_mask & u.update_counter != tw.near_update_frame || u.mp.crates.list.is_empty() {
        return;
    }
    let mut i = 0;
    while i < u.mp.crates.list.len() {
        let c = u.mp.crates.list[i];
        if st(ld(u.total_time_elapsed) - ld(c.created)) <= tw.expiry_time {
            let r = tw.player_near_radius;
            let finder = u.ship_list.iter().filter_map(|&id| u.ships.get(id)).find(|s| {
                let d = |a: f32, b: f32| {
                    let x = st(ld(a) - ld(b));
                    -r < x && x < r
                };
                s.flags & sof::DEAD == 0 && s.player < n && d(c.position.x, s.posinfo.position.x) && d(c.position.y, s.posinfo.position.y) && d(c.position.z, s.posinfo.position.z)
            });
            if let Some(p) = finder.map(|s| s.player) {
                u.mp.crates.list.remove(i);
                player_found_a_crate(u, c, p);
                continue;
            }
            i += 1;
        } else {
            u.mp.crates.list.remove(i);
            u.mp.events.push(crate::multiplayer::MpEvent::CrateExpired { position: c.position });
        }
    }
}

/// `cratesPlayerFoundACrate`.
fn player_found_a_crate(u: &mut Universe, c: Crate, player: usize) {
    let reward = reward_player(u, c, player);
    if player == u.mp.local_player {
        let e = match reward {
            Reward::Ships { .. } => speech_event::SHIPS,
            Reward::Resources(_) => speech_event::RESOURCES,
            Reward::Technology(_) => speech_event::TECHNOLOGY,
        };
        crate::speech::queue(u, None, e, -1);
    }
    u.mp.events.push(crate::multiplayer::MpEvent::CrateFound { player, position: c.position, reward });
}

/// The ship prizes (type, group size) by random number, Somtaaw then Beast.
fn ship_prize(race: ShipRace, f: f32) -> (usize, u32) {
    const SOMTAAW: [(f32, usize, u32); 11] =
        [(0.01, 10, 1), (0.04, 0x19, 1), (0.1, 8, 2), (0.15, 7, 2), (0.2, 6, 2), (0.3, 0x11, 5), (0.4, 4, 4), (0.5, 5, 4), (0.6, 1, 8), (0.7, 2, 8), (0.8, 0, 10)];
    const BEAST: [(f32, usize, u32); 11] = [
        (0.01, 0x2e, 1),
        (0.04, 0x2c, 3),
        (0.1, 0x2a, 2),
        (0.15, 0x29, 2),
        (0.2, 0x30, 2),
        (0.3, 0x34, 5),
        (0.4, 0x26, 4),
        (0.5, 0x27, 3),
        (0.6, 0x23, 8),
        (0.7, 0x28, 3),
        (0.8, 0x21, 10),
    ];
    let (table, last) = if race == ShipRace::Somtaaw { (&SOMTAAW, (0xe, 1)) } else { (&BEAST, (0x31, 1)) };
    table.iter().find(|&&(lim, _, _)| f < lim).map_or(last, |&(_, t, n)| (t, n))
}

/// The technology prizes by random number, Somtaaw then Beast.
fn tech_prize(race: ShipRace, f: f32) -> usize {
    const SOMTAAW: [usize; 20] = [6, 7, 0xc, 0xe, 0x11, 0x12, 0x13, 0x14, 0x17, 0x1b, 0x1c, 0x1d, 0x21, 0x22, 0x24, 0x25, 0x19, 0x18, 0x16, 0x15];
    const BEAST: [usize; 20] = [0, 1, 2, 3, 4, 5, 6, 8, 0x11, 0x12, 0x13, 0x14, 0x18, 0x1a, 0x1c, 0x20, 0x22, 0x27, 0x28, 0xe];
    let table = if race == ShipRace::Somtaaw { &SOMTAAW } else { &BEAST };
    // Steps of 0.05: below 0.05 the first, 0.05..0.1 the second, ..., 0.95 and up the last.
    let mut k = 0;
    let mut lim = 0.05f32;
    while k < 19 && lim <= f {
        k += 1;
        lim = [0.05f32, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5, 0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9, 0.95, 2.0][k];
    }
    table[k]
}

/// `cratesRewardPlayer` (0x45c460).
fn reward_player(u: &mut Universe, c: Crate, player: usize) -> Reward {
    let tw = u.mp.crates.tweaks;
    let race = u.players[player].race;
    if unit_random(u) <= tw.get_ship {
        let f = unit_random(u);
        let (ti, count) = ship_prize(race, f);
        if let Some(t) = ShipType::from_index(ti).filter(|&t| u.statics.ship(t).is_some_and(|s| s.info_needed)) {
            let mut p = c.position;
            let mut made = Vec::new();
            for _ in 0..count {
                let Ok(id) = crate::base::create_ship(u, t, player, p, Default::default()) else { break };
                crate::construction::unit_cap_create_ship(u, id);
                made.push(id);
                let r = u.statics.ship(t).map_or(0.0, |s| s.header.coll.coll_sphere_size);
                p.z = st(ld(r) + ld(r) + ld(p.z));
            }
            crate::command::cl_formation(u, &made, 1);
            return Reward::Ships { ship_type: t, count };
        }
    }
    if unit_random(u) <= tw.get_research {
        let mut tech = 0;
        for _ in 0..5 {
            let f = unit_random(u);
            tech = tech_prize(race, f);
            if !u.base.players[player].tech.contains(tech) {
                break;
            }
        }
        if !u.base.players[player].tech.contains(tech) {
            give_tech_to_player(u, player, tech);
            return Reward::Technology(tech);
        }
    }
    let span = tw.max_rus.wrapping_sub(tw.min_rus).wrapping_add(1);
    let rus = (u.randy.random(RAN_GAME) % span).wrapping_add(tw.min_rus) as i32;
    u.base.players[player].resource_units += rus;
    let st_ = &mut u.base.stats;
    if st_.players.len() <= player {
        st_.players.resize(player + 1, crate::base::PlayerStats::default());
    }
    st_.players[player].ru_collected += rus;
    st_.players[player].ru_total += rus;
    Reward::Resources(rus)
}

/// `rmGiveTechToPlayerByType`: the technology, after what it needs that the player lacks
/// (the highest missing one each time).
pub fn give_tech_to_player(u: &mut Universe, player: usize, tech: usize) {
    let kind = u.base.players[player].research_table;
    loop {
        let table = crate::research::table_for(u, kind);
        let Some(set) = table.tech_depend.get(tech).copied() else { break };
        let dep = crate::base::Techs::from_set(set);
        let have = crate::base::Techs::and(&dep, &u.base.players[player].tech);
        let missing = (0..crate::base::NUM_TECHS).rev().find(|&i| dep.has[i] ^ have.has[i] == 1);
        match missing {
            None | Some(0) => break,
            Some(i) => {
                crate::fow::tech_add(u, player, i);
                crate::upgrade::player_update_from_research(u, player);
            }
        }
    }
    crate::fow::tech_add(u, player, tech);
    crate::upgrade::player_update_from_research(u, player);
}

/// Where a player is for crate placement: its mothership, or its first ship.
fn player_position(u: &Universe, player: usize) -> Vector {
    if let Some(ms) = u.base.players[player].mothership.and_then(|m| u.ships.get(m)) {
        return ms.posinfo.position;
    }
    u.ship_list.iter().filter_map(|&id| u.ships.get(id)).find(|s| s.player == player).map_or(Vector::ZERO, |s| s.posinfo.position)
}

/// `cratesGetBestCrateLocation` (0x45ce60): away from the live players' centre, in the
/// direction they leave open, at a random distance, jittered by up to a third of the
/// universe, moved on until no ship is there, and kept inside the world.
fn best_location(u: &mut Universe) -> Vector {
    let n = u.mp.num_players;
    let live: Vec<usize> = (0..n).filter(|&p| u.base.players[p].state != 1).collect();
    let mut centre = Vector::ZERO;
    for &p in &live {
        let at = player_position(u, p);
        centre = Vector::new(st(ld(at.x) + ld(centre.x)), st(ld(at.y) + ld(centre.y)), st(ld(at.z) + ld(centre.z)));
    }
    let k = 1.0 / live.len().max(1) as f32;
    centre = Vector::new(k * centre.x, k * centre.y, k * centre.z);
    let mut dir = Vector::ZERO;
    for &p in &live {
        let at = player_position(u, p);
        let d = Vector::new(at.x - centre.x, at.y - centre.y, at.z - centre.z).normalized();
        // Each player's pull is divided by its bounty (at least 1).
        let b = f32::from(u.mp.bounties.get(p).copied().unwrap_or(0)).max(1.0);
        let s = 1.0 / b;
        dir = Vector::new(s * d.x + dir.x, s * d.y + dir.y, s * d.z + dir.z);
    }
    let dir = dir.normalized();
    let tw = u.mp.crates.tweaks;
    let mut at = Vector::ZERO;
    at.x = random_between(u, tw.random_low_distance, tw.random_high_distance) * dir.x;
    at.y = random_between(u, tw.random_low_distance, tw.random_high_distance) * dir.y;
    at.z = random_between(u, tw.random_low_distance, tw.random_high_distance) * dir.z;
    at = Vector::new(centre.x + at.x, centre.y + at.y, centre.z + at.z);
    let size = Vector::new(u.world_bounds.x - 5000.0, u.world_bounds.y - 5000.0, u.world_bounds.z - 5000.0);
    let third = |s: f32| s * 0.333_333_34;
    at.x = random_between(u, -third(size.x), third(size.x)) + at.x;
    at.y = random_between(u, -third(size.y), third(size.y)) + at.y;
    at.z = random_between(u, -third(size.z), third(size.z)) + at.z;
    let r1 = u.randy.random(RAN_GAME) as f32;
    let r2 = u.randy.random(RAN_GAME) as f32;
    let r3 = u.randy.random(RAN_GAME) as f32;
    let mut tries = 0;
    while !location_good(u, at) && tries < 10_000 {
        at.x = (r1 * 4.656_613e-7 - 1000.0) + at.x;
        at.y = (r2 * 4.656_613e-7 - 1000.0) + 1000.0 + at.y;
        at.z = (r3 * 4.656_613e-7 - 1000.0) + at.z;
        tries += 1;
    }
    clip_to_limits(u, at)
}

/// `cratesIsLocationGood`: no ship's collision sphere holds the point.
fn location_good(u: &Universe, p: Vector) -> bool {
    u.ship_list.iter().filter_map(|&id| u.ships.get(id)).all(|s| {
        let c = s.coll.coll_position;
        let d2 = (p.x - c.x) * (p.x - c.x) + (p.y - c.y) * (p.y - c.y) + (p.z - c.z) * (p.z - c.z);
        let r = u.statics.ship(s.ship_type).map_or(0.0, |st_| st_.header.coll.coll_sphere_size);
        d2 >= r * r
    })
}

/// `pieMovePointClipToLimits`: the point is kept inside the universe.
fn clip_to_limits(u: &Universe, p: Vector) -> Vector {
    let b = Vector::new(u.world_bounds.x - 5000.0, u.world_bounds.y - 5000.0, u.world_bounds.z - 5000.0);
    Vector::new(p.x.clamp(-b.x, b.x), p.y.clamp(-b.y, b.y), p.z.clamp(-b.z, b.z))
}
