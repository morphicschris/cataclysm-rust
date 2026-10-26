//! Mission 1: after the linking technology, two Acolytes link into an ACV through the
//! special-action menu's "Link" (the item the Z key's menu shows), and the ACV splits again
//! through "Unlink" (needs the game: `HWC_DATA_DIR` or `HWC_GAME_DIR`; skipped otherwise).

use std::sync::{Arc, OnceLock};

use hwc_formats::gamedata::GameData;
use hwc_math::Vector;
use hwc_sim::base::{self, CreateOptions};
use hwc_sim::mission::{load_kas_mission, mission_script_name, GameDataSource, MissionConfig, MissionRun};
use hwc_sim::player_orders::{self as po, SpecialItem};
use hwc_sim::statics::{LoadOptions, ShipType, StaticWorld, Technology};
use hwc_sim::{ShipId, SimTweaks};

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
        let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions { single_player: true, ..LoadOptions::default() }));
        let tweaks = SimTweaks::load(&src);
        Some(Data { gd, statics, tweaks })
    })
    .as_ref()
}

fn mission1(d: &Data) -> MissionRun {
    let script = load_kas_mission(&d.gd, &mission_script_name(1)).expect("mission script");
    let mut config = MissionConfig::new(1);
    config.log_calls = false;
    config.speech_seconds = Some(3.0);
    MissionRun::start(d.statics.clone(), d.tweaks.clone(), &GameDataSource(&d.gd), script, config).expect("mission 1")
}

const PLAYER: usize = 0;

/// Two Acolytes of the player beside its mothership.
fn acolytes(run: &mut MissionRun, dz: f32) -> [ShipId; 2] {
    let u = &mut run.universe;
    let ms = u.base.players[PLAYER].mothership.expect("mothership");
    let p = u.ship(ms).unwrap().posinfo.position;
    let mut make = |off: Vector| {
        let id = base::create_ship(u, ShipType::sAcolyte, PLAYER, p + off, CreateOptions::default()).expect("acolyte");
        hwc_sim::construction::unit_cap_create_ship(u, id);
        id
    };
    [make(Vector::new(3000.0, 0.0, dz)), make(Vector::new(3400.0, 300.0, dz + 200.0))]
}

fn menu(run: &MissionRun, sel: &[ShipId]) -> Vec<SpecialItem> {
    po::special_menu(&run.universe, PLAYER, sel, &[])
}

fn acvs(run: &MissionRun) -> Vec<ShipId> {
    let u = &run.universe;
    u.ship_list.iter().copied().filter(|&id| u.ship(id).is_some_and(|s| s.ship_type == ShipType::sACV && s.player == PLAYER)).collect()
}

