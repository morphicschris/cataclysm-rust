//! Multiplayer games on one machine (needs the game: `HWC_DATA_DIR` or `HWC_GAME_DIR`;
//! skipped when neither is set): every shipped map loads for every player count it takes,
//! with Somtaaw and Beast players, and a game runs to a winner.

use std::sync::{Arc, OnceLock};

use hwc_formats::gamedata::GameData;
use hwc_sim::mission::GameDataSource;
use hwc_sim::multiplayer::{mg, MpEvent};
use hwc_sim::skirmish::{game_type_index, game_types, map_list, skirmish_races, GameOptions, GameSetup, PlayerSetup, SkirmishRun};
use hwc_sim::statics::{LoadOptions, ShipRace, ShipType, StaticWorld};
use hwc_sim::SimTweaks;

struct Data {
    gd: GameData,
    statics: Arc<StaticWorld>,
    tweaks: SimTweaks,
}

fn data() -> Option<&'static Data> {
    static DATA: OnceLock<Option<Data>> = OnceLock::new();
    DATA.get_or_init(|| {
        let gd = match GameData::from_env() {
            Some(Ok(gd)) => gd,
            _ => {
                eprintln!("HWC_DATA_DIR / HWC_GAME_DIR not set; skipping");
                return None;
            }
        };
        let src = GameDataSource(&gd);
        let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions::default()));
        let tweaks = SimTweaks::load(&src);
        Some(Data { gd, statics, tweaks })
    })
    .as_ref()
}

fn default_game(d: &Data) -> GameOptions {
    let src = GameDataSource(&d.gd);
    game_types(&src).into_iter().find(|g| g.name == "Default").expect("the Default game type").options()
}

fn setup(map: &str, races: &[ShipRace], options: GameOptions, seed: u32) -> GameSetup {
    GameSetup {
        map: map.to_string(),
        players: races.iter().enumerate().map(|(i, &race)| PlayerSetup { race, computer: i > 0 }).collect(),
        options,
        slots: None,
        seed,
        effects: false,
        local_player: 0,
        network: false,
    }
}

#[test]
fn the_game_types_and_maps_are_read() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let types = game_types(&src);
    let names: Vec<&str> = types.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["Default", "I hate harvesting", "Bounty Hunter", "Carpe Diem"]);
    let def = types[0].options();
    assert_eq!(def.flags, mg::LAST_MOTHERSHIP | mg::HARVESTING_ENABLED | mg::RESEARCH_ENABLED | mg::UNIT_CAPS_ENABLED | mg::ALLOW_ALLIES | mg::HYPERSPACE);
    assert_eq!((def.starting_resources, def.injection_interval, def.injection_amount), (1, 14400, 2000));
    let carpe = types[3].options();
    assert_eq!(carpe.flags & (mg::CAPTURE_CAPITAL_SHIP | mg::RESEARCH_ENABLED | mg::RESOURCE_INJECTIONS), mg::CAPTURE_CAPITAL_SHIP | mg::RESOURCE_INJECTIONS);
    // Picking a type selects its button; a change away from every type selects "custom".
    for (i, g) in types.iter().enumerate() {
        assert_eq!(game_type_index(&types, &g.options()), i, "{}", g.name);
    }
    // "Bounty Hunter" leaves the injection settings as they were.
    let mut o = types[1].options();
    types[2].apply(&mut o);
    assert_eq!((o.injection_interval, o.bounty_size), (14400, 2));
    let mut custom = def.clone();
    custom.flags ^= mg::CRATES_ENABLED;
    assert_eq!(game_type_index(&types, &custom), types.len());

    let maps = map_list(&src);
    let names: Vec<(&str, usize)> = maps.iter().map(|m| (m.name.as_str(), m.max_players)).collect();
    assert_eq!(
        names,
        [
            ("Coldwar2", 2),
            ("TheGates3", 3),
            ("TrigsBones3", 3),
            ("Kristalzupacken4", 4),
            ("FerrousFaucets4", 4),
            ("TheAnvil6", 6),
            ("KiithKnox6", 6),
            ("Metro8", 8),
            ("Armed8", 8),
            ("ArmedDeux8", 8),
        ]
    );
}

