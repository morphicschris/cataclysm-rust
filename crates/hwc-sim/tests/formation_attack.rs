//! Formation attacks over the real game data (`HWC_GAME_DIR` or `HWC_EXTRACTED_DIR`; skipped
//! when neither is set): `processAttackToDoInFormation` / `delegateCommand` keep a formation
//! whose leader holds it during battle (neutral, aggressive) in its slots across its attack
//! runs, while an evasive leader's formation breaks up and re-forms when the attack ends.

use std::sync::{Arc, OnceLock};

use hwc_formats::big::BigArchive;
use hwc_math::Vector;
use hwc_sim::command::{attr, order};
use hwc_sim::formation::kind;
use hwc_sim::harness::{Order, Run, Scenario, Spawn};
use hwc_sim::object::tactics;
use hwc_sim::statics::{DataSource, DirSource, LoadOptions, ShipRace, ShipType, StaticWorld};
use hwc_sim::{ShipId, SimTweaks};

struct Data {
    statics: Arc<StaticWorld>,
    tweaks: SimTweaks,
}

/// Loads the static world and the tweaks afresh from the game data.
fn load() -> Option<Data> {
    let src: Box<dyn DataSource> = if let Ok(dir) = std::env::var("HWC_EXTRACTED_DIR") {
        Box::new(DirSource::new(dir).ok()?)
    } else if let Ok(dir) = std::env::var("HWC_GAME_DIR") {
        Box::new(BigArchive::open(std::path::Path::new(&dir).join("cataclysm.big")).ok()?)
    } else {
        return None;
    };
    let statics = Arc::new(StaticWorld::load_from(src.as_ref(), LoadOptions::default()));
    let tweaks = SimTweaks::load(src.as_ref());
    Some(Data { statics, tweaks })
}

fn data() -> Option<&'static Data> {
    static DATA: OnceLock<Option<Data>> = OnceLock::new();
    DATA.get_or_init(load).as_ref()
}

/// What one formation attack did, sampled every update once the formation reached its target.
#[derive(Debug, Default)]
struct Sampled {
    /// Per update between the leader's turns: the wingmen's mean distance from their slots.
    slot_error: Vec<f32>,
    /// Attack runs the leader made (passes close to the target).
    runs: usize,
    /// Updates the formation's axes were locked for a manoeuvre handed to the wingmen.
    locked: usize,
    /// Updates at least one wingman flew the leader's manoeuvre.
    wingmen_manoeuvring: usize,
    /// The command kept its formation attribute and every ship throughout.
    kept_command: bool,
    /// The attack ended (target gone) and the command is a formation again.
    ended: bool,
    /// Updates after the end that the formation flew its holding pattern.
    holding: usize,
    /// After the end: the slot error once the ships have settled.
    settled_error: Option<f32>,
    /// The world hash after every update.
    hashes: Vec<u64>,
}

impl Sampled {
    fn percentile(&self, p: f32) -> f32 {
        let mut v = self.slot_error.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[((v.len() - 1) as f32 * p) as usize]
    }
}

/// Six ships of `ship_type` in `formation_type` with `tac` tactics attack a target that cannot
/// be killed until `kill_at` (then it dies), for `ticks` updates.
fn attack(ship_type: ShipType, formation_type: u32, tac: usize, ticks: u32, kill_at: Option<u32>) -> Option<Sampled> {
    attack_with(data()?, ship_type, formation_type, tac, ticks, kill_at)
}

