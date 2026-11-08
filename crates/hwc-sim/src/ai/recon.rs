//! Reconnaissance (`aioCreateReconaissance`, `aimProcessActiveRecon`, `aiaSendRecon`,
//! `aimProcessShipRecon`, `aimProcessIntercept`): a scout or two that wander from blob to blob
//! (nearer the enemy or back towards home, by the number of enemy blobs seen) so the player
//! learns where the enemy is, and that the armada sends to look at targets its ships do not
//! see.

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{fild_u32, ld, st};
use hwc_math::random::RAN_AI_PLAYER;
use hwc_math::Vector;

use super::team::{self, add_move, AiMove, Handler, MoveKind, SAME_FORMATION};
use super::{ai, ai_mut, Manager};
use crate::object::ShipId;
use crate::statics::{ShipRace, ShipType};
use crate::universe::Universe;

/// `MOVE_COUNTSHIPS`, `MOVE_ACTIVERECON`, `MOVE_SHIPRECON`, `MOVE_INTERCEPT`.
pub mod move_type {
    pub const INTERCEPT: u32 = 9;
    pub const ACTIVE_RECON: u32 = 0xd;
    pub const SHIP_RECON: u32 = 0xe;
    pub const COUNT_SHIPS: u32 = 0xf;
}

/// The recon moves' formation and tactics (`aimCreateActiveRecon(.., 7, 0, ..)`) and the moves
/// they insert (formation 8).
const RECON_FORMATION: i32 = team::NO_FORMATION;
const RECON_TACTICS: i32 = 0;
const RECON_MOVE_FORMATION: i32 = SAME_FORMATION;
/// How far from the enemy mothership the first scout's standoff point is (0x466a6000).
const FIRST_STANDOFF: f32 = 15000.0;
/// `aimProcessIntercept`: done within this (squared) of the target (0x740aac), and the
/// intercept point is refreshed once the target is this (squared) from it (0x740ab0).
const INTERCEPT_CLOSE_SQ: f32 = 4_000_000.0;
const INTERCEPT_REFRESH_SQ: f32 = 2_000_000.0;
/// `aiaAttackManager`: an advanced player adds a scout while there are more enemy blobs per
/// scout than this (0x7409f4), up to five scouts.
const BLOBS_PER_SCOUT: usize = 5;

fn rnd(u: &mut Universe) -> u32 {
    u.randy.random(RAN_AI_PLAYER)
}

fn ty(n: u8) -> ShipType {
    ShipType::from_index(n.into()).expect("ship type")
}

/// `aimCreateFancyGetShips(team, primary, 1, list, 1, ..)`: priority 1; the list's own flag
/// says whether an alternative is drawn at random.
fn get_ships(u: &mut Universe, t: usize, primary: u8, alternatives: &[(u8, i32)], random: bool) {
    super::attack::fancy_get_ships(u, t, primary, 1, alternatives, random, 1);
}

/// `aioCreateReconaissance` (0x40e3b0): kind 0 sends the scout towards a point near the enemy
/// mothership first; kind 1 wanders the blobs; kind 2 (the usual) wanders the enemy blobs.
pub fn create(u: &mut Universe, p: usize, t: usize, kind: u32) {
    let race = u.players[p].race;
    let enemy_ms = enemy_mothership_coords(u, p);
    if kind == 0 {
        let dest = random_standoff_point(u, enemy_ms, FIRST_STANDOFF, Vector::ZERO, 1);
        match race {
            ShipRace::Somtaaw => get_ships(u, t, 1, &[(4, 14), (4, 14), (3, 14), (3, 14), (0, 20), (2, 10)], true),
            ShipRace::Beast => {
                let x = if super::research::ship_type_acquired(u, p, ty(0x26)) { 0x26 } else { 0x25 };
                get_ships(u, t, 0x23, &[(0x28, 14), (0x28, 14), (0x25, 14), (0x25, 14), (0x21, 20), (0x24, 10), (x, 14), (x, 14)], true);
            }
            _ => {}
        }
        add_move(u, t, AiMove::new(MoveKind::Move { destination: dest }, RECON_FORMATION, 2, true, false));
        add_move(u, t, AiMove::new(MoveKind::CountShips, RECON_MOVE_FORMATION, 2, false, false));
    } else {
        match race {
            ShipRace::Somtaaw => get_ships(u, t, 1, &[(2, 10)], false),
            ShipRace::Beast => get_ships(u, t, 0x23, &[(0x24, 10)], false),
            _ => {}
        }
    }
    let mut m = AiMove::new(MoveKind::ActiveRecon { out: true, enemy: kind != 1 }, RECON_FORMATION, RECON_TACTICS, true, false);
    m.team_died = Some(Handler::Reconnaissance);
    add_move(u, t, m);
    add_move(u, t, AiMove::new(MoveKind::Done, SAME_FORMATION, 2, false, false));
}