/// Every map, every player count, Somtaaw first and Beast first: each player gets a
/// mothership of its race and a start fleet all of its race, at its own start position, and
/// the starting RUs of the game type.
#[test]
fn every_map_loads_for_every_player_count() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let options = default_game(d);
    let mut report = Vec::new();
    for map in map_list(&src) {
        for n in map.min_players..=map.max_players {
            for first in [ShipRace::Somtaaw, ShipRace::Beast] {
                let races = skirmish_races(first, n);
                let run = SkirmishRun::start(d.statics.clone(), d.tweaks.clone(), &src, setup(&map.name, &races, options.clone(), 7 + n as u32))
                    .unwrap_or_else(|e| panic!("{} with {n}: {e}", map.name));
                let u = &run.universe;
                let mut motherships = Vec::new();
                for (p, &race) in races.iter().enumerate() {
                    let ms = u.base.players[p].mothership.unwrap_or_else(|| panic!("{} with {n}: player {p} has no mothership", map.name));
                    let ms_type = u.ships.ship(ms).ship_type;
                    let want = if race == ShipRace::Somtaaw { ShipType::sMothership } else { ShipType::bMothership };
                    assert_eq!(ms_type, want, "{} with {n}: player {p}'s mothership", map.name);
                    motherships.push(u.ships.ship(ms).posinfo.position);
                    let fleet: Vec<_> = u.ship_list.iter().map(|&id| u.ships.ship(id)).filter(|s| s.player == p).collect();
                    assert!(fleet.len() > 1, "{} with {n}: player {p} has only {} ships", map.name, fleet.len());
                    for s in &fleet {
                        let r = u.statics.ship(s.ship_type).map(|st| st.ship_race);
                        assert_eq!(r, Some(race), "{} with {n}: player {p} has a {}", map.name, s.ship_type);
                    }
                    // Medium resources, five times as much for computer players (harvesting on).
                    let rus = u.base.players[p].resource_units;
                    assert_eq!(rus, if p == 0 { 2500 } else { 12500 }, "{} with {n}: player {p}'s RUs", map.name);
                }
                // Nobody shares a start position.
                for a in 0..n {
                    for b in a + 1..n {
                        let (pa, pb) = (motherships[a], motherships[b]);
                        let d2 = (pa.x - pb.x).powi(2) + (pa.y - pb.y).powi(2) + (pa.z - pb.z).powi(2);
                        assert!(d2 > 1000.0 * 1000.0, "{} with {n}: players {a} and {b} start together", map.name);
                    }
                }
                if first == ShipRace::Somtaaw {
                    report.push(format!("{} x{n}: {} ships, slots {:?}", map.name, u.ship_list.len(), &run.slots[..map.max_players]));
                }
                for w in &run.level.warnings {
                    if !w.contains("not modelled") {
                        report.push(format!("  {} x{n}: {w}", map.name));
                    }
                }
            }
        }
    }
    println!("{}", report.join("\n"));
}

