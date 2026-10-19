//! `univShipAutoBehaviourUpdate` (0x53efa0): what a ship does on its own each think.
//!
//! * A materialising ship (`specialFlags2 & 0x80000`, e.g. built by KAS) grows its health over
//!   its build time and pays for itself when it starts.
//! * Ships of computer players use their specials against enemies within their vision range,
//!   at random with a chance per 32 frames that grows with the difficulty (1, 2, 5, 10, 15 in
//!   201 by single-player skill; 1, 5, 10 by computer-player difficulty): Mimics disguise,
//!   Leeches latch on, ACVs fire their EMP, rammers ram, hive frigates launch, the Dreadnought
//!   fires its cannon, cloakers cloak (and decloak with no enemy about), and the Beast's heavy
//!   cruisers, carriers and mothership infect. In multiplayer games they also pick fights with
//!   the enemies of their computer player (its primary enemy, whoever shot at them, or anyone
//!   per the player's bigotry).

use hwc_math::fpu::{ld, st};
use hwc_math::random::RAN_AI_PLAYER;

use crate::base::special2;
use crate::object::ShipId;
use crate::statics::{ShipRace, ShipType};
use crate::universe::Universe;

/// Frame mask of the special and attack decisions (0x8ab844).
const DECISION_RATE: u32 = 0x1f;
/// Frame mask of the upgrade decision (0x8ab84c).
const UPGRADE_RATE: u32 = 0xff;

/// The chance (in 201) per decision that a computer ship acts.
fn chance(u: &Universe, player: usize) -> u32 {
    if u.single_player {
        match u.sp_difficulty {
            0 => 1,
            1 => 2,
            2 => 5,
            3 => 10,
            _ => 15,
        }
    } else {
        match u.base.players.get(player).map_or(0, |p| p.ai_difficulty) {
            0 => 1,
            1 => 5,
            _ => 10,
        }
    }
}

/// The materialising part: returns true when the ship is still materialising (or just
/// finished), so the rest is skipped.
fn materialise(u: &mut Universe, id: ShipId) -> bool {
    let statics = u.statics.clone();
    let now = u.total_time_elapsed;
    let s = u.ships.ship(id);
    if s.special_flags2 & special2::MATERIALISING == 0 {
        return false;
    }
    let Some(stat) = statics.ship(s.ship_type) else { return true };
    if s.base.materialise_fraction <= 0.0 {
        let t = stat.build_time;
        let secs = ((t + ((t >> 31) & 15)) >> 4) as f32;
        let player = s.player;
        let s = u.ships.ship_mut(id);
        s.base.materialise_timer.start(now, secs);
        s.base.materialise_fraction = f32::from_bits(0x3586_37bd);
        u.base.players[player].resource_units -= stat.build_cost;
        return true;
    }
    let f = s.base.materialise_timer.fraction(now);
    let s = u.ships.ship_mut(id);
    s.base.materialise_fraction = if f > 1.0 { 1.0 } else { st(f) };
    let h = st(ld(stat.health.max_health) * ld(s.base.materialise_fraction));
    s.health = h;
    if h < stat.health.max_health {
        return true;
    }
    s.health = stat.health.max_health;
    s.special_flags2 &= !special2::UNDER_CONSTRUCTION;
    let (player, t, race) = (s.player, s.ship_type, s.ship_race);
    if race == ShipRace::Somtaaw && stat.ship_class == Some(crate::statics::ShipClass::Component) {
        crate::research::activate_lab(u, player, t, false);
    }
    u.ships.ship_mut(id).special_flags2 &= !special2::MATERIALISING;
    true
}