/// `aiaProcessSpecialTeams` (0x4018c0), on the player's first turn: the scouts (and the harass
/// team).
pub fn special_teams(u: &mut Universe, p: usize) {
    let f = ai(u, p).attack_features;
    if ai(u, p).attack.recon_teams.is_empty() {
        if f & super::attack::feature::RECON != 0 {
            let t = super::team_create(u, p, Manager::Attack, "");
            ai_mut(u, p).attack.recon_teams.push(t);
            create(u, p, t, 0);
        }
        if f & super::attack::feature::ADVANCED_RECON != 0 {
            let t = super::team_create(u, p, Manager::Attack, "");
            ai_mut(u, p).attack.recon_teams.push(t);
            create(u, p, t, 2);
        }
    }
}

/// `aiaAttackManager`'s extra scouts of an advanced player.
pub fn more_scouts(u: &mut Universe, p: usize) {
    if ai(u, p).attack_features & super::attack::feature::ADVANCED_RECON == 0 {
        return;
    }
    let blobs = super::attack::enemy_blobs(u, p).len();
    let n = ai(u, p).attack.recon_teams.len();
    if n == 0 || (n < 5 && BLOBS_PER_SCOUT < blobs / n) {
        let t = super::team_create(u, p, Manager::Attack, "");
        ai_mut(u, p).attack.recon_teams.push(t);
        create(u, p, t, 2);
    }
}

/// `aiaSendRecon` (0x401000): the scout nearest the targets goes to look at them.
pub fn send_recon(u: &mut Universe, p: usize, targets: &[ShipId]) -> Option<usize> {
    let c = team::centre(u, targets);
    let mut best = 1e21f32;
    let mut found = None;
    for &t in &ai(u, p).attack.recon_teams {
        let Some(Some((_, tm))) = u.ai.teams.get(t) else { continue };
        let Some(s) = tm.ships.first().and_then(|&id| u.ships.get(id)) else { continue };
        let d = st(crate::level::distance_squared(c, s.posinfo.position));
        if d < best {
            best = d;
            found = Some(t);
        }
    }
    let t = found?;
    let mut m = AiMove::new(MoveKind::ShipRecon { targets: targets.to_vec(), found: Vec::new() }, RECON_FORMATION, RECON_TACTICS, true, true);
    m.team_died = Some(Handler::Reconnaissance);
    team::insert_move(u, t, m);
    Some(t)
}

/// `aihReconaissanceTeamDiedHandler`: the scout team starts over as a kind 2 scout.
pub fn team_died_handler(u: &mut Universe, t: usize) {
    let Some(p) = u.ai.teams.get(t).and_then(|x| x.as_ref()).map(|x| x.0) else { return };
    team::delete_all_moves(u, t);
    create(u, p, t, 2);
}

/// `aiuFindEnemyMothershipCoords` (0x41b0e0): where the primary enemy's mothership is (the
/// origin when it has none).
pub fn enemy_mothership_coords(u: &Universe, p: usize) -> Vector {
    super::attack::find_enemy_mothership(u, p).and_then(|m| u.ships.get(m)).map_or(Vector::ZERO, |s| s.posinfo.position)
}

/// `aiuGenerateRandomStandoffPoint` (0x419ee0): a point `distance` from `centre` in a random
/// direction; with `mode` 1 the direction must face `towards`, with 2 face away (drawn again
/// until it does).
pub fn random_standoff_point(u: &mut Universe, centre: Vector, distance: f32, towards: Vector, mode: u32) -> Vector {
    loop {
        let mut d = [0f32; 3];
        for v in &mut d {
            let r = rnd(u);
            *v = st(fild_u32(r) * ld(f32::from_bits(0x31a0_0000)) - 10.0);
        }
        let len = fsqrt(ld(d[0]) * ld(d[0]) + ld(d[1]) * ld(d[1]) + ld(d[2]) * ld(d[2]));
        let n = if len == 0.0 { [0.0; 3] } else { [st(ld(d[0]) / len), st(ld(d[1]) / len), st(ld(d[2]) / len)] };
        let p = Vector::new(st(ld(centre.x) + ld(n[0]) * ld(distance)), st(ld(centre.y) + ld(n[1]) * ld(distance)), st(ld(centre.z) + ld(n[2]) * ld(distance)));
        let dot = ld(d[0]) * (ld(towards.x) - ld(centre.x)) + ld(d[1]) * (ld(towards.y) - ld(centre.y)) + ld(d[2]) * (ld(towards.z) - ld(centre.z));
        match mode {
            1 if dot < 0.0 => continue,
            2 if 0.0 < dot => continue,
            _ => return p,
        }
    }
}