/// The game runs, and a short game ends: player 1's mothership is destroyed with
/// "last mothership" on, its ships are disabled to blow up, player 0 is declared the winner
/// `WAIT_TIME_TO_WIN` later and the game ends after `WAIT_TIME_TO_QUIT`.
#[test]
fn losing_the_mothership_loses_the_game() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let mut run = SkirmishRun::start(d.statics.clone(), d.tweaks.clone(), &src, setup("Coldwar2", &[ShipRace::Somtaaw, ShipRace::Beast], default_game(d), 1)).expect("start");
    for _ in 0..16 * 5 {
        run.step();
    }
    let u = &mut run.universe;
    let ms = u.base.players[1].mothership.expect("player 1's mothership");
    u.ships.ship_mut(ms).health = 0.0;
    hwc_sim::damage::add_ship_to_delete_list(u, ms, -1);
    let mut events = Vec::new();
    let mut steps = 0;
    while !run.over() && steps < 16 * 60 {
        run.step();
        steps += 1;
        events.extend(run.universe.mp.events.drain(..));
    }
    println!("events: {events:?} after {steps} steps");
    assert!(events.contains(&MpEvent::PlayerDefeated { player: 1, how: 0 }), "{events:?}");
    assert_eq!(run.winners(), Some(1), "player 0 wins");
    assert!(run.over(), "the game ends");
    let u = &run.universe;
    assert_eq!(u.base.players[1].state, 1);
    assert_eq!(u.base.players[0].state, 0);
    // The loser's ships careen to their deaths.
    let left: Vec<_> = u.ship_list.iter().map(|&id| u.ships.ship(id)).filter(|s| s.player == 1).collect();
    assert!(left.iter().all(|s| s.death_time > 0.0), "every ship of the loser is set to blow up");
}

/// Resource injections and the lump sum arrive when the game type says.
#[test]
fn resource_injections_arrive() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let mut options = default_game(d);
    options.flags |= mg::RESOURCE_INJECTIONS | mg::RESOURCE_LUMP_SUM;
    options.flags &= !mg::HARVESTING_ENABLED;
    options.injection_interval = 160;
    options.injection_amount = 700;
    options.lump_sum_time = 240;
    options.lump_sum_amount = 3000;
    let mut run = SkirmishRun::start(d.statics.clone(), d.tweaks.clone(), &src, setup("Coldwar2", &[ShipRace::Beast, ShipRace::Somtaaw], options, 2)).expect("start");
    let before = run.universe.base.players[0].resource_units;
    let mut got = Vec::new();
    for _ in 0..250 {
        run.step();
        let now = run.universe.update_counter;
        got.extend(run.universe.mp.events.drain(..).filter_map(|e| match e {
            MpEvent::ResourcesInjected { amount } => Some((now, amount)),
            _ => None,
        }));
    }
    assert_eq!(got, [(160, 700), (240, 3000)]);
    assert_eq!(run.universe.base.players[0].resource_units, before + 3700);
}

/// "Carpe Diem" (capture the capital ship, research off): every player gets a Hive Frigate of
/// its race and knows every technology.
#[test]
fn carpe_diem_gives_a_hive_frigate() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let carpe = game_types(&src).into_iter().find(|g| g.name == "Carpe Diem").expect("Carpe Diem").options();
    let run = SkirmishRun::start(d.statics.clone(), d.tweaks.clone(), &src, setup("TheGates3", &[ShipRace::Somtaaw, ShipRace::Beast, ShipRace::Somtaaw], carpe, 3)).expect("start");
    let u = &run.universe;
    for (p, t) in [(0, ShipType::sHiveFrigate), (1, ShipType::bHiveFrigate), (2, ShipType::sHiveFrigate)] {
        assert!(u.ship_list.iter().any(|&id| u.ships.ship(id).player == p && u.ships.ship(id).ship_type == t), "player {p} has a {t}");
        assert_eq!(u.base.players[p].tech.mode, hwc_sim::base::Techs::MODE_ALL, "player {p} knows everything");
    }
}

