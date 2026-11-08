//! A computer player's teams and their moves (`AITeam.c`, `AIMoves.c`, `AIVar.c`): each team
//! works through its list of moves, one current at a time, every turn of its player
//! (`aitExecute`). The moves here are the ones the attack manager's orders use: getting ships
//! built or handed over (`FancyGetShips`), the attack variable (`VarDec`), waiting out of the way
//! for the signal to attack (`TempGuard`), the armada (`Armada`) and the attacks it leads to
//! (`Attack`, `FlankAttack`, `Move`, `MoveTo`).

use hwc_math::fpu::{ld, st};
use hwc_math::Vector;

use super::{ai, ai_mut, Manager};
use crate::command;
use crate::object::{sof, ShipId};
use crate::statics::ShipType;
use crate::universe::Universe;

/// The move numbers (`AIMoveTypes`) the code looks at.
pub mod move_type {
    pub const DONE: u32 = 0;
    pub const GUARD_SHIPS: u32 = 1;
    pub const DEFEND_MOTHERSHIP: u32 = 2;
    pub const FORMATION: u32 = 5;
    pub const TEMP_GUARD: u32 = 3;
    pub const GET_SHIPS: u32 = 4;
    pub const MOVE: u32 = 6;
    pub const MOVE_TO: u32 = 10;
    pub const ATTACK: u32 = 0x11;
    pub const FLANK_ATTACK: u32 = 0x13;
    pub const FANCY_GET_SHIPS: u32 = 0x16;
    pub const VAR_DEC: u32 = 0x1c;
    pub const ARMADA: u32 = 0x21;
    pub const HYPERSPACE: u32 = 0x2c;
}

/// Team flags (`teamFlags`).
pub mod team_flag {
    /// The team is to be destroyed.
    pub const DESTROY: u32 = 0x10;
    /// The team waits in the armada.
    pub const ARMADA: u32 = 0x1_0000;
    /// The team may hyperspace to its targets.
    pub const HYPERSPACE: u32 = 0x2_0000;
    /// The team waits in a temporary guard.
    pub const TEMP_GUARDING: u32 = 0x4_0000;
}

/// Formations (`TypeOfFormation`) the moves name: 7 none, 8 the one before.
pub const NO_FORMATION: i32 = 7;
pub const SAME_FORMATION: i32 = 8;

/// `teamDelay` by team level (0x740a18): turns between moves.
const TEAM_DELAY: [u8; 5] = [2, 1, 1, 0, 0];
/// `aitTeamIsFinishedMoving`'s range for `MOVE` (0x740aa4).
const MOVE_ARRIVED: f32 = 1300.0;
/// The distance of a flank from its targets (0x740ad8) and the moves of a flank attack
/// (formation 0x740adc, attack formation 0x740ae0).
const FLANK_DISTANCE: f32 = 6000.0;
const FLANK_MOVE_FORMATION: i32 = 4;
const FLANK_ATTACK_FORMATION: i32 = 1;
/// The share of the sphere of influence a temporary guard moves out (0x740b34) and its move's
/// formation and tactics (0x740b38, 0x740b3c).
const TEMP_GUARD_SHARE: f32 = f32::from_bits(0x3f2a_a64c);
const TEMP_GUARD_FORMATION: i32 = 1;
const TEMP_GUARD_TACTICS: i32 = 2;
/// Formation of a temporary guard's attack on threats (0x740b40).
const TEMP_GUARD_ATTACK_FORMATION: i32 = 1;

/// What a move does, with its parameters.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MoveKind {
    /// `MOVE_DONE`: the team has nothing left to do.
    Done,
    /// `MOVE_FANCYGETSHIPS`: `num` ships of `primary`, or alternatives each worth `ratio`
    /// tenths of one, from the new ships or built on request; `var` is the AI variable set
    /// when the requested ships have come.
    FancyGetShips { primary: ShipType, num: i32, alternatives: Vec<(ShipType, i32)>, random_alternative: bool, priority: u32, var: Option<String> },
    /// `MOVE_VARDEC`.
    VarDec { var: String },
    /// `MOVE_TEMPGUARD`: wait (moved out from the mothership) until the attack variable goes
    /// negative.
    TempGuard,
    /// `MOVE_ARMADA`: attack the armada's targets whenever there are some.
    Armada,
    /// `MOVE_ATTACK`.
    Attack { targets: Vec<ShipId> },
    /// `MOVE_FLANKATTACK`: move to a flank of the targets, then attack them.
    FlankAttack { targets: Vec<ShipId> },
    /// `MOVE_MOVE`.
    Move { destination: Vector },
    /// `MOVE_MOVETO`: move towards `destination` until `limit` from where the team started
    /// (`by_distance`) or for `limit` seconds.
    MoveTo { destination: Vector, limit: f32, by_distance: bool, start_time: f32, start: Vector },
    /// `MOVE_GUARDSHIPS`: protect the ships while any is left.
    GuardShips { ships: Vec<ShipId> },
    /// `MOVE_DEFMOSHIP`: attack the ships attacking the mothership while there are some.
    DefendMothership,
    /// `MOVE_FORMATION`.
    Formation { formation: i32 },
    /// `MOVE_COUNTSHIPS`: done at once (its process is an empty stub in Cataclysm).
    CountShips,
    /// `MOVE_ACTIVERECON`: wander the blobs, going out (towards home) or in, among the enemy
    /// blobs or all of them (+0x14, +0x18).
    ActiveRecon { out: bool, enemy: bool },
    /// `MOVE_SHIPRECON`: find the targets (+0x14); those found so far (+0x18).
    ShipRecon { targets: Vec<ShipId>, found: Vec<ShipId> },
    /// `MOVE_INTERCEPT`: fly to the target (+0x14), looking again every `wait` seconds
    /// (+0x18, until +0x1c) at the point flown to (+0x20) and whether it was moving (+0x2c).
    Intercept { target: Option<ShipId>, wait: f32, until: f32, point: Vector, moving: bool },
    /// `MOVE_HARASSATTACK`: find a harass target (+0x14) and go for it.
    HarassAttack { target: Option<ShipId> },
    /// `MOVE_ADVANCEDATTACK`: `aiuAttack` the targets, then wait until the ships stop.
    AdvancedAttack { targets: Vec<ShipId>, started: bool },
    /// `MOVE_VARSET`, `MOVE_VARWAIT` (until the variable has the value), `MOVE_VARDESTROY`.
    VarSet { var: String, value: i32 },
    VarWait { var: String, value: i32 },
    VarDestroy { var: String },
    /// `MOVE_GUARDCOOPTEAM`: guard the cooperating team's ships.
    GuardCooperatingTeam,
    /// `MOVE_ACTIVEPATROL`: patrol paths of the resourcers (kind 0) or support frigates (1).
    ActivePatrol { kind: u32 },
    /// `MOVE_PATROLMOVE`: a move to each point of the path, from `start`.
    PatrolMove { path: Vec<Vector>, start: usize },
    /// A patrol's move to one point (`MOVE_MOVE` numbered 7).
    PatrolPoint { destination: Vector },
    /// `MOVE_FLANKATTACK` with its hyperspace flag (+0x18): the team jumps to the flank.
    FlankAttackHyperspace { targets: Vec<ShipId> },
    /// `MOVE_HYPERSPACE` (0x2c): the team jumps to the destination (`clWrapMpHyperspace`),
    /// paid from the player's hyperspace savings.
    Hyperspace { destination: Vector },
}

