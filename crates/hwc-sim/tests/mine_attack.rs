//! Attacking mines (needs the game: `HWC_DATA_DIR` or `HWC_GAME_DIR`; skipped when neither is
//! set): in Mission 3 the player's Acolytes are ordered to attack the mine nearest to them, as
//! the Ctrl band box does, and shoot it until it is destroyed; the order then ends.

use std::sync::Arc;

use hwc_formats::gamedata::GameData;
use hwc_sim::campaign::Carry;
use hwc_sim::mission::{load_kas_mission, mission_script_name, GameDataSource, MissionConfig, MissionRun};
use hwc_sim::missile::kind;
use hwc_sim::statics::{LoadOptions, ShipType, StaticWorld};
use hwc_sim::{command, ShipId, SimTweaks};

#[test]
fn acolytes_destroy_a_mine() {
    let Some(Ok(gd)) = GameData::from_env() else {
        eprintln!("HWC_DATA_DIR / HWC_GAME_DIR not set; skipping");
        return;
    };
    let src = GameDataSource(&gd);
    let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions { single_player: true, ..LoadOptions::default() }));
    let tweaks = SimTweaks::load(&src);
    let script = load_kas_mission(&gd, &mission_script_name(3)).expect("mission script");
    let mut config = MissionConfig::new(3);
    config.log_calls = false;
    config.speech_seconds = Some(3.0);
    let carry = Carry::warp(statics.clone(), tweaks.clone(), &src, 3).expect("warp.script level 3");
    let mut run = MissionRun::start_campaign(statics, tweaks, &src, script, config, &carry).expect("mission 3");
    // Until the fleet is in and the Acolytes have launched.
    let acolytes = |run: &MissionRun| -> Vec<ShipId> {
        let u = &run.universe;
        u.ship_list
            .iter()
            .copied()
            .filter(|&id| {
                u.ships.get(id).is_some_and(|s| {
                    s.player == 0 && s.ship_type == ShipType::sAcolyte && s.coll_my_blob.is_some() && s.flags & hwc_sim::object::sof::HIDE == 0
                })
            })
            .collect()
    };
    let mut frames = 0;
    while acolytes(&run).len() < 2 {
        run.step();
        frames += 1;
        assert!(frames < 16 * 240, "no Acolytes out in mission 3");
    }
    let sel = acolytes(&run);
    let u = &run.universe;
    let centre = u.ships.ship(sel[0]).posinfo.position;
    let (mine, _) = u
        .missile_list
        .iter()
        .filter_map(|&m| u.missiles.get(m))
        .filter(|m| m.kind == kind::MINE)
        .map(|m| {
            let d = m.posinfo.position;
            let (dx, dy, dz) = (d.x - centre.x, d.y - centre.y, d.z - centre.z);
            (m.id, dx * dx + dy * dy + dz * dz)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .expect("a mine");
    let target = ShipId::from_mine(mine).expect("mine id");
    command::cl_attack(&mut run.universe, &sel, &[target]);
    let cid = run.universe.ships.ship(sel[0]).command.expect("attack command");
    assert_eq!(run.universe.commands.get(cid).map(|c| c.attack_targets.clone()), Some(vec![target]), "the mine is the order's target");
    let mut shots = 0usize;
    let full = run.universe.missiles.missile(mine).health;
    let mut health = full;
    for f in 0..16 * 180 {
        if let Some(m) = run.universe.missiles.get(mine) {
            health = m.health;
        }
        run.step();
        shots += run.universe.bullet_list.iter().filter(|&&b| run.universe.bullets.get(b).is_some_and(|b| b.target == Some(target))).count();
        if run.universe.missiles.get(mine).is_none() {
            eprintln!("mine destroyed after {:.1} s ({full} health, {health} before the last update)", f as f32 / 16.0);
            assert!(health < full, "the mine was not worn down by the Acolytes' fire");
            let attacking = sel.iter().any(|&id| {
                run.universe.ships.get(id).and_then(|s| s.command).and_then(|c| run.universe.commands.get(c)).is_some_and(|c| c.attack_targets.contains(&target))
            });
            assert!(!attacking, "the dead mine is still a target");
            return;
        }
    }
    panic!("the mine survived three minutes ({shots} bullet-frames aimed at it)");
}