/// A whole game under scripted orders: player 0 builds Acolytes at its mothership and sends a
/// strike force of destroyers and multi-beam frigates at player 1's mothership, which is
/// destroyed; player 0 wins and the game ends.
#[test]
fn a_scripted_game_runs_to_a_winner() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let mut options = default_game(d);
    options.flags &= !mg::RESEARCH_ENABLED;
    // Both players human: a computer player would defend its mothership.
    let mut s = setup("Coldwar2", &[ShipRace::Somtaaw, ShipRace::Beast], options, 5);
    for p in &mut s.players {
        p.computer = false;
    }
    let mut run = SkirmishRun::start(d.statics.clone(), d.tweaks.clone(), &src, s).expect("start");
    let u = &mut run.universe;
    let ms0 = u.base.players[0].mothership.expect("mothership 0");
    let ms1 = u.base.players[1].mothership.expect("mothership 1");
    u.base.players[0].resource_units = 20_000;
    let queued = hwc_sim::construction::queue_build(u, ms0, ShipType::sAcolyte, 5);
    assert_eq!(queued, 5, "the mothership takes the Acolytes");
    let at = u.ships.ship(ms0).posinfo.position;
    let mut force = Vec::new();
    for (i, t) in [ShipType::sDestroyer, ShipType::sDestroyer, ShipType::sDestroyer, ShipType::sDestroyer, ShipType::sMultiBeamFrigate, ShipType::sMultiBeamFrigate].into_iter().enumerate() {
        let p = hwc_math::Vector::new(at.x + 1500.0 + 600.0 * i as f32, at.y, at.z + 1500.0);
        let id = u.add_ship(t, 0, p, None).expect("ship");
        hwc_sim::upgrade::apply_upgrades(u, id);
        force.push(id);
    }
    // Orders go to ships in the collision blobs, which take them in at the next update.
    for _ in 0..8 {
        run.step();
    }
    hwc_sim::command::cl_attack(&mut run.universe, &force, &[ms1]);
    let mut events = Vec::new();
    let mut steps = 0u32;
    while !run.over() && steps < 16 * 60 * 20 {
        run.step();
        steps += 1;
        events.extend(run.universe.mp.events.drain(..));
    }
    let u = &run.universe;
    let acolytes = u.ship_list.iter().filter(|&&id| u.ships.ship(id).player == 0 && u.ships.ship(id).ship_type == ShipType::sAcolyte).count();
    println!("after {:.0} s: events {events:?}; player 0 has {acolytes} Acolytes, {} RUs", steps as f32 / 16.0, u.base.players[0].resource_units);
    assert!(acolytes >= 5, "the Acolytes were built");
    assert_eq!(run.winners(), Some(1), "player 0 wins");
    assert!(run.over());
}

/// Crates: one appears between the players when its check comes round, and the first ship to
/// reach it claims what is inside for its player.
#[test]
fn a_crate_appears_and_is_claimed() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let mut options = default_game(d);
    options.flags |= mg::CRATES_ENABLED;
    // No computer players: nothing else spends or builds while the crate is watched.
    let mut s = setup("Kristalzupacken4", &[ShipRace::Somtaaw, ShipRace::Beast, ShipRace::Somtaaw], options, 9);
    for p in &mut s.players {
        p.computer = false;
    }
    let mut run = SkirmishRun::start(d.statics.clone(), d.tweaks.clone(), &src, s).expect("start");
    let tw = run.universe.mp.crates.tweaks;
    assert_eq!((tw.add_check_time, tw.expiry_time, tw.add_prob, tw.min_rus), (240.0, 420.0, 0.25, 2000), "Crates.script is read");
    run.universe.mp.crates.tweaks.add_check_time = 2.0;
    run.universe.mp.crates.tweaks.add_prob = 0.0;
    let mut appeared = None;
    for _ in 0..16 * 3 {
        run.step();
        for e in run.universe.mp.events.drain(..) {
            if let MpEvent::CrateAppeared { position } = e {
                appeared = Some(position);
            }
        }
    }
    run.universe.mp.crates.tweaks.add_check_time = 240.0;
    let at = appeared.expect("a crate appeared");
    assert_eq!(run.universe.mp.crates.list.len(), 1);
    // Player 2's first recon goes straight to it.
    let u = &mut run.universe;
    let scout = u.ship_list.iter().copied().find(|&id| u.ships.ship(id).player == 2 && u.ships.ship(id).ship_type == ShipType::sRecon).expect("a recon");
    hwc_sim::base::set_ship_position(u, scout, at);
    let before = (u.base.players[2].resource_units, u.ship_list.len(), u.base.players[2].tech);
    let mut found = None;
    for _ in 0..80 {
        run.step();
        for e in run.universe.mp.events.drain(..) {
            if let MpEvent::CrateFound { player, reward, .. } = e {
                found = Some((player, reward));
            }
        }
    }
    let (player, reward) = found.expect("the crate was found");
    println!("crate at {at:?}: {reward:?}");
    assert_eq!(player, 2);
    assert!(run.universe.mp.crates.list.is_empty());
    let u = &run.universe;
    match reward {
        hwc_sim::crates::Reward::Resources(n) => {
            assert!((2000..=10000).contains(&n));
            assert_eq!(u.base.players[2].resource_units, before.0 + n);
        }
        hwc_sim::crates::Reward::Ships { ship_type, count } => {
            assert_eq!(u.ship_list.len(), before.1 + count as usize, "{count} {ship_type} given");
        }
        hwc_sim::crates::Reward::Technology(t) => {
            assert!(!before.2.contains(t) && u.base.players[2].tech.contains(t));
        }
    }
}