/// The event handlers a move can carry (`aieHandlerSet*`, `aih*Handler`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Handler {
    /// `aihReconaissanceTeamDiedHandler`.
    Reconnaissance,
    /// `aihHarassTeamDiedHandler`.
    Harass,
    /// `aihHarassNumbersLowHandler`.
    HarassNumbersLow,
    /// `aihGenericGettingRockedHandler`.
    GenericGettingRocked,
    /// `aihHarassFiringSingleShipHandler`, `aihHarassDisengageSingleShipHandler`.
    HarassFiringSingleShip,
    HarassDisengageSingleShip,
    /// `aihKamikazeHealthLowHandler`.
    KamikazeHealthLow,
    /// `aihPatrolTeamDiedHandler`: the team is destroyed.
    Patrol,
    /// The roving defence's handlers (`aihFast/SlowDefense*Handler`).
    FastDefenseTeamDied,
    SlowDefenseTeamDied,
    FastDefenseNumbersLow,
    SlowDefenseNumbersLow,
    FastDefenseDistress,
    SlowDefenseDistress,
}

/// One event of a move (`aieHandlerSet*`): its handler, whether it fires only once and has
/// fired, and for health low and numbers low the percentage (+0xfc, +0x114), whether each ship
/// counts (+0x100) and the ship count it started from (+0x118).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub handler: Handler,
    pub once: bool,
    pub fired: bool,
    pub percent: i32,
    pub base: i32,
    pub per_ship: bool,
}

/// The events of a move (`AITeamMove` +0xcc..+0x154), copied to the moves it inserts.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Events {
    pub getting_rocked: Option<Event>,
    pub firing: Option<Event>,
    pub disengage: Option<Event>,
    pub health_low: Option<Event>,
    pub numbers_low: Option<Event>,
    /// The interrupt on the distress calls (+0x140..+0x150): the delay in turns as the
    /// percentage, the turns left as the base.
    #[serde(default, deserialize_with = "crate::save::added_in_18")]
    pub interrupt: Option<Event>,
}

/// One move (`AITeamMove`, 0x170 bytes).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AiMove {
    pub kind: MoveKind,
    /// +0x164 `processing`, +0x165 `wait`, +0x166 `remove`.
    pub processing: bool,
    pub wait: bool,
    pub remove: bool,
    /// +0x168 `formation`, +0x16c `tactics`.
    pub formation: i32,
    pub tactics: i32,
    /// +0x13c: called when the team has no ships left (`aieHandlerSetTeamDied`).
    #[serde(default, deserialize_with = "crate::save::added_in_15")]
    pub team_died: Option<Handler>,
    /// The other events (`aieExecute`).
    #[serde(default, deserialize_with = "crate::save::added_in_16")]
    pub events: Events,
}

impl AiMove {
    pub fn new(kind: MoveKind, formation: i32, tactics: i32, wait: bool, remove: bool) -> Self {
        AiMove { kind, processing: false, wait, remove, formation, tactics, team_died: None, events: Events::default() }
    }

    pub fn move_type(&self) -> u32 {
        match self.kind {
            MoveKind::Done => move_type::DONE,
            MoveKind::FancyGetShips { .. } => move_type::FANCY_GET_SHIPS,
            MoveKind::VarDec { .. } => move_type::VAR_DEC,
            MoveKind::TempGuard => move_type::TEMP_GUARD,
            MoveKind::Armada => move_type::ARMADA,
            MoveKind::Attack { .. } => move_type::ATTACK,
            MoveKind::FlankAttack { .. } => move_type::FLANK_ATTACK,
            MoveKind::Move { .. } => move_type::MOVE,
            MoveKind::MoveTo { .. } => move_type::MOVE_TO,
            MoveKind::GuardShips { .. } => move_type::GUARD_SHIPS,
            MoveKind::DefendMothership => move_type::DEFEND_MOTHERSHIP,
            MoveKind::Formation { .. } => move_type::FORMATION,
            MoveKind::CountShips => super::recon::move_type::COUNT_SHIPS,
            MoveKind::ActiveRecon { .. } => super::recon::move_type::ACTIVE_RECON,
            MoveKind::ShipRecon { .. } => super::recon::move_type::SHIP_RECON,
            MoveKind::Intercept { .. } => super::recon::move_type::INTERCEPT,
            MoveKind::HarassAttack { .. } => super::harass::move_type::HARASS_ATTACK,
            MoveKind::AdvancedAttack { .. } => super::harass::move_type::ADVANCED_ATTACK,
            MoveKind::VarSet { .. } => super::takeout::move_type::VAR_SET,
            MoveKind::VarWait { .. } => super::takeout::move_type::VAR_WAIT,
            MoveKind::VarDestroy { .. } => super::takeout::move_type::VAR_DESTROY,
            MoveKind::GuardCooperatingTeam => super::takeout::move_type::GUARD_COOPERATING_TEAM,
            MoveKind::ActivePatrol { .. } => super::roving::move_type::ACTIVE_PATROL,
            MoveKind::PatrolMove { .. } => super::roving::move_type::PATROL_MOVE,
            MoveKind::PatrolPoint { .. } => super::roving::move_type::PATROL_POINT,
            MoveKind::FlankAttackHyperspace { .. } => move_type::FLANK_ATTACK,
            MoveKind::Hyperspace { .. } => move_type::HYPERSPACE,
        }
    }
}

// ---- AI variables (AIVar.c) ------------------------------------------------------------------

/// `aivarLabelGenerate` + `aivarCreate`: a new variable with a unique name.
pub fn var_create(u: &mut Universe, value: i32) -> String {
    u.ai.next_label += 1;
    let name = format!("aivar{}", u.ai.next_label);
    u.ai.vars.push((name.clone(), value));
    name
}

/// `aivarValueGet(aivarFind(name))` (0 for a missing variable).
pub fn var_get(u: &Universe, name: &str) -> i32 {
    u.ai.vars.iter().find(|v| v.0 == name).map_or(0, |v| v.1)
}

/// `aivarValueSet(aivarFind(name))`; a missing variable is left missing.
pub fn var_set(u: &mut Universe, name: &str, value: i32) {
    if let Some(v) = u.ai.vars.iter_mut().find(|v| v.0 == name) {
        v.1 = value;
    }
}