#[test]
fn mission1_acolytes_link_into_an_acv_from_the_special_menu() {
    let Some(d) = data() else { return };
    let mut run = mission1(d);
    run.run(32);
    let old = acolytes(&mut run, 0.0);
    // Without the linking technology the menu has no "Link" and nothing to upgrade.
    assert!(!menu(&run, &old).contains(&SpecialItem::Link));
    assert!(!po::any_can_upgrade(&run.universe, PLAYER, &old));

    // Research linking.
    let link = Technology::Link.index();
    let u = &mut run.universe;
    if !hwc_sim::research::research_tech_if_lab_free(u, PLAYER, link) || !u.base.players[PLAYER].tech.contains(link) {
        u.base.players[PLAYER].tech.add(link);
        hwc_sim::upgrade::player_update_from_research(u, PLAYER);
    }
    assert!(run.universe.base.players[PLAYER].tech.contains(link));

    // Acolytes built before the research take the upgrade first (the main menu's "Upgrade"):
    // Acolytes upgrade by docking, so they dock and come out upgraded.
    assert!(!menu(&run, &old).contains(&SpecialItem::Link));
    assert!(po::any_can_upgrade(&run.universe, PLAYER, &old));
    assert!(hwc_sim::upgrade::upgrade_cost(&run.universe, old[0]) > 0.0);
    po::mr_upgrade_ships(&mut run.universe, &old);
    let order = |run: &MissionRun, a: ShipId| run.universe.ship(a).and_then(|s| s.command).and_then(|c| run.universe.commands.get(c)).map(|c| c.order);
    assert_eq!(order(&run, old[0]), Some(hwc_sim::command::order::DOCK));
    let upgraded = |run: &MissionRun| {
        old.iter().all(|&a| {
            run.universe.ship(a).is_some_and(|s| {
                hwc_sim::upgrade::has_upgrade(s, hwc_sim::link::UPGRADE_LINK) && s.flags & hwc_sim::object::sof::HIDE == 0 && s.docking_ship.is_none()
            })
        })
    };
    let mut n = 0;
    while !upgraded(&run) && n < 16 * 300 {
        run.step();
        n += 1;
    }
    assert!(upgraded(&run), "docked, upgraded and out again ({n} frames)");
    run.run(16 * 5);
    assert!(menu(&run, &old).contains(&SpecialItem::Link));
    assert!(!menu(&run, &old).contains(&SpecialItem::Unlink));

    // Acolytes built after the research can link at once.
    let new = acolytes(&mut run, -2000.0);
    assert!(menu(&run, &new).contains(&SpecialItem::Link));

    // Link (the Z menu's item): the two pairs become two ACVs.
    let all = [old[0], old[1], new[0], new[1]];
    let health: f32 = old.iter().map(|&a| run.universe.ship(a).unwrap().health).sum();
    assert!(po::execute_special(&mut run.universe, PLAYER, &all, SpecialItem::Link, &[]));
    // The lower ship of each pair is the one that flies.
    for pair in [old, new] {
        let (a, b) = (run.universe.ship(pair[0]).unwrap(), run.universe.ship(pair[1]).unwrap());
        assert_eq!(a.base.link.partner, Some(pair[1]));
        let (lo, hi) = if a.posinfo.position.z < b.posinfo.position.z { (a, b) } else { (b, a) };
        assert_eq!(lo.base.link.state, hwc_sim::link::state::APPROACH);
        assert_eq!(hi.base.link.state, hwc_sim::link::state::PARTNER);
    }
    let mut n = 0;
    while acvs(&run).len() < 2 && n < 16 * 120 {
        run.step();
        n += 1;
    }
    let made = acvs(&run);
    assert_eq!(made.len(), 2, "two ACVs after {n} frames");
    for a in all {
        assert!(run.universe.ship(a).is_none(), "the Acolytes are gone");
    }
    // The linked ship's health: the mean of its parts' fractions.
    let u = &run.universe;
    let (ma, mv) = (u.statics.ship(ShipType::sAcolyte).unwrap().health.max_health, u.statics.ship(ShipType::sACV).unwrap().health.max_health);
    let want = (health / 2.0) / ma * mv;
    assert!(made.iter().any(|&id| (u.ship(id).unwrap().health - want).abs() < 1.0), "health {want}");

    // The ACV's menu: Unlink and its EMP (once researched); Unlink splits it again.
    let acv = made[0];
    let m = menu(&run, &[acv]);
    assert!(m.contains(&SpecialItem::Unlink) && !m.contains(&SpecialItem::Link), "{m:?}");
    assert!(po::execute_special(&mut run.universe, PLAYER, &[acv], SpecialItem::Unlink, &[]));
    run.run(4);
    assert!(run.universe.ship(acv).is_none());
    let u = &run.universe;
    let parts = u.ship_list.iter().filter(|&&id| u.ship(id).is_some_and(|s| s.ship_type == ShipType::sAcolyte && s.player == PLAYER)).count();
    assert!(parts >= 2);
}

#[test]
fn an_odd_acolyte_is_left_out_of_the_link() {
    let Some(d) = data() else { return };
    let mut run = mission1(d);
    run.run(32);
    let u = &mut run.universe;
    u.base.players[PLAYER].tech.add(Technology::Link.index());
    hwc_sim::upgrade::player_update_from_research(u, PLAYER);
    let pair = acolytes(&mut run, 0.0);
    let odd = acolytes(&mut run, 5000.0)[0];
    let cid = po::mr_link_ships(&mut run.universe, &[pair[0], pair[1], odd]).expect("link order");
    let u = &run.universe;
    let sel = &u.commands.get(cid).unwrap().selection;
    assert_eq!(sel.len(), 1, "only the initiator keeps the order");
    assert!(u.ship(odd).unwrap().base.link.partner.is_none());
    assert!(u.ship(odd).unwrap().command.is_none());
}