/// `univShipAutoBehaviourUpdate` (0x53efa0).
pub fn update(u: &mut Universe, id: ShipId) {
    if materialise(u, id) {
        return;
    }
    let s = u.ships.ship(id);
    if s.special_flags2 & special2::UNDER_CONSTRUCTION != 0 {
        return;
    }
    let player = s.player;
    if !u.base.players.get(player).is_some_and(|p| p.ai) {
        return;
    }
    let number = u32::from(s.id.0);
    let counter = u.update_counter;
    // (Every 256 frames a computer ship that can be upgraded goes to be upgraded; the upgrade
    // command is not modelled.)
    let _ = UPGRADE_RATE;
    if DECISION_RATE & counter != number & DECISION_RATE {
        return;
    }
    let p = chance(u, player);
    let t = s.ship_type;
    let sp = u.single_player;
    let mut enemies: Option<Vec<ShipId>> = None;
    if u.randy.random(RAN_AI_PLAYER) % 201 < p {
        let range = st(crate::visibility::vision_range(u, id));
        let found = crate::tactics::enemies_within_proximity(u, id, range);
        let cloaker = matches!(t, ShipType::bCloakedFighter | ShipType::bIonArrayFrigate);
        if found.is_empty() {
            if cloaker && !sp && crate::cloak::is_cloaked(u.ships.ship(id)) {
                crate::ships::cl_special(u, &[id], &[]);
            }
        } else {
            match t {
                ShipType::sMimic | ShipType::sMCV => {
                    if !sp && !crate::ships::mimic::is_disguised(u, id) {
                        let race = u.players[player].race;
                        let disguise = crate::ships::mimic::pick_random_disguise_type(u, t, race);
                        crate::ships::mimic::cl_disguise_as(u, &[id], disguise, player);
                    }
                }
                ShipType::sLeech => {
                    if !crate::ships::leech::is_active(u, id) {
                        crate::ships::cl_special(u, &[id], &found);
                    }
                }
                ShipType::sACV | ShipType::bACV => {
                    if !sp {
                        crate::ships::cl_special(u, &[id], &found);
                    }
                }
                ShipType::sRammingFrigate | ShipType::bRammingFrigate => {
                    if !sp && !crate::ships::ramming::is_live_rammer(u, id) {
                        crate::ships::cl_special(u, &[id], &found);
                    }
                }
                ShipType::sHiveFrigate | ShipType::bHiveFrigate => {
                    if !sp && !crate::ships::hive::is_launching(u, id) {
                        crate::ships::cl_special(u, &[id], &found);
                    }
                }
                ShipType::sDreadnought => {
                    if !sp {
                        crate::ships::cl_special(u, &[id], &[]);
                    }
                }
                ShipType::bCloakedFighter | ShipType::bIonArrayFrigate => {
                    if !crate::cloak::is_cloaked(u.ships.ship(id)) && !sp {
                        crate::ships::cl_special(u, &[id], &[]);
                    }
                }
                ShipType::bHeavyCruiser | ShipType::bCarrier | ShipType::bMothership
                    if u.ships.ship(id).special_flags2 & special2::SPECIAL_BUSY == 0 =>
                {
                    crate::ships::cl_special(u, &[id], &found);
                }
                _ => {}
            }
        }
        enemies = Some(found);
    }
    // Picking fights (multiplayer only).
    if matches!(t, ShipType::sLeech | ShipType::sWorker | ShipType::bWorker) || sp {
        return;
    }
    if u.randy.random(RAN_AI_PLAYER) % 201 >= p {
        return;
    }
    let bigotry = if u.multiplayer_game { u32::from(u.base.players[player].ai_bigotry) } else { 100 };
    let anyone = bigotry <= u.randy.random(RAN_AI_PLAYER) % 101;
    let enemies = match enemies {
        Some(e) => e,
        None => {
            let range = st(crate::visibility::vision_range(u, id));
            crate::tactics::enemies_within_proximity(u, id, range)
        }
    };
    if enemies.is_empty() {
        return;
    }
    let primary = u.base.players[player].ai_primary_enemy;
    let rocker = u.ships.ship(id).getting_rocked.and_then(|r| u.ships.get(r)).map(|r| r.player);
    let wanted = |e: ShipId| {
        let ep = u.ships.ship(e).player;
        anyone || Some(ep) == primary || rocker == Some(ep)
    };
    let cmd = u.ships.ship(id).command.and_then(|c| u.commands.get(c)).cloned();
    match cmd {
        Some(c) if c.order == crate::command::order::ATTACK => {
            let mut targets = c.attack_targets.clone();
            let before = targets.len();
            for &e in &enemies {
                if wanted(e) && !targets[..before].contains(&e) {
                    targets.push(e);
                }
            }
            if targets.len() > before {
                crate::command::cl_attack(u, &[id], &targets);
            }
        }
        _ => {
            let targets: Vec<ShipId> = enemies.iter().copied().filter(|&e| wanted(e)).collect();
            if !targets.is_empty() {
                crate::command::cl_attack(u, &[id], &targets);
            }
        }
    }
}