pub fn var_destroy(u: &mut Universe, name: &str) {
    u.ai.vars.retain(|v| v.0 != name);
}

// ---- teams -----------------------------------------------------------------------------------

fn team(u: &Universe, t: usize) -> Option<&super::AiTeam> {
    u.ai.teams.get(t).and_then(|x| x.as_ref()).map(|x| &x.1)
}

fn team_mut(u: &mut Universe, t: usize) -> Option<&mut super::AiTeam> {
    u.ai.teams.get_mut(t).and_then(|x| x.as_mut()).map(|x| &mut x.1)
}

/// `listAddNode` of an `aimCreate*`: the move goes at the end of the team's list.
pub fn add_move(u: &mut Universe, t: usize, m: AiMove) {
    if let Some(tm) = team_mut(u, t) {
        tm.moves.push(m);
    }
}

/// `aitAddmoveBeforeAndMakeCurrent`: `m` goes before the current move and becomes current,
/// setting the team's tactics and formation when they change.
pub(super) fn add_move_before_and_make_current(u: &mut Universe, t: usize, mut m: AiMove) {
    let Some(tm) = team(u, t) else { return };
    let Some(c) = tm.current else { return };
    let (cur_tactics, cur_formation) = (tm.moves[c].tactics, tm.moves[c].formation);
    let ships = tm.ships.clone();
    if m.tactics != cur_tactics && !ships.is_empty() {
        wrap_set_tactics(u, &ships, m.tactics);
    }
    if m.formation == SAME_FORMATION {
        m.formation = cur_formation;
    } else if m.formation != NO_FORMATION && m.formation != cur_formation && ships.len() > 1 {
        wrap_formation(u, &ships, m.formation);
    }
    let tm = team_mut(u, t).expect("team");
    tm.moves.insert(c, m);
    tm.current = Some(c);
}

/// `aimInsertMove` (0x40e630): the move goes before the current one and becomes current (the
/// current one, unless it is getting ships, starts over afterwards); with no current move it
/// goes first.
pub fn insert_move(u: &mut Universe, t: usize, m: AiMove) {
    let Some(tm) = team_mut(u, t) else { return };
    match tm.current {
        None => {
            tm.moves.insert(0, m);
            tm.current = Some(0);
        }
        Some(c) => {
            if !matches!(tm.moves[c].move_type(), move_type::GET_SHIPS | move_type::FANCY_GET_SHIPS) {
                tm.moves[c].processing = false;
            }
            add_move_before_and_make_current(u, t, m);
        }
    }
}

/// An attack on `targets` inserted before the current move and made current (as the guard,
/// armada and defence moves do).
pub fn insert_attack_before(u: &mut Universe, t: usize, targets: Vec<ShipId>, formation: i32) {
    add_move_before_and_make_current(u, t, AiMove::new(MoveKind::Attack { targets }, formation, 2, true, true));
}

/// `aitDeleteAllTeamMoves`.
pub fn delete_all_moves(u: &mut Universe, t: usize) {
    let closed: Vec<AiMove> = match team_mut(u, t) {
        Some(tm) => {
            tm.current = None;
            std::mem::take(&mut tm.moves)
        }
        None => return,
    };
    for m in closed {
        close_move(u, m);
    }
}

/// `aitDeleteMovesUntilMoveType`: the moves from the current one up to one of type `ty` go;
/// that one becomes current (none when there is none).
pub fn delete_moves_until(u: &mut Universe, t: usize, ty: u32) {
    let Some(tm) = team_mut(u, t) else { return };
    let Some(c) = tm.current else { return };
    let mut i = c;
    while i + 1 < tm.moves.len() && tm.moves[i].move_type() != ty {
        i += 1;
    }
    if tm.moves[i].move_type() == ty {
        tm.moves.drain(c..i);
        tm.current = Some(c);
    } else {
        tm.moves.drain(c..i);
        tm.current = None;
    }
}

/// A move's close function (`aimCloseGetShips` frees its variable).
fn close_move(u: &mut Universe, m: AiMove) {
    if let MoveKind::FancyGetShips { var: Some(v), .. } = m.kind {
        var_destroy(u, &v);
    }
}

/// `aitTeamIsDone`: no current move, or it is `MOVE_DONE`.
pub fn team_is_done(u: &Universe, t: usize) -> bool {
    team(u, t).is_none_or(|tm| tm.current.is_none_or(|c| tm.moves[c].move_type() == move_type::DONE))
}

/// `aitCountTeamsWaitingForShips`: the player's teams of the manager (all with `None`) that
/// have no move yet or are getting ships.
pub fn count_teams_waiting_for_ships(u: &Universe, p: usize, manager: Option<Manager>) -> usize {
    u.ai.teams
        .iter()
        .flatten()
        .filter(|(o, tm)| *o == p && manager.is_none_or(|m| tm.manager == m))
        .filter(|(_, tm)| tm.current.is_none_or(|c| matches!(tm.moves[c].move_type(), move_type::GET_SHIPS | move_type::FANCY_GET_SHIPS)))
        .count()
}

/// `aitFindNumTeamsWithFlag`.
pub fn num_teams_with_flag(u: &Universe, p: usize, flag: u32) -> usize {
    super::teams_with_flag(u, p, flag).len()
}

/// `aitDestroy` (0x417220) with the ships given back (`aifTeamDied`): the team's ships wait to
/// be handed out again, its moves are closed, its requests forgotten and its manager's slot
/// freed.
pub fn destroy(u: &mut Universe, t: usize) {
    delete_all_moves(u, t);
    let Some(Some((p, tm))) = u.ai.teams.get_mut(t).map(Option::take) else { return };
    if let Some(Some(a)) = u.ai.players.get_mut(p) {
        for id in tm.ships {
            if !a.new_ships.contains(&id) {
                a.new_ships.push(id);
            }
        }
        for w in &mut a.waiting {
            w.retain(|w| w.team != t);
        }
        super::attack::team_died(a, t);
        super::defense::team_died(a, t);
    }
}