fn attack_with(d: &Data, ship_type: ShipType, formation_type: u32, tac: usize, ticks: u32, kill_at: Option<u32>) -> Option<Sampled> {
    let n = 6;
    let mut spawns: Vec<Spawn> = (0..n).map(|i| Spawn { ship_type, player: 0, position: Vector::new(-12000.0, i as f32 * 250.0, 0.0) }).collect();
    spawns.push(Spawn { ship_type: ShipType::tResourceCollector, player: 1, position: Vector::new(8000.0, 0.0, 0.0) });
    let ours: Vec<usize> = (0..n).collect();
    let sc = Scenario {
        name: "formation_attack_runs",
        players: vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        spawns,
        orders: vec![
            (0, Order::Tactics { ships: ours.clone(), tactics: tac }),
            (0, Order::Formation { ships: ours.clone(), formation_type }),
            (200, Order::Attack { ships: ours.clone(), targets: vec![n] }),
        ],
        resources: Vec::new(),
        resource_units: Vec::new(),
    };
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), sc).unwrap();
    let ids: Vec<ShipId> = run.ids.clone();
    let target = ids[n];
    let mut s = Sampled { kept_command: true, ..Default::default() };
    let (mut approaching, mut prev_dist) = (false, f32::MAX);
    let mut ended_at = None;
    for tick in 0..ticks {
        if let Some(t) = run.universe.ships.get_mut(target) {
            // Out of reach of the attack until it is meant to die.
            t.health = if kill_at.is_some_and(|k| tick >= k) { 0.5 } else { 1.0e9 };
        }
        s.hashes.push(run.step());
        if tick < 100 {
            continue;
        }
        let u = &run.universe;
        let Some(c) = ids[..n].iter().find_map(|&id| u.ship(id)).and_then(|x| x.command).and_then(|c| u.commands.get(c)) else {
            s.kept_command = false;
            continue;
        };
        if c.attributes & attr::FORMATION == 0 || ids[..n].iter().any(|&id| u.ship(id).is_some_and(|x| x.command != Some(c.id))) {
            s.kept_command = false;
        }
        // The formation's leader is the first ship of its (sorted) selection.
        let leader = u.ship(c.selection[0]).expect("leader");
        let in_attack = c.order == order::ATTACK && c.attack_in_range;
        if u.ship(target).is_none() && c.order == order::NULL && ended_at.is_none() {
            s.ended = true;
            ended_at = Some(tick);
        }
        let coordsys = if c.formation.locked { c.formation.coordsys } else { leader.rotinfo.coordsys };
        let origin = leader.coll.coll_position;
        let wingmen: Vec<_> = c.selection[1..].iter().filter_map(|&id| u.ship(id)).collect();
        let error = wingmen
            .iter()
            .map(|w| {
                let slot = coordsys.mul_vec(w.formation_offset) + origin;
                (w.posinfo.position - slot).mag_sq().sqrt()
            })
            .sum::<f32>()
            / wingmen.len().max(1) as f32;
        let dist = u.ship(target).map_or(f32::MAX, |t| (leader.posinfo.position - t.posinfo.position).mag_sq().sqrt());
        if std::env::var("HWC_TRACE").is_ok() && tick % 16 == 0 {
            println!(
                "t{tick} order {} inr {} err {error:.0} as {} fm {} talk {} lock {} dist {dist:.0} spd {:.0}",
                c.order,
                c.attack_in_range,
                leader.ai_state_attack,
                leader.flightman,
                leader.tactics_talk,
                c.formation.locked,
                leader.posinfo.velocity.mag_sq().sqrt()
            );
        }
        if in_attack {
            let manoeuvring = wingmen.iter().any(|w| w.tactics_need_to_flightman != 0);
            if c.formation.locked {
                s.locked += 1;
            }
            if manoeuvring {
                s.wingmen_manoeuvring += 1;
            }
            // While the axes are locked for a turn the slots are those after it (the wingmen
            // fly the turn themselves), so the geometry is measured between turns.
            if !c.formation.locked && !manoeuvring {
                s.slot_error.push(error);
            }
            // A run: the leader's closest approach to the target, then away again.
            if approaching && dist > prev_dist && prev_dist < 2500.0 {
                s.runs += 1;
                approaching = false;
            } else if dist < prev_dist {
                approaching = true;
            }
        }
        prev_dist = dist;
        if ended_at.is_some() && c.attributes & attr::HOLDING_PATTERN != 0 {
            s.holding += 1;
        }
        if let Some(e) = ended_at {
            if tick >= e + 30 * 16 {
                s.settled_error = Some(error);
                break;
            }
        }
    }
    if std::env::var("HWC_PRINT").is_ok() {
        println!(
            "{ship_type:?} type {formation_type} tactics {tac}: samples {} runs {} locked {} manoeuvring {} p50 {:.0} p90 {:.0} max {:.0} kept {} ended {} settled {:?}",
            s.slot_error.len(),
            s.runs,
            s.locked,
            s.wingmen_manoeuvring,
            s.percentile(0.5),
            s.percentile(0.9),
            s.percentile(1.0),
            s.kept_command,
            s.ended,
            s.settled_error
        );
    }
    Some(s)
}

/// A neutral Delta of Acolytes attacking a target stays in its slots across several attack
/// runs: the leader makes its runs (approach, pass, break off, come round) and the wingmen fly
/// the same flip turns with the formation's axes locked, keep station on it otherwise, and
/// fire at the target only while it is ahead.
#[test]
fn acolyte_delta_keeps_its_slots_across_attack_runs() {
    let Some(s) = attack(ShipType::sAcolyte, kind::DELTA, tactics::NEUTRAL, 16 * 150, None) else { return };
    assert!(s.kept_command, "one formation command throughout");
    assert!(s.slot_error.len() > 1000, "the attack ran ({} updates)", s.slot_error.len());
    assert!(s.runs >= 3, "several attack runs ({})", s.runs);
    assert!(s.locked > 0 && s.wingmen_manoeuvring > 0, "the wingmen fly the leader's turns in a locked formation");
    let (p50, p90) = (s.percentile(0.5), s.percentile(0.9));
    assert!(p50 < 250.0, "the wingmen hold their slots (median error {p50:.0})");
    assert!(p90 < 450.0, "the wingmen hold their slots (90th percentile error {p90:.0})");
}