/// Computer players against each other: they build resourcers and harvest, ask for the
/// Somtaaw mothership's modules and research.
#[test]
fn computer_players_build_and_harvest() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    for slider in [10u8, 50, 90] {
        let mut options = default_game(d);
        options.cpu_difficulty = slider;
        let mut s = setup("Coldwar2", &[ShipRace::Somtaaw, ShipRace::Beast], options, 11);
        for p in &mut s.players {
            p.computer = true;
        }
        let mut run = SkirmishRun::start(d.statics.clone(), d.tweaks.clone(), &src, s).expect("start");
        let count = |run: &SkirmishRun, p: usize, t: ShipType| run.universe.ship_list.iter().filter(|&&id| run.universe.ships.ship(id).player == p && run.universe.ships.ship(id).ship_type == t).count();
        let workers0 = (count(&run, 0, ShipType::sWorker), count(&run, 1, ShipType::bWorker));
        for _ in 0..16 * 60 * 6 {
            run.step();
        }
        let u = &run.universe;
        let workers = (count(&run, 0, ShipType::sWorker), count(&run, 1, ShipType::bWorker));
        let collected: Vec<i32> = (0..2).map(|p| u.base.stats.players.get(p).map_or(0, |s| s.ru_collected)).collect();
        let modules: Vec<(ShipType, bool)> = u.ship_list.iter().map(|&id| u.ships.ship(id)).filter(|s| s.player == 0 && matches!(s.ship_type, ShipType::sMothershipDockingBay | ShipType::sMothershipMicro | ShipType::sMothershipSpecial | ShipType::sMothershipWeapons | ShipType::sMothershipArmour | ShipType::sMothershipSupport)).map(|s| (s.ship_type, s.special_flags2 & hwc_sim::base::special2::UNDER_CONSTRUCTION == 0)).collect();
        let research: Vec<usize> = (0..2).map(|p| u.base.players[p].topics.len() + u.base.players[p].tech.has.iter().filter(|&&h| h == 1).count()).collect();
        println!("slider {slider}: workers {workers0:?} -> {workers:?}, collected {collected:?}, RUs {:?}, Somtaaw modules {modules:?}, research {research:?}", (0..2).map(|p| u.base.players[p].resource_units).collect::<Vec<_>>());
        assert!(workers.0 > workers0.0 && workers.1 > workers0.1, "both build resourcers");
        assert!(collected[0] > 0 && collected[1] > 0, "both harvest");
        assert!(modules.iter().any(|&(t, done)| t == ShipType::sMothershipDockingBay && done), "the Somtaaw computer player finishes its docking bay: {modules:?}");

    }
}