/// `aitExecute` (0x417c00): each of the player's teams runs its current move after its delay;
/// a finished move hands over to the next with its tactics and formation.
pub fn execute(u: &mut Universe, p: usize) {
    let ids: Vec<usize> = u.ai.teams.iter().enumerate().filter_map(|(i, t)| t.as_ref().filter(|t| t.0 == p).map(|_| i)).collect();
    for t in ids {
        events(u, t);
        let Some(tm) = team(u, t) else { continue };
        if tm.flags & team_flag::DESTROY != 0 {
            destroy(u, t);
            continue;
        }
        if tm.current.is_none() {
            if tm.moves.is_empty() {
                continue;
            }
            team_mut(u, t).expect("team").current = Some(0);
        }
        let tm = team(u, t).expect("team");
        let c = tm.current.expect("current");
        if tm.moves[c].move_type() == move_type::DONE {
            continue;
        }
        if tm.delay != 0 {
            team_mut(u, t).expect("team").delay -= 1;
            continue;
        }
        if !process(u, t) {
            continue;
        }
        let Some(tm) = team(u, t) else { continue };
        if tm.flags & team_flag::DESTROY != 0 {
            destroy(u, t);
            continue;
        }
        // The move that finished is the current one (a move inserted before it while
        // processing became current instead and did not finish).
        let Some(c) = tm.current else { continue };
        let (old_formation, old_tactics, old_remove) = (tm.moves[c].formation, tm.moves[c].tactics, tm.moves[c].remove);
        let features = tm.features;
        let ships = tm.ships.clone();
        let level = tm.level;
        if c + 1 < tm.moves.len() {
            let next = &tm.moves[c + 1];
            let (nt, nf) = (next.tactics, next.formation);
            if features & 8 != 0 && nt != old_tactics && !ships.is_empty() {
                wrap_set_tactics(u, &ships, nt);
            }
            if nf == SAME_FORMATION {
                team_mut(u, t).expect("team").moves[c + 1].formation = old_formation;
            } else if nf != NO_FORMATION && nf != old_formation && ships.len() > 1 {
                wrap_formation(u, &ships, nf);
            }
            let tm = team_mut(u, t).expect("team");
            tm.current = Some(c + 1);
            tm.delay = TEAM_DELAY[usize::from(level).min(4)];
        } else {
            team_mut(u, t).expect("team").current = None;
        }
        if old_remove {
            let tm = team_mut(u, t).expect("team");
            let m = tm.moves.remove(c);
            if let Some(cur) = tm.current.as_mut() {
                if *cur > c {
                    *cur -= 1;
                }
            }
            close_move(u, m);
        }
    }
}

/// `aieExecute` (0x403a70), the events ported: a team with no ships left calls its current
/// move's team-died handler.
fn events(u: &mut Universe, t: usize) {
    let Some(tm) = team(u, t) else { return };
    let Some(c) = tm.current else { return };
    let m = &tm.moves[c];
    if m.move_type() == move_type::DONE {
        return;
    }
    let ships = tm.ships.clone();
    if let (Some(h), true) = (m.team_died, ships.is_empty()) {
        super::harass::handle(u, t, h, None);
        return;
    }
    let ev = m.events.clone();
    let open = |e: &Option<Event>| e.filter(|e| !e.once || !e.fired);
    let set_fired = |u: &mut Universe, f: fn(&mut Events) -> &mut Option<Event>| {
        if let Some(e) = f(&mut team_mut(u, t).expect("team").moves[c].events).as_mut() {
            e.fired = true;
        }
    };
    if let Some(e) = open(&ev.getting_rocked) {
        let rocker = ships.iter().find_map(|&id| {
            let s = u.ships.get(id)?;
            let r = s.getting_rocked?;
            (u.ships.get(r)?.player != s.player).then_some(r)
        });
        if let Some(r) = rocker {
            set_fired(u, |e| &mut e.getting_rocked);
            super::harass::handle(u, t, e.handler, Some(r));
            return;
        }
    }
    let attacking = |u: &Universe| ships.iter().any(|&id| u.ships.get(id).is_some_and(|s| s.ship_is_attacking));
    if let Some(e) = open(&ev.firing) {
        if attacking(u) {
            set_fired(u, |e| &mut e.firing);
            super::harass::handle(u, t, e.handler, None);
            return;
        }
    }
    if let (Some(e), true) = (open(&ev.disengage), ev.firing.is_some_and(|f| f.fired)) {
        if !attacking(u) {
            set_fired(u, |e| &mut e.disengage);
            super::harass::handle(u, t, e.handler, None);
            return;
        }
    }
    if let Some(e) = open(&ev.health_low) {
        if super::harass::health_low(u, &ships, e.percent, e.per_ship) {
            set_fired(u, |e| &mut e.health_low);
            super::harass::handle(u, t, e.handler, None);
            return;
        }
    }
    if let Some(e) = open(&ev.numbers_low) {
        let n = ships.len() as i32;
        if e.base == 0 {
            if let Some(x) = team_mut(u, t).expect("team").moves[c].events.numbers_low.as_mut() {
                x.base = n;
            }
        } else if n * 100 < e.percent * e.base {
            set_fired(u, |e| &mut e.numbers_low);
            super::harass::handle(u, t, e.handler, None);
            return;
        }
    }
    let p = u.ai.teams[t].as_ref().expect("team").0;
    if let Some(e) = open(&ev.interrupt) {
        if !super::ai(u, p).defense.distress.is_empty() {
            if e.percent == 0 || e.base == 0 {
                // (The original resets the count after the handler, on the same move.)
                if let Some(x) = team_mut(u, t).expect("team").moves[c].events.interrupt.as_mut() {
                    x.fired = true;
                    x.base = e.percent;
                }
                super::harass::handle(u, t, e.handler, None);
            } else if let Some(x) = team_mut(u, t).expect("team").moves[c].events.interrupt.as_mut() {
                x.base -= 1;
            }
        }
    }
}

/// `aitDeleteCurrentMove` (0x417990): the current move is closed and goes; the next is current.
pub(super) fn delete_current_move(u: &mut Universe, t: usize) {
    let Some(tm) = team_mut(u, t) else { return };
    let Some(c) = tm.current else { return };
    let m = tm.moves.remove(c);
    tm.current = (c < tm.moves.len()).then_some(c);
    close_move(u, m);
}

pub(super) fn wrap_formation_pub(u: &mut Universe, sel: &[ShipId], formation: i32) {
    wrap_formation(u, sel, formation);
}

pub(super) fn ships_no_longer_attacking_pub(u: &Universe, sel: &[ShipId]) -> bool {
    ships_no_longer_attacking(u, sel)
}