/// When the target dies the formation (strike craft) flies its holding pattern
/// (`processHoldingPattern`) and stays in formation, settling back into its slots.
#[test]
fn acolyte_delta_reforms_after_the_target_dies() {
    let Some(s) = attack(ShipType::sAcolyte, kind::DELTA, tactics::AGGRESSIVE, 16 * 120, Some(16 * 60)) else { return };
    assert!(s.kept_command, "one formation command throughout");
    assert!(s.ended, "the attack ended when the target died");
    assert!(s.holding > 16 * 20, "the strike craft fly their holding pattern ({} updates)", s.holding);
    let e = s.settled_error.expect("sampled after the end");
    assert!(e < 300.0, "back in the slots after the attack ({e:.0})");
}

/// An evasive leader does not hold the formation during battle (`holdFormationDuringBattle`
/// [Evasive] is 0): the ships attack on their own and the slots are not kept.
#[test]
fn evasive_formation_attacks_out_of_formation() {
    let Some(held) = attack(ShipType::sAcolyte, kind::DELTA, tactics::NEUTRAL, 16 * 90, None) else { return };
    let Some(free) = attack(ShipType::sAcolyte, kind::DELTA, tactics::EVASIVE, 16 * 90, None) else { return };
    assert!(free.locked == 0 && free.wingmen_manoeuvring == 0, "no formation manoeuvres when evasive");
    assert!(free.percentile(0.5) > 2.0 * held.percentile(0.5), "evasive {:.0} vs neutral {:.0}", free.percentile(0.5), held.percentile(0.5));
}

/// Every slot formation type attacks in formation with neutral tactics: Delta, Broad, X
/// (Delta3D), Claw and Wall keep the command and their slots; the fighters' leader makes its
/// runs and hands its turns to the wingmen.
#[test]
fn each_formation_type_attacks_in_formation() {
    for t in [kind::DELTA, kind::BROAD, kind::DELTA3D, kind::CLAW, kind::WALL] {
        let Some(s) = attack(ShipType::sAcolyte, t, tactics::NEUTRAL, 16 * 100, None) else { return };
        assert!(s.kept_command, "type {t}: one formation command throughout");
        assert!(s.runs >= 2, "type {t}: attack runs ({})", s.runs);
        assert!(s.wingmen_manoeuvring > 0, "type {t}: the wingmen fly the leader's turns");
        assert!(s.percentile(0.5) < 300.0, "type {t}: median slot error {:.0}", s.percentile(0.5));
        assert!(s.percentile(0.9) < 450.0, "type {t}: 90th percentile slot error {:.0}", s.percentile(0.9));
    }
}


/// The scripts the formation attack depends on were read: `holdFormationDuringBattle` is 0 for
/// evasive and 1 for neutral and aggressive leaders. Every other test here fails in a
/// confusing way (formations break up as if evasive) when it is not.
#[test]
fn tactics_script_holds_formations_for_neutral_and_aggressive() {
    let Some(d) = data() else { return };
    assert_eq!(d.tweaks.combat.hold_formation_during_battle, [0, 1, 1], "holdFormationDuringBattle[Evasive, Neutral, Aggressive] from Tactics.script");
}

/// Determinism: the same formation attack, run twice in one process from the same data and
/// once more from data loaded afresh, gives the same world hash after every update (nothing
/// in the load or the update depends on hashing, addresses, globals or earlier runs). With
/// `HWC_PRINT` the final hash is printed so separate builds (`-p hwc-sim`, `--workspace`,
/// `-p cataclysm -p hwc-sim`) can be compared.
#[test]
fn formation_attack_is_reproducible() {
    let Some(d) = data() else { return };
    let ticks = 16 * 70;
    let a = attack_with(d, ShipType::sAcolyte, kind::DELTA, tactics::NEUTRAL, ticks, Some(16 * 40)).expect("run");
    let b = attack_with(d, ShipType::sAcolyte, kind::DELTA, tactics::NEUTRAL, ticks, Some(16 * 40)).expect("run");
    let fresh = load().expect("data");
    let c = attack_with(&fresh, ShipType::sAcolyte, kind::DELTA, tactics::NEUTRAL, ticks, Some(16 * 40)).expect("run");
    assert!(!a.hashes.is_empty());
    for (other, what) in [(&b, "second run"), (&c, "run from freshly loaded data")] {
        assert_eq!(a.hashes.len(), other.hashes.len(), "{what}: same number of updates");
        if let Some(t) = a.hashes.iter().zip(&other.hashes).position(|(x, y)| x != y) {
            panic!("{what}: world hash differs from update {t} ({:016x} vs {:016x})", a.hashes[t], other.hashes[t]);
        }
    }
    if std::env::var("HWC_PRINT").is_ok() {
        println!("formation attack world hash after {} updates: {:016x}", a.hashes.len(), a.hashes.last().unwrap());
    }
}