/// Computer players against each other: the attack manager's teams get their ships, wait in
/// a temporary guard, then the armada goes after the enemy.
#[test]
fn computer_players_attack() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    for slider in [10u8, 90] {
        let mut options = default_game(d);
        options.cpu_difficulty = slider;
        // Without research every ship can be built from the start.
        options.flags &= !hwc_sim::multiplayer::mg::RESEARCH_ENABLED;
        let mut s = setup("Coldwar2", &[ShipRace::Somtaaw, ShipRace::Beast], options, 11);
        for p in &mut s.players {
            p.computer = true;
        }
        let mut run = SkirmishRun::start(d.statics.clone(), d.tweaks.clone(), &src, s).expect("start");
        let mut attacked = [false; 2];
        let mut teams_with_ships = [0usize; 2];
        for minute in 0..20 {
            for _ in 0..16 * 60 {
                run.step();
                let u = &run.universe;
                for &id in &u.ship_list {
                    let sh = u.ships.ship(id);
                    if sh.player > 1 {
                        continue;
                    }
                    let Some(c) = sh.command.and_then(|c| u.commands.get(c)) else { continue };
                    if c.order == hwc_sim::command::order::ATTACK && c.attack_targets.iter().any(|&t| u.ships.get(t).is_some_and(|x| x.player != sh.player && x.player < 2)) {
                        attacked[sh.player] = true;
                    }
                }
            }
            let u = &run.universe;
            for p in 0..2 {
                let a = u.ai.players[p].as_ref().unwrap();
                let teams: Vec<String> = a.attack.teams.iter().flatten().filter_map(|&t| u.ai.teams[t].as_ref()).map(|(_, t)| format!("{}:{:?}", t.ships.len(), t.current.map(|c| t.moves[c].move_type()))).collect();
                teams_with_ships[p] = teams_with_ships[p].max(a.attack.teams.iter().flatten().filter_map(|&t| u.ai.teams[t].as_ref()).filter(|(_, t)| !t.ships.is_empty()).count());
                let var = hwc_sim::ai::team::var_get(u, &a.attack.var);
                println!("slider {slider} min {minute} p{p}: ships {} RUs {} var {var} armada {:?} teams {teams:?} requests {:?}", u.ship_list.iter().filter(|&&id| u.ships.ship(id).player == p).count(), u.base.players[p].resource_units, a.attack.armada_targets.len(), a.requests[0].iter().map(|r| (r.ship_type, r.num)).collect::<Vec<_>>());
            }
        }
        println!("slider {slider}: attacked {attacked:?}, teams with ships {teams_with_ships:?}");
        assert!(teams_with_ships.iter().all(|&n| n > 0), "both form attack teams");
        assert!(attacked.iter().any(|&a| a), "a computer player attacks");
    }
}