/// The current move's process function; true when it is done.
fn process(u: &mut Universe, t: usize) -> bool {
    let tm = team(u, t).expect("team");
    let c = tm.current.expect("current");
    match tm.moves[c].kind.clone() {
        MoveKind::Done => false,
        MoveKind::FancyGetShips { .. } => process_fancy_get_ships(u, t, c),
        MoveKind::VarDec { var } => {
            if !team(u, t).expect("team").moves[c].processing {
                let v = var_get(u, &var);
                var_set(u, &var, v - 1);
                team_mut(u, t).expect("team").moves[c].processing = true;
            }
            true
        }
        MoveKind::TempGuard => process_temp_guard(u, t, c),
        MoveKind::Armada => process_armada(u, t, c),
        MoveKind::Attack { .. } => process_attack(u, t, c),
        MoveKind::FlankAttack { targets } => process_flank_attack(u, t, c, targets, false),
        MoveKind::FlankAttackHyperspace { targets } => process_flank_attack(u, t, c, targets, true),
        MoveKind::Hyperspace { destination } => process_hyperspace(u, t, c, destination),
        MoveKind::Move { destination } => process_move(u, t, c, destination),
        MoveKind::MoveTo { .. } => process_move_to(u, t, c),
        MoveKind::GuardShips { .. } => process_guard_ships(u, t, c),
        MoveKind::DefendMothership => {
            let p = u.ai.teams[t].as_ref().expect("team").0;
            super::defense::process_defend_mothership(u, p, t)
        }
        MoveKind::Formation { formation } => {
            let ships = team(u, t).expect("team").ships.clone();
            if ships.is_empty() || wrap_filter(u, &ships).is_empty() {
                return false;
            }
            wrap_formation(u, &ships, formation);
            set_processing(u, t, c, true);
            true
        }
        MoveKind::CountShips => true,
        MoveKind::ActiveRecon { .. } => super::recon::process_active_recon(u, t, c),
        MoveKind::ShipRecon { .. } => super::recon::process_ship_recon(u, t, c),
        MoveKind::Intercept { .. } => super::recon::process_intercept(u, t, c),
        MoveKind::HarassAttack { .. } => super::harass::process_harass_attack(u, t, c),
        MoveKind::AdvancedAttack { .. } => super::harass::process_advanced_attack(u, t, c),
        MoveKind::VarSet { var, value } => {
            if !team(u, t).expect("team").moves[c].processing {
                var_set(u, &var, value);
                set_processing(u, t, c, true);
            }
            true
        }
        MoveKind::VarWait { var, value } => {
            set_processing(u, t, c, true);
            if u.ai.vars.iter().any(|v| v.0 == var) && var_get(u, &var) == value {
                return true;
            }
            !team(u, t).expect("team").moves[c].wait
        }
        MoveKind::VarDestroy { var } => {
            if !team(u, t).expect("team").moves[c].processing {
                var_destroy(u, &var);
                set_processing(u, t, c, true);
            }
            true
        }
        MoveKind::ActivePatrol { .. } => super::roving::process_active_patrol(u, t, c),
        MoveKind::PatrolMove { .. } => super::roving::process_patrol_move(u, t, c),
        MoveKind::PatrolPoint { destination } => process_move(u, t, c, destination),
        MoveKind::GuardCooperatingTeam => {
            let done = super::takeout::process_guard_cooperating_team(u, t);
            if done {
                set_processing(u, t, c, true);
            }
            done
        }
    }
}

fn set_processing(u: &mut Universe, t: usize, c: usize, v: bool) {
    if let Some(tm) = team_mut(u, t) {
        tm.moves[c].processing = v;
    }
}

/// `aimProcessFancyGetShips` (0x40da40): the first time, ships of the wanted types are taken
/// from the new ships (`num * 10` tenths, an alternative counting its ratio); what is still
/// missing is requested, from a random alternative or the primary type, as the unit caps
/// allow. The move is done when the variable of the request is set (the ships have come).
fn process_fancy_get_ships(u: &mut Universe, t: usize, c: usize) -> bool {
    let tm = team(u, t).expect("team");
    let MoveKind::FancyGetShips { primary, num, alternatives, random_alternative, priority, var } = tm.moves[c].kind.clone() else { return true };
    if tm.moves[c].processing {
        return var.is_some_and(|v| var_get(u, &v) != 0);
    }
    let p = u.ai.teams[t].as_ref().expect("team").0;
    let script = tm.manager == Manager::Script;
    let v = var_create(u, 0);
    if let MoveKind::FancyGetShips { var, .. } = &mut team_mut(u, t).expect("team").moves[c].kind {
        *var = Some(v.clone());
    }
    let take = |u: &mut Universe, ty: ShipType| -> bool {
        let a = ai(u, p);
        let Some(i) = a.new_ships.iter().position(|&id| u.ships.get(id).is_some_and(|s| s.ship_type == ty)) else { return false };
        let id = ai_mut(u, p).new_ships.remove(i);
        super::team_add_ship(u, t, id);
        true
    };
    let mut need = num * 10;
    while need > 0 && take(u, primary) {
        need -= 10;
    }
    for &(alt, ratio) in &alternatives {
        if need <= 0 {
            break;
        }
        while need > 0 && take(u, alt) {
            need -= ratio;
        }
    }
    if need <= 0 {
        var_set(u, &v, 1);
        set_processing(u, t, c, true);
        return true;
    }
    let n_alts = alternatives.len();
    if random_alternative {
        let r = u.randy.random(hwc_math::random::RAN_AI_PLAYER) as usize % (n_alts + 1);
        if r != 0 {
            let mut k = r - 1;
            for _ in 0..n_alts {
                let (alt, ratio) = alternatives[k];
                if can_build_ship_type(u, p, alt, script) {
                    let n = ((need as u32) / ratio.max(1) as u32).max(1) as i32;
                    let ok = super::unit_cap_can_build(u, p, alt, n);
                    if ok == n {
                        team_requests_ships(u, p, t, alt, n, &v, priority);
                        set_processing(u, t, c, true);
                        return false;
                    }
                    if (n as f64) * 0.25 < f64::from(ok) {
                        need = (n - ok) * ratio;
                        team_requests_ships(u, p, t, alt, ok, &v, priority);
                    }
                }
                k += 1;
                if k >= n_alts {
                    k = 0;
                }
            }
        }
    }
    if !can_build_ship_type(u, p, primary, script) {
        for &(alt, ratio) in &alternatives {
            if !can_build_ship_type(u, p, alt, script) {
                continue;
            }
            let n = ((need as u32) / ratio.max(1) as u32).max(1) as i32;
            let ok = super::unit_cap_can_build(u, p, alt, n);
            if ok == n {
                team_requests_ships(u, p, t, alt, n, &v, priority);
                set_processing(u, t, c, true);
                return false;
            }
            if (n as f64) * 0.75 < f64::from(ok) {
                need = (ok - n) * 10;
                team_requests_ships(u, p, t, alt, ok, &v, priority);
            }
        }
        // (The team gives up: it is destroyed, its requests left in the queues.)
        var_destroy(u, &v);
        let tm = team_mut(u, t).expect("team");
        if let MoveKind::FancyGetShips { var, .. } = &mut tm.moves[c].kind {
            *var = None;
        }
        tm.flags |= team_flag::DESTROY;
        return true;
    }
    let n = (need / 10).max(1);
    let ok = super::unit_cap_can_build(u, p, primary, n);
    if ok == n {
        team_requests_ships(u, p, t, primary, n, &v, priority);
    } else if f64::from(ok) <= f64::from(n) * 0.75 {
        // Too few may be built: the move is done if the team already has enough ships of a
        // wanted type, else it tries again next turn.
        var_destroy(u, &v);
        let tm = team_mut(u, t).expect("team");
        if let MoveKind::FancyGetShips { var, .. } = &mut tm.moves[c].kind {
            *var = None;
        }
        let tm = team(u, t).expect("team");
        let count = tm.ships.len() as i32;
        if let Some(first) = tm.ships.first().and_then(|&id| u.ships.get(id)).map(|s| s.ship_type) {
            if first == primary && num <= count {
                return true;
            }
            for &(alt, ratio) in &alternatives {
                if first == alt && (num * 10) / ratio.max(1) <= count {
                    return true;
                }
            }
        }
        return false;
    } else {
        team_requests_ships(u, p, t, primary, ok, &v, priority);
    }
    set_processing(u, t, c, true);
    var_get(u, &v) != 0
}

