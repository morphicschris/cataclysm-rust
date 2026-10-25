//! The NIS support functions over the real game data (`HWC_GAME_DIR` or `HWC_EXTRACTED_DIR`;
//! skipped when neither is set): the `fire` event of the shipped NIS files, NIS missiles and
//! the hide toggle.

use std::sync::{Arc, OnceLock};

use hwc_formats::big::BigArchive;
use hwc_math::Vector;
use hwc_sim::base::{self, CreateOptions};
use hwc_sim::nis_support;
use hwc_sim::object::sof;
use hwc_sim::statics::{DataSource, DirSource, LoadOptions, ShipRace, ShipType, StaticWorld};
use hwc_sim::{SimTweaks, Universe};

struct Data {
    statics: Arc<StaticWorld>,
    tweaks: SimTweaks,
}

fn data() -> Option<&'static Data> {
    static DATA: OnceLock<Option<Data>> = OnceLock::new();
    DATA.get_or_init(|| {
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
    })
    .as_ref()
}

fn universe() -> Option<Universe> {
    let d = data()?;
    let mut u = Universe::new(d.statics.clone(), d.tweaks.clone(), &[ShipRace::Somtaaw, ShipRace::Taiidan]);
    base::init(&mut u);
    Some(u)
}

#[test]
fn nis_fire_shoots_the_interceptors_active_gun_without_a_target() {
    let Some(mut u) = universe() else {
        eprintln!("skipped: no game data");
        return;
    };
    let id = base::create_ship(&mut u, ShipType::tHeavyInterceptor, 1, Vector::ZERO, CreateOptions::default()).unwrap();
    let before = u.bullets.iter().count();
    let gun = u.ships.ship(id).interceptor.active_gun;
    assert!(nis_support::fire(&mut u, id));
    assert_eq!(u.bullets.iter().count(), before + 1);
    assert!(u.bullets.iter().all(|b| b.target.is_none()));
    let n = u.ships.ship(id).guns.len() as i32;
    assert_eq!(u.ships.ship(id).interceptor.active_gun as i32, (gun as i32 + 1) % n);
}

#[test]
fn nis_missile_lives_as_long_as_the_nis() {
    let Some(mut u) = universe() else {
        eprintln!("skipped: no game data");
        return;
    };
    let p = Vector::new(100.0, -200.0, 300.0);
    let Some(id) = nis_support::create_nis_missile(&mut u, ShipRace::Taiidan, p, 42.0) else {
        eprintln!("skipped: no Taiidan missile statics");
        return;
    };
    let m = u.missiles.missile(id);
    assert_eq!(m.posinfo.position, p);
    assert_eq!(m.total_lifetime, 42.0);
    assert_eq!(m.player, Some(1));
    assert_eq!(m.bullet_type, 9);
    assert_eq!(m.damage, 48.0);
    assert!(m.target.is_none() && m.owner.is_none());
    assert_ne!(m.flags & sof::NIS_SHIP, 0);
    assert!(u.missile_list.contains(&id));

    nis_support::hide_just_about_everything(&mut u);
    assert_eq!(u.missiles.missile(id).flags & sof::HIDE, 0, "NIS objects stay visible");
}