/// The computer player's defence: escorts for its resourcers, and when enemy frigates attack
/// its mothership its ships go after them.
#[test]
fn a_computer_player_defends_its_mothership() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let mut options = default_game(d);
    options.flags &= !mg::RESEARCH_ENABLED;
    options.cpu_difficulty = 50;
    let mut run = SkirmishRun::start(d.statics.clone(), d.tweaks.clone(), &src, setup("Coldwar2", &[ShipRace::Somtaaw, ShipRace::Beast], options, 7)).expect("start");
    // The computer player builds up for four minutes.
    for _ in 0..16 * 60 * 4 {
        run.step();
    }
    let escorts = {
        let u = &run.universe;
        let a = u.ai.players[1].as_ref().unwrap();
        a.defense.teams.iter().filter(|&&t| u.ai.teams[t].as_ref().is_some_and(|(_, tm)| tm.moves.iter().any(|m| matches!(m.kind, hwc_sim::ai::team::MoveKind::GuardShips { .. })))).count()
    };
    let u = &mut run.universe;
    let ms1 = u.base.players[1].mothership.expect("mothership 1");
    let at = u.ships.ship(ms1).posinfo.position;
    let mut force = Vec::new();
    for i in 0..4 {
        let p = hwc_math::Vector::new(at.x + 2500.0 + 600.0 * i as f32, at.y, at.z + 2500.0);
        let id = u.add_ship(ShipType::sMultiBeamFrigate, 0, p, None).expect("ship");
        force.push(id);
    }
    for _ in 0..8 {
        run.step();
    }
    hwc_sim::command::cl_attack(&mut run.universe, &force, &[ms1]);
    let mut defended = false;
    let mut attackers_seen = false;
    for _ in 0..16 * 60 * 2 {
        run.step();
        let u = &run.universe;
        attackers_seen |= !u.ai.players[1].as_ref().unwrap().defense.mothership_attackers.is_empty();
        defended |= u.ship_list.iter().any(|&id| {
            let s = u.ships.ship(id);
            s.player == 1 && s.command.and_then(|c| u.commands.get(c)).is_some_and(|c| c.order == hwc_sim::command::order::ATTACK && c.attack_targets.iter().any(|t| force.contains(t)))
        });
        if defended {
            break;
        }
    }
    let u = &run.universe;
    let a = u.ai.players[1].as_ref().unwrap();
    println!("escorts {escorts}, attackers seen {attackers_seen}, defended {defended}, defence teams {:?}, mothership team {:?}", a.defense.teams, a.defense.mothership_team);
    assert!(attackers_seen, "the computer player sees its mothership attacked");
    assert!(defended, "its ships attack the attackers");
}

/// Bounties (`calculatePlayerBounties`, `bountyShipWasKilled`): with bounties on, each player's
/// is worked out from the fleets' value and the RUs held, and a ship destroyed by another
/// player's weapons pays that player its owner's bounty in hundredths of its cost.
#[test]
fn bounties_are_worked_out_and_paid() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let mut options = default_game(d);
    options.bounty_size = 1;
    let mut s = setup("Coldwar2", &[ShipRace::Somtaaw, ShipRace::Beast], options, 3);
    for p in &mut s.players {
        p.computer = false;
    }
    let mut run = SkirmishRun::start(d.statics.clone(), d.tweaks.clone(), &src, s).expect("start");
    for _ in 0..4 {
        run.step();
    }
    let b = run.universe.mp.bounties.clone();
    println!("bounties {b:?}, base {}", run.universe.mp.bounty_base);
    assert!(run.universe.mp.bounty_base > 0, "the medium size from tweak.script");
    assert!(b.iter().all(|&x| x > 0), "{b:?}");
    let u = &mut run.universe;
    let victim = u.ship_list.iter().copied().find(|&id| {
        let s = u.ships.ship(id);
        s.player == 1 && u.base.players[1].mothership != Some(id) && s.flags & hwc_sim::object::sof::HIDE == 0 && s.flags2 & 4 == 0 && u.statics.ship(s.ship_type).is_some_and(|st| st.build_cost > 0 && st.ship_class != Some(hwc_sim::statics::ShipClass::Component))
    });
    let Some(victim) = victim else { return };
    let cost = u.statics.ship(u.ships.ship(victim).ship_type).unwrap().build_cost;
    let before = u.base.players[0].resource_units;
    let hp = u.ships.ship(victim).health;
    assert!(hwc_sim::damage::apply_damage_to_target(u, victim, hp + 1.0, 0, 0x29a, 0, None), "it dies");
    let paid = u.base.players[0].resource_units - before;
    let expect = (f64::from(b[1]) * f64::from(0.01f32) * f64::from(cost)) as i32;
    println!("paid {paid} for a ship of cost {cost} at bounty {}", b[1]);
    assert_eq!(paid, expect);
    assert_eq!(u.base.stats.players[0].ru_bounty, expect);
}