/// `aiuCanBuildShipType` (0x41e470): the player has a mothership, the type is its race's with
/// its statics loaded (not a resourcer), and the mothership can build it (script teams skip
/// that test).
pub fn can_build_ship_type(u: &Universe, p: usize, ty: ShipType, script: bool) -> bool {
    let Some(ms) = u.base.players[p].mothership.filter(|&m| u.ships.get(m).is_some_and(|s| s.flags & sof::DEAD == 0)) else { return false };
    let race = u.players[p].race;
    if !crate::research::race_types(race).contains(&ty.index()) {
        return false;
    }
    let Some(st) = u.statics.ship(ty) else { return false };
    if !st.info_needed || (st.ship_class == Some(crate::statics::ShipClass::Resource) && !u.single_player) {
        return false;
    }
    script || crate::construction::can_build_ship_type(u, ms, ty, false)
}

/// `aifTeamRequestsShipsCB` (0x404d00): a request for the team's manager (high priority ones
/// first), and the team waiting for them with the variable to set.
pub fn team_requests_ships(u: &mut Universe, p: usize, t: usize, ty: ShipType, num: i32, var: &str, priority: u32) {
    let Some(manager) = team(u, t).map(|tm| tm.manager) else { return };
    let q = match manager {
        Manager::Attack => 0,
        Manager::Defense => 1,
        Manager::Script => 2,
        Manager::Resource => return,
    };
    let a = ai_mut(u, p);
    let creator = if q == 2 { a.script_creator } else { a.ai_creator };
    let r = super::Request { ship_type: ty, num, priority, creator };
    let w = super::TeamWaiting { ship_type: ty, num, team: t, done_var: var.to_string() };
    if priority & 1 != 0 {
        a.requests[q].insert(0, r);
        a.waiting[q].insert(0, w);
    } else {
        a.requests[q].push(r);
        a.waiting[q].push(w);
    }
}

/// `aimProcessGuardShips` (0x40ad30): the team protects the ships, and is done when they are
/// all gone.
fn process_guard_ships(u: &mut Universe, t: usize, c: usize) -> bool {
    let tm = team(u, t).expect("team");
    let team_ships = tm.ships.clone();
    if team_ships.is_empty() {
        return false;
    }
    let MoveKind::GuardShips { ships } = tm.moves[c].kind.clone() else { return true };
    let processing = tm.moves[c].processing;
    let ships: Vec<ShipId> = ships.into_iter().filter(|&id| u.ships.get(id).is_some_and(|s| s.flags & sof::DEAD == 0)).collect();
    if !processing {
        if !ships.is_empty() {
            let ok = wrap_filter(u, &team_ships);
            if ok.is_empty() {
                return false;
            }
            command::cl_protect(u, &ok, &ships);
        }
        set_processing(u, t, c, true);
        return false;
    }
    ships.is_empty()
}

/// `aimProcessTempGuard` (0x409d00): while the attack variable is not negative the team waits;
/// the first time it moves out towards the middle of the map, two thirds of the way across
/// the mothership's sphere of influence. When the variable goes negative the move is done.
fn process_temp_guard(u: &mut Universe, t: usize, c: usize) -> bool {
    let p = u.ai.teams[t].as_ref().expect("team").0;
    let tm = team_mut(u, t).expect("team");
    if tm.ships.is_empty() {
        tm.flags &= !team_flag::TEMP_GUARDING;
        return true;
    }
    tm.flags |= team_flag::TEMP_GUARDING;
    let var = ai(u, p).attack.var.clone();
    if var_get(u, &var) < 0 {
        team_mut(u, t).expect("team").flags &= !team_flag::TEMP_GUARDING;
        return true;
    }
    if !team(u, t).expect("team").moves[c].processing {
        if u.base.players[p].mothership.is_some() {
            let limit = st(ld(ai(u, p).attack.sphere_of_influence).sqrt() * ld(TEMP_GUARD_SHARE));
            let m = AiMove::new(
                MoveKind::MoveTo { destination: Vector::ZERO, limit, by_distance: true, start_time: 0.0, start: Vector::ZERO },
                TEMP_GUARD_FORMATION,
                TEMP_GUARD_TACTICS,
                true,
                true,
            );
            add_move_before_and_make_current(u, t, m);
            // The guard move is now after the new one.
            let tm = team_mut(u, t).expect("team");
            let cur = tm.current.expect("current");
            tm.moves[cur + 1].processing = true;
            return false;
        }
        set_processing(u, t, c, true);
        return false;
    }
    // Capital ships threatening the mothership (the defence manager's sphere of influence) are
    // attacked; the guard starts over afterwards.
    let threats: Vec<ShipId> = ai(u, p).defense.threats.iter().copied().filter(|&id| u.ships.get(id).and_then(|s| u.statics.ship(s.ship_type)).is_some_and(|st| st.is_capital_ship)).collect();
    if !threats.is_empty() {
        insert_attack_before(u, t, threats, TEMP_GUARD_ATTACK_FORMATION);
        let tm = team_mut(u, t).expect("team");
        let cur = tm.current.expect("current");
        tm.moves[cur + 1].processing = false;
    }
    // (The ships to rescue are the HW1 races'.)
    false
}

/// `aimProcessArmada` (0x40aa60): the team joins the armada and, whenever the armada has
/// targets, attacks them (flanking them when the team can).
fn process_armada(u: &mut Universe, t: usize, c: usize) -> bool {
    let p = u.ai.teams[t].as_ref().expect("team").0;
    let tm = team_mut(u, t).expect("team");
    if tm.ships.is_empty() {
        return true;
    }
    if tm.moves[c].processing {
        if !ai(u, p).attack.armada_targets.is_empty() {
            set_processing(u, t, c, false);
        }
        return false;
    }
    tm.flags |= team_flag::ARMADA;
    let features = tm.features;
    let targets = ai(u, p).attack.armada_targets.clone();
    if targets.is_empty() {
        return false;
    }
    // A team that may jump (and has ships that can, and the savings are there) jumps to its
    // flank.
    let flags = team(u, t).expect("team").flags;
    let ships = team(u, t).expect("team").ships.clone();
    let jump = flags & team_flag::HYPERSPACE != 0 && !crate::mp_hyperspace::hyperspace_capable(u, &ships).is_empty() && 0 < ai(u, p).hyper_savings;
    let kind = match (features & 2 != 0, jump) {
        (true, true) => MoveKind::FlankAttackHyperspace { targets },
        (true, false) => MoveKind::FlankAttack { targets },
        (false, _) => MoveKind::Attack { targets },
    };
    add_move_before_and_make_current(u, t, AiMove::new(kind, SAME_FORMATION, 2, true, true));
    let tm = team_mut(u, t).expect("team");
    let cur = tm.current.expect("current");
    tm.moves[cur + 1].processing = true;
    false
}