fn blob_centre(u: &Universe, id: ShipId) -> Option<Vector> {
    let b = u.ships.get(id)?.coll_my_blob?;
    u.blobs.get(b).map(|b| b.centre)
}

fn dist_sq(a: Vector, b: Vector) -> f32 {
    let (x, y, z) = (ld(a.x) - ld(b.x), ld(a.y) - ld(b.y), ld(a.z) - ld(b.z));
    st(x * x + y * y + z * z)
}

fn ahead(home: Vector, here: Vector, there: Vector) -> f64 {
    (ld(home.x) - ld(here.x)) * (ld(there.x) - ld(here.x)) + (ld(home.y) - ld(here.y)) * (ld(there.y) - ld(here.y)) + (ld(home.z) - ld(here.z)) * (ld(there.z) - ld(here.z))
}

/// `aiuFindStandoffOfNextNearestBlobIn` / `Out` (0x41d550, 0x41d310) and their enemy-blob
/// forms (0x41da30, 0x41d790): a point in the next blob further from the mothership (`out`
/// false: the farthest blob on the far side of the scout) or nearer to it (`out` true: the
/// nearest on the home side); the origin when there is none.
fn next_blob_standoff(u: &mut Universe, p: usize, scout: ShipId, out: bool, enemy: bool) -> Vector {
    let Some(ms) = u.base.players[p].mothership else { return Vector::ZERO };
    let (Some(home), Some(here)) = (blob_centre(u, ms), blob_centre(u, scout)) else { return Vector::ZERO };
    // (centre, radius, holds a mothership)
    let blobs: Vec<(Vector, f32, bool)> = if enemy {
        super::attack::enemy_blobs(u, p).into_iter().map(|b| (b.centre, b.radius, b.mothership)).collect()
    } else {
        u.blobs.blobs.iter().map(|b| (b.centre, b.radius, false)).collect()
    };
    let mut best: Option<(Vector, f32, bool)> = None;
    let mut best_d = if out { 1e21f32 } else { 0.0 };
    for &(c, r, m) in &blobs {
        if c == here {
            continue;
        }
        let d = dist_sq(home, c);
        let a = ahead(home, here, c);
        let pick = if out { d < best_d && a < 0.0 } else { best_d < d && 0.0 <= a };
        if pick {
            best = Some((c, r, m));
            best_d = d;
        }
    }
    let Some((c, r, m)) = best else { return Vector::ZERO };
    let distance = if enemy && out && m { FIRST_STANDOFF } else { st(ld(r) * 0.5) };
    let scout_pos = u.ships.get(scout).map_or(Vector::ZERO, |s| s.posinfo.position);
    // Only the enemy blob in-search passes the scout as the point to face (mode 0: unused).
    random_standoff_point(u, c, distance, scout_pos, 0)
}

/// `aimProcessActiveRecon` (0x40a4f0): never done while the mothership lives; the first time
/// the team flies towards the enemy mothership for five seconds, then from blob to blob, out
/// and back.
pub fn process_active_recon(u: &mut Universe, t: usize, c: usize) -> bool {
    let Some(Some((p, tm))) = u.ai.teams.get(t) else { return true };
    let p = *p;
    let Some(&lead) = tm.ships.first() else { return false };
    let MoveKind::ActiveRecon { out, enemy } = tm.moves[c].kind else { return true };
    if !tm.moves[c].processing {
        let dest = enemy_mothership_coords(u, p);
        // (A script team in single player would use its own tactics.)
        let tactics = 2;
        let m = AiMove::new(
            MoveKind::MoveTo { destination: dest, limit: 5.0, by_distance: false, start_time: 0.0, start: Vector::ZERO },
            RECON_MOVE_FORMATION,
            tactics,
            true,
            true,
        );
        team::add_move_before_and_make_current(u, t, m);
        let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
        let cur = tm.current.expect("current");
        tm.moves[cur + 1].processing = true;
        return false;
    }
    if u.base.players[p].mothership.is_none() {
        return true;
    }
    let point = next_blob_standoff(u, p, lead, out, enemy);
    if point == Vector::ZERO {
        if let MoveKind::ActiveRecon { out, .. } = &mut u.ai.teams[t].as_mut().expect("team").1.moves[c].kind {
            *out = !*out;
        }
        return false;
    }
    team::add_move_before_and_make_current(u, t, AiMove::new(MoveKind::Move { destination: point }, RECON_MOVE_FORMATION, 2, true, true));
    let n = super::attack::enemy_blobs(u, p).len() as u32;
    let r = rnd(u) % 0x65;
    let enemy = if enemy { r <= 100u32.wrapping_sub(n.wrapping_mul(20)) } else { n.wrapping_mul(15) < r };
    let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
    let cur = tm.current.expect("current");
    if let MoveKind::ActiveRecon { enemy: e, .. } = &mut tm.moves[cur + 1].kind {
        *e = enemy;
    }
    false
}