/// `aimProcessAttack` (0x40a460): the team attacks, then (waiting) the move lasts until no ship
/// of the team is attacking any more or the targets are gone.
fn process_attack(u: &mut Universe, t: usize, c: usize) -> bool {
    let tm = team(u, t).expect("team");
    let ships = tm.ships.clone();
    if ships.is_empty() {
        return true;
    }
    let MoveKind::Attack { mut targets } = tm.moves[c].kind.clone() else { return true };
    targets.retain(|&id| u.ships.get(id).is_some_and(|s| s.flags & sof::DEAD == 0));
    if !tm.moves[c].processing {
        let wait = tm.moves[c].wait;
        if !targets.is_empty() {
            wrap_attack(u, &ships, &targets);
        }
        let tm = team_mut(u, t).expect("team");
        tm.moves[c].processing = true;
        tm.moves[c].kind = MoveKind::Attack { targets };
        return !wait;
    }
    if !targets.is_empty() && !ships_no_longer_attacking(u, &ships) {
        team_mut(u, t).expect("team").moves[c].kind = MoveKind::Attack { targets };
        return false;
    }
    team_mut(u, t).expect("team").moves[c].kind = MoveKind::Attack { targets: Vec::new() };
    true
}

/// `aimProcessFlankAttack` (0x408640): a move to a point to one side of the targets, then the
/// attack, go before this move, which is then done.
fn process_flank_attack(u: &mut Universe, t: usize, c: usize, targets: Vec<ShipId>, hyperspace: bool) -> bool {
    let tm = team(u, t).expect("team");
    if tm.ships.is_empty() || tm.moves[c].processing {
        return true;
    }
    let team_centre = centre(u, &tm.ships);
    let target_centre = centre(u, &targets);
    let flank = flank_coordinates(u, target_centre, team_centre, FLANK_DISTANCE);
    let to_flank = if hyperspace { MoveKind::Hyperspace { destination: flank } } else { MoveKind::Move { destination: flank } };
    add_move_before_and_make_current(u, t, AiMove::new(to_flank, FLANK_MOVE_FORMATION, 2, true, true));
    let tm = team(u, t).expect("team");
    let tactics = if u.single_player && tm.manager == Manager::Script { tm.tactics } else { 2 };
    // Teams with feature 1 attack by type (`aimProcessAdvancedAttack`).
    let attack = if tm.features & 1 != 0 { MoveKind::AdvancedAttack { targets, started: false } } else { MoveKind::Attack { targets } };
    // The attack goes between the move and this move.
    let tm = team_mut(u, t).expect("team");
    let cur = tm.current.expect("current");
    tm.moves.insert(cur + 1, AiMove::new(attack, FLANK_ATTACK_FORMATION, tactics, true, true));
    tm.moves[cur + 2].processing = true;
    false
}

/// `aimProcessHyperspace` (0x407e80): the team's ships that can jump do, from their centre to
/// the destination, the cost coming off the hyperspace savings; done once the first ship is
/// selectable again.
fn process_hyperspace(u: &mut Universe, t: usize, c: usize, destination: Vector) -> bool {
    let tm = team(u, t).expect("team");
    let ships = tm.ships.clone();
    if ships.is_empty() {
        return true;
    }
    if !tm.moves[c].processing {
        let p = u.ai.teams[t].as_ref().expect("team").0;
        // `aiuFilterSelectableShips`.
        let selectable: Vec<ShipId> = ships
            .iter()
            .copied()
            .filter(|&id| u.ships.get(id).is_some_and(|s| s.flags & (0xa0_0000 | sof::HYPERSPACE) == 0 && s.special_flags2 & 0x8 == 0))
            .collect();
        let sel = crate::mp_hyperspace::hyperspace_capable(u, &selectable);
        if !sel.is_empty() {
            let from = centre(u, &sel);
            let d = st(crate::level::distance_squared(from, destination).sqrt());
            let cost = crate::mp_hyperspace::hyperspace_cost(u, d, &sel) as i32;
            super::ai_mut(u, p).hyper_savings -= cost;
            crate::mp_hyperspace::cl_mp_hyperspace(u, &sel, from, destination);
        }
        set_processing(u, t, c, true);
        return false;
    }
    u.ships.get(ships[0]).is_none_or(|s| s.flags & sof::SELECTABLE != 0)
}

/// `aiuGenerateFlankCoordinates` (0x41e5d0): `distance` from `a`, square to the line from `a`
/// to `b` and to a random direction.
fn flank_coordinates(u: &mut Universe, a: Vector, b: Vector, distance: f32) -> Vector {
    let mut r = || st(ld(u.randy.random(hwc_math::random::RAN_AI_PLAYER) as f32) * ld(f32::from_bits(0x31a0_0000)) - 10.0);
    let up = Vector::new(r(), r(), r());
    let d = Vector::new(b.x - a.x, b.y - a.y, b.z - a.z);
    let n = Vector::new(st(ld(d.y) * ld(up.z) - ld(d.z) * ld(up.y)), st(ld(d.z) * ld(up.x) - ld(d.x) * ld(up.z)), st(ld(d.x) * ld(up.y) - ld(d.y) * ld(up.x)));
    let len = (ld(n.x) * ld(n.x) + ld(n.y) * ld(n.y) + ld(n.z) * ld(n.z)).sqrt();
    let n = if len > 0.0 { Vector::new(st(ld(n.x) / len), st(ld(n.y) / len), st(ld(n.z) / len)) } else { n };
    Vector::new(st(ld(n.x) * ld(distance) + ld(a.x)), st(ld(n.y) * ld(distance) + ld(a.y)), st(ld(n.z) * ld(distance) + ld(a.z)))
}

/// `aimProcessMove` (0x407f50).
fn process_move(u: &mut Universe, t: usize, c: usize, destination: Vector) -> bool {
    let tm = team(u, t).expect("team");
    let ships = tm.ships.clone();
    if ships.is_empty() {
        return true;
    }
    let wait = tm.moves[c].wait;
    if !tm.moves[c].processing {
        if !wrap_move(u, &ships, destination) {
            return false;
        }
        set_processing(u, t, c, true);
    } else if team_finished_moving(u, &ships, destination, MOVE_ARRIVED) {
        return true;
    }
    !wait
}

/// `aimProcessMoveTo` (0x408350): the team moves; it halts after the time limit, or once its
/// centre is `limit` from where it started.
fn process_move_to(u: &mut Universe, t: usize, c: usize) -> bool {
    let tm = team(u, t).expect("team");
    let ships = tm.ships.clone();
    if ships.is_empty() {
        return true;
    }
    let MoveKind::MoveTo { destination, limit, by_distance, start_time, start } = tm.moves[c].kind.clone() else { return true };
    if !tm.moves[c].processing {
        if !ships.iter().any(|&id| u.ships.get(id).is_some_and(|s| s.flags & sof::HIDE != 0)) {
            wrap_move(u, &ships, destination);
            let now = u.total_time_elapsed;
            let start = centre(u, &ships);
            let tm = team_mut(u, t).expect("team");
            tm.moves[c].kind = MoveKind::MoveTo { destination, limit, by_distance, start_time: now, start };
            tm.moves[c].processing = true;
        }
        return false;
    }
    let done = if by_distance {
        let now = centre(u, &ships);
        ld(limit) < crate::level::distance_squared(now, start).sqrt()
    } else {
        ld(limit) < ld(u.total_time_elapsed) - ld(start_time)
    };
    if done {
        wrap_halt(u, &ships);
    }
    done
}

// ---- wrappers (AIUtilities.c) ----------------------------------------------------------------

/// The test of every `aiuWrap*`: not crazy or disabled, not in hyperspace, not being built.
pub(super) fn wrap_ok_pub(u: &Universe, id: ShipId) -> bool {
    wrap_ok(u, id)
}

fn wrap_ok(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| s.flags & (sof::CRAZY | sof::DISABLED | sof::HYPERSPACE | sof::DEAD) == 0 && s.special_flags2 & 0x8 == 0)
}

fn wrap_filter(u: &Universe, sel: &[ShipId]) -> Vec<ShipId> {
    sel.iter().copied().filter(|&id| wrap_ok(u, id)).collect()
}

/// `aiuWrapMove` (0x41eeb0): the fit, mobile ships move from the first ship's position.
pub fn wrap_move(u: &mut Universe, sel: &[ShipId], dest: Vector) -> bool {
    let Some(&first) = sel.first() else { return false };
    let mut ok = wrap_filter(u, sel);
    if ok.is_empty() {
        return false;
    }
    ok.retain(|&id| !u.ships.get(id).and_then(|s| u.statics.ship(s.ship_type)).is_some_and(|st| st.header.immobile));
    let from = u.ships.get(first).map_or(Vector::ZERO, |s| s.posinfo.position);
    if !ok.is_empty() {
        command::cl_move(u, &ok, command::waypoint_mode::ONCE, from, &[dest]);
    }
    true
}

/// `aiuWrapAttack` (0x41ed40).
pub fn wrap_attack(u: &mut Universe, sel: &[ShipId], targets: &[ShipId]) {
    let ok = wrap_filter(u, sel);
    if !ok.is_empty() {
        command::cl_attack(u, &ok, targets);
    }
}

fn wrap_halt(u: &mut Universe, sel: &[ShipId]) {
    let ok = wrap_filter(u, sel);
    if !ok.is_empty() {
        command::cl_halt(u, &ok);
    }
}

fn wrap_set_tactics(u: &mut Universe, sel: &[ShipId], tactics: i32) {
    let ok = wrap_filter(u, sel);
    if !ok.is_empty() {
        command::cl_set_tactics(u, &ok, tactics.max(0) as usize);
    }
}

fn wrap_formation(u: &mut Universe, sel: &[ShipId], formation: i32) {
    let ok = wrap_filter(u, sel);
    if !ok.is_empty() {
        command::cl_formation(u, &ok, formation.max(0) as u32);
    }
}

/// `aiuShipsNoLongerAttacking` (0x41e0f0): no ship of the selection has an attack order.
fn ships_no_longer_attacking(u: &Universe, sel: &[ShipId]) -> bool {
    !sel.iter().any(|&id| u.ships.get(id).and_then(|s| s.command).and_then(|c| u.commands.get(c)).is_some_and(|c| c.order == command::order::ATTACK))
}

/// `aitTeamIsFinishedMoving` (0x417320): the centre is within `range` of the destination, or
/// the first ship has nowhere to go.
fn team_finished_moving(u: &Universe, ships: &[ShipId], dest: Vector, range: f32) -> bool {
    if ships.is_empty() {
        return true;
    }
    let c = centre(u, ships);
    let d = crate::level::distance_squared(c, dest);
    let r = ld(range) * ld(range);
    let first_still = u.ships.get(ships[0]).is_none_or(|s| s.move_to == Vector::ZERO);
    !(r <= d) || first_still
}

/// `selCentrePointComputeGeneral`: the middle of the ships' bounding box.
pub fn centre(u: &Universe, ships: &[ShipId]) -> Vector {
    let mut lo = [1e21f32; 3];
    let mut hi = [-1e21f32; 3];
    for p in ships.iter().filter_map(|&id| u.ships.get(id)).map(|s| s.posinfo.position) {
        for (k, c) in [p.x, p.y, p.z].into_iter().enumerate() {
            if c <= lo[k] {
                lo[k] = c;
            }
            if hi[k] <= c {
                hi[k] = c;
            }
        }
    }
    Vector::new(st((ld(hi[0]) + ld(lo[0])) * 0.5), st((ld(hi[1]) + ld(lo[1])) * 0.5), st((ld(hi[2]) + ld(lo[2])) * 0.5))
}

/// A ship died: the attack moves forget it as a target (`aimShipDiedGuardShips`).
pub fn ship_died(u: &mut Universe, id: ShipId) {
    for (_, tm) in u.ai.teams.iter_mut().flatten() {
        for m in &mut tm.moves {
            match &mut m.kind {
                MoveKind::Attack { targets } | MoveKind::FlankAttack { targets } | MoveKind::FlankAttackHyperspace { targets } | MoveKind::GuardShips { ships: targets } => targets.retain(|&s| s != id),
                // `aimShipReconShipDied`, `aimInterceptShipDied`.
                MoveKind::ShipRecon { targets, found } => {
                    targets.retain(|&s| s != id);
                    found.retain(|&s| s != id);
                }
                MoveKind::Intercept { target, .. } if *target == Some(id) => *target = None,
                MoveKind::AdvancedAttack { targets, .. } => targets.retain(|&s| s != id),
                _ => {}
            }
        }
    }
    // `aimHarassAttackShipDied`.
    let found: Vec<(usize, usize)> = u.ai.teams.iter().enumerate().filter_map(|(t, x)| x.as_ref().map(|x| (t, &x.1))).flat_map(|(t, tm)| tm.moves.iter().enumerate().filter(|(_, m)| matches!(m.kind, MoveKind::HarassAttack { target: Some(s) } if s == id)).map(move |(c, _)| (t, c))).collect();
    for (t, c) in found {
        super::harass::harass_target_died(u, t, c, id);
    }
}