/// `aimProcessShipRecon` (0x40a840): the targets that come into the scout's blob are found;
/// done when none are left to find. Until then an intercept of the nearest waits after it.
pub fn process_ship_recon(u: &mut Universe, t: usize, c: usize) -> bool {
    let Some(Some((_, tm))) = u.ai.teams.get(t) else { return true };
    let Some(&lead) = tm.ships.first() else { return true };
    let MoveKind::ShipRecon { mut targets, mut found } = tm.moves[c].kind.clone() else { return true };
    if targets.is_empty() {
        return true;
    }
    let mut processing = tm.moves[c].processing;
    let in_blob: Vec<ShipId> = u.ships.get(lead).and_then(|s| s.coll_my_blob).and_then(|b| u.blobs.get(b)).map(|b| b.ships.iter().copied().filter(|s| targets.contains(s)).collect()).unwrap_or_default();
    if !in_blob.is_empty() {
        targets.retain(|s| !in_blob.contains(s));
        for s in in_blob {
            if !found.contains(&s) {
                found.push(s);
            }
        }
        processing = false;
    }
    let done = targets.is_empty();
    let mut intercept = None;
    if !done && !processing {
        let lp = u.ships.ship(lead).posinfo.position;
        let nearest = targets.iter().copied().filter_map(|id| u.ships.get(id).map(|s| (id, st(crate::level::distance_squared(lp, s.posinfo.position))))).fold(None, |b: Option<(ShipId, f32)>, x| if b.is_none_or(|b| x.1 < b.1) { Some(x) } else { b });
        intercept = nearest.map(|n| n.0);
        processing = true;
    }
    let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
    tm.moves[c].kind = MoveKind::ShipRecon { targets, found };
    tm.moves[c].processing = processing;
    if let Some(target) = intercept {
        let m = AiMove::new(MoveKind::Intercept { target: Some(target), wait: 5.0, until: 0.0, point: Vector::ZERO, moving: false }, RECON_FORMATION, RECON_TACTICS, true, true);
        tm.moves.insert(c + 1, m);
    }
    done
}

/// `aimProcessIntercept` (0x408150): the team flies to where the target is going (halfway to
/// its destination when it is moving) and looks again every `wait` seconds; done when close to
/// the target or it is gone.
pub fn process_intercept(u: &mut Universe, t: usize, c: usize) -> bool {
    let Some(Some((_, tm))) = u.ai.teams.get(t) else { return true };
    let Some(&lead) = tm.ships.first() else { return true };
    let MoveKind::Intercept { target, wait, until, point, moving } = tm.moves[c].kind.clone() else { return true };
    let Some(target) = target else { return true };
    let Some(ts) = u.ships.get(target) else { return true };
    let tpos = ts.posinfo.position;
    let tdest = ts.move_to;
    let now = u.total_time_elapsed;
    if !tm.moves[c].processing {
        let (point, moving) = if tdest == Vector::ZERO {
            (tpos, false)
        } else {
            (Vector::new(st((ld(tpos.x) + ld(tdest.x)) * 0.5), st((ld(tdest.y) + ld(tpos.y)) * 0.5), st((ld(tdest.z) + ld(tpos.z)) * 0.5)), true)
        };
        let formation = tm.moves[c].formation;
        let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
        tm.moves[c].kind = MoveKind::Intercept { target: Some(target), wait, until: st(ld(now) + ld(wait)), point, moving };
        tm.moves[c].processing = true;
        team::add_move_before_and_make_current(u, t, AiMove::new(MoveKind::Move { destination: point }, formation, 2, false, true));
        return false;
    }
    let lp = u.ships.ship(lead).posinfo.position;
    if dist_sq(tpos, lp) < INTERCEPT_CLOSE_SQ {
        return true;
    }
    if now < until {
        return false;
    }
    let again = if !moving { INTERCEPT_REFRESH_SQ < dist_sq(tpos, point) } else { tdest != point };
    if again {
        u.ai.teams[t].as_mut().expect("team").1.moves[c].processing = false;
    }
    false
}
