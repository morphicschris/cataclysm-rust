//! The in-game interface's view of the running mission: `hwc_ui::ingame::GameUiHost` over the
//! simulation (`MissionRun`), plus the language strings for names and descriptions.
//!
//! Factories, build lists and jobs come from the construction layer (`cmUpdateShipAvailable`
//! over the build-manager table), labs and technologies from the research layer, launchers from
//! the ships docked inside, the selection from the game's selection list, objectives from the
//! KAS mission.

use hwc_sim::base::{lab_status, Techs, SOF_MODULE};
use hwc_sim::object::sof;
use hwc_sim::statics::{ShipClass, ShipRace, ShipType, Technology, WeaponType};
use hwc_sim::{construction, research, ShipId, Universe};
use hwc_ui::ingame::host::{
    BuildJob, BuildOption, BuildRow, BuildStatus, Command, DockedShip, Factory, Formation,
    GameMessage, GameUiHost, GunInfo, Lab, LabStatus, Launcher, Module, Objective, OrderKind,
    Owner, PlayerInfo, Race, ResearchRow, ShipInfo, Tech, TechState,
};
use hwc_ui::ingame::strings::Strings;

/// The local player (`universe.curPlayerIndex` in single player).
pub const PLAYER: usize = 0;

/// UI ship number of a simulation ship.
pub fn ui_id(id: ShipId) -> u32 {
    u32::from(id.0)
}

pub fn sim_id(id: u32) -> ShipId {
    ShipId(id as u16)
}

/// Everything the host reads, borrowed for one call into the UI.
pub struct SimHost<'a> {
    /// The universe on view: the mission's, or the NIS level's before it.
    pub world: &'a crate::world::World,
    pub strings: &'a Strings,
    pub selection: &'a [ShipId],
    pub selecting: &'a [ShipId],
    pub messages: &'a [GameMessage],
    pub paused: bool,
    pub escape_pending: bool,
    /// The sensors manager is open.
    pub in_sensors: bool,
    /// RUs shown (the mission's `RUsSet` value is copied onto the player as it changes).
    pub resource_units: i32,
    /// Screen position and radius of ships (popup highlights).
    pub screen_pos: &'a dyn Fn(ShipId) -> Option<(i32, i32, i32)>,
}

impl SimHost<'_> {
    fn u(&self) -> &Universe {
        self.world.universe()
    }

    /// The mission's game state (`None` while its NIS level plays).
    fn game(&self) -> Option<&hwc_sim::mission::Game> {
        self.world.mission_run().map(|r| &r.game)
    }

    /// `spMissionEndLabelGet`: "End Mission" in the missions without a roll call, else
    /// "Hyperspace".
    pub fn mission_end_label(&self) -> String {
        mission_end_label(self.strings, self.world.mission())
    }

    fn race(&self) -> ShipRace {
        self.u()
            .players
            .get(PLAYER)
            .map_or(ShipRace::Somtaaw, |p| p.race)
    }

    fn text(&self, key: &str) -> String {
        self.strings.text(key)
    }

    /// Display name of a ship type (`ShipTypeToNiceStr`).
    pub fn ship_name(&self, t: ShipType) -> String {
        self.strings
            .get(&format!("str{}", t.name()))
            .map_or_else(|| t.name().to_ascii_uppercase(), |s| s.trim().to_string())
    }

    fn class_of(&self, t: ShipType) -> Option<ShipClass> {
        self.u().statics.ship(t).and_then(|s| s.ship_class)
    }

    fn class_name(&self, c: ShipClass) -> String {
        let key = match c {
            ShipClass::Destroyer => "strCLASS_SuperCapital".to_string(),
            c => format!("str{}", c.name()),
        };
        self.text(&key).trim().to_string()
    }

    fn mothership(&self) -> Option<ShipId> {
        self.u().base.players.get(PLAYER).and_then(|p| p.mothership)
    }

    fn factory_ships(&self) -> Vec<ShipId> {
        let u = self.u();
        let mut out: Vec<ShipId> = self.mothership().into_iter().collect();
        for f in &u.base.factories {
            let Some(id) = f.ship else { continue };
            if out.contains(&id) {
                continue;
            }
            if u.ship(id).is_some_and(|s| s.player == PLAYER) {
                out.push(id);
            }
        }
        out.truncate(3);
        out
    }

    fn factory(&self, id: ShipId) -> Option<Factory> {
        let s = self.u().ship(id)?;
        Some(Factory {
            ship: ui_id(id),
            ship_type: s.ship_type.index() as u32,
            name: self.ship_name(s.ship_type),
            mothership: Some(id) == self.mothership(),
        })
    }

    fn ship_info(&self, id: ShipId) -> Option<ShipInfo> {
        let u = self.u();
        let s = u.ship(id)?;
        let st = u.statics.ship(s.ship_type)?;
        let class = st.ship_class.unwrap_or(ShipClass::Fighter);
        let owner = if s.player == PLAYER {
            if s.base.disabled {
                Owner::Disabled
            } else {
                Owner::Own
            }
        } else {
            Owner::Enemy
        };
        let order = s
            .command
            .and_then(|c| u.commands.get(c))
            .map_or(OrderKind::Idle, |c| match c.order {
                hwc_sim::command::order::MOVE => OrderKind::Moving,
                hwc_sim::command::order::ATTACK => OrderKind::Attacking,
                hwc_sim::command::order::DOCK | hwc_sim::command::order::LAUNCH_SHIP => {
                    OrderKind::Docking
                }
                hwc_sim::command::order::NULL | hwc_sim::command::order::HALT => OrderKind::Idle,
                _ => OrderKind::Other,
            });
        let guns = st
            .guns
            .as_ref()
            .map(|gs| {
                gs.iter()
                    .enumerate()
                    .map(|(i, g)| {
                        let level = s.guns.get(i).map_or(0, |g| g.level as usize);
                        let kind = WeaponType::from_index(level)
                            .map_or("PROJECTILE".to_string(), |w| weapon_name(w).to_string());
                        GunInfo {
                            size: gun_size_name(g.gun_size).into(),
                            kind,
                            damage: g.damage_mult,
                            upgrade_pending: false,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(ShipInfo {
            ship: ui_id(id),
            ship_type: s.ship_type.index() as u32,
            name: self.ship_name(s.ship_type),
            class: class.index() as u32,
            health: s.health,
            max_health: st.health.max_health,
            upgrades: s.upgrade_flags,
            upgrades_researched: 0,
            upgrades_possible: hwc_sim::upgrade::type_mask(s.ship_type),
            tech_level: 0,
            tech_level_raisable: false,
            rank: 0,
            hotkey_group: None,
            rank_name: String::new(),
            class_name: self.class_name(class),
            owner,
            guns,
            order,
            tactics: s.tactics as i32,
            fuel: None,
            resources: (st.max_resources > 0).then_some(s.base.resources),
        })
    }

    fn tech_has(&self, techs: &Techs, i: usize) -> bool {
        techs.contains(i)
    }
}

fn gun_size_name(s: hwc_sim::statics::GunSize) -> &'static str {
    use hwc_sim::statics::GunSize::*;
    match s {
        Small => "SMALL",
        Medium => "MEDIUM",
        Large => "LARGE",
        VeryLarge => "VERY LARGE",
    }
}

fn weapon_name(w: WeaponType) -> &'static str {
    match w {
        WeaponType::Energy => "ENERGY",
        WeaponType::Ion => "ION",
        WeaponType::PlasmaBomb => "PLASMA",
        WeaponType::Missile => "MISSILE",
        WeaponType::Mine => "MINE",
        WeaponType::Laser => "LASER",
        _ => "PROJECTILE",
    }
}

/// Module (lab) headings of the research list, in lab order.
const LAB_HEADINGS: [&str; 5] = [
    "strHANGAR_HEADING",
    "strARMOUR_HEADING",
    "strENGINEERING_HEADING",
    "strADV_ENGINEERING_HEADING",
    "strWEAPON_HEADING",
];

impl GameUiHost for SimHost<'_> {
    fn player(&self) -> PlayerInfo {
        let u = self.u();
        let p = &u.base.players[PLAYER];
        let pending: i32 = self
            .build_jobs()
            .iter()
            .map(|j| {
                let t = ShipType::from_index(j.ship_type as usize);
                let su = t
                    .and_then(|t| u.statics.ship(t))
                    .map_or(0, |s| s.support_units);
                su * j.count.max(0)
            })
            .sum();
        PlayerInfo {
            race: if self.race() == ShipRace::Beast {
                Race::Beast
            } else {
                Race::Somtaaw
            },
            resource_units: self.resource_units,
            support_capacity: p.su_capacity,
            support_used: p.su_used,
            support_pending: pending,
            ship_count: u
                .ships
                .iter()
                .filter(|s| s.player == PLAYER && s.flags & SOF_MODULE == 0)
                .count() as i32,
            single_player: true,
            multiplayer: false,
            paused: self.paused,
            in_sensors: self.in_sensors,
            tutorial: false,
            mission: self.world.mission() as i32,
        }
    }

    fn game_time(&self) -> f32 {
        self.u().total_time_elapsed
    }

    fn factories(&self) -> Vec<Factory> {
        self.factory_ships()
            .into_iter()
            .filter_map(|id| self.factory(id))
            .collect()
    }

    fn build_list(&self, factory: u32) -> Vec<BuildRow> {
        let u = self.u();
        let fid = sim_id(factory);
        let Some(f) = u.ship(fid) else {
            return Vec::new();
        };
        let Some(fst) = u.statics.ship(f.ship_type) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        // The table's order; what is offered comes from the simulation's
        // `cmUpdateShipAvailable` (level Include/ExcludeShips, research, acquired types).
        for &(flag, value) in construction::build_table(fst.ship_race, fst.can_build_big_ships) {
            if flag == 0 {
                let class = ShipClass::from_index(value as usize);
                let name = class.map_or_else(String::new, |c| self.class_name(c));
                let word = self.text("strCLASS_Class");
                rows.push(BuildRow::Class(format!("{name} {}", word.trim())));
                continue;
            }
            let Some(t) = ShipType::from_index(value as usize) else {
                continue;
            };
            let Some(st) = u.statics.ship(t) else {
                continue;
            };
            let researched = hwc_sim::level::construction_offers(u, fid, t);
            let status = if !researched {
                BuildStatus::Hidden
            } else if !construction::can_build_ship_type(u, fid, t, true)
                || !construction::player_has_support_units_for_type(u, t, PLAYER, 0)
                || !construction::unit_cap_can_create_ship(u, t, fid, 0)
            {
                BuildStatus::Greyed
            } else {
                BuildStatus::Buildable
            };
            rows.push(BuildRow::Ship(BuildOption {
                ship_type: t.index() as u32,
                name: self.ship_name(t),
                class: st.ship_class.map_or(0, |c| c.index() as u32),
                cost: st.build_cost,
                support_units: st.support_units,
                status,
                cap_left: None,
                total_cap: false,
                description: self
                    .strings
                    .get(&format!("str{}Info", t.name()))
                    .unwrap_or("")
                    .to_string(),
            }));
        }
        rows
    }

    fn build_jobs(&self) -> Vec<BuildJob> {
        let u = self.u();
        let mut out = Vec::new();
        for f in &u.base.factories {
            let Some(fid) = f.ship else { continue };
            if !u.ship(fid).is_some_and(|s| s.player == PLAYER) {
                continue;
            }
            for j in f.jobs.iter().filter(|j| j.n_jobs > 0) {
                let Some(t) = j.info else { continue };
                let cost = u.statics.ship(t).map_or(0, |s| s.build_cost);
                let progress = if j.time_total > 0 {
                    1.0 - j.time_left as f32 / j.time_total as f32
                } else {
                    0.0
                };
                out.push(BuildJob {
                    factory: ui_id(fid),
                    ship_type: t.index() as u32,
                    count: j.n_jobs,
                    batch_total: j.n_total.max(j.n_jobs),
                    paused: j.paused,
                    progress: progress.clamp(0.0, 1.0),
                    cost,
                    cost_so_far: j.cost_so_far,
                });
            }
        }
        out
    }

    fn labs(&self) -> Vec<Lab> {
        let u = self.u();
        let p = &u.base.players[PLAYER];
        let table = research::table_for(u, p.research_table);
        let n = if self.race() == ShipRace::Beast { 1 } else { 5 };
        (0..n)
            .map(|i| {
                let lab = p.labs[i];
                let topic = lab
                    .topic
                    .and_then(|id| p.topics.iter().find(|t| t.id == id));
                let progress = topic.map_or(0.0, |t| {
                    let total = table.tech_time.get(t.tech).copied().unwrap_or(0) as f32;
                    if total > 0.0 {
                        (1.0 - t.time_left / total).clamp(0.0, 1.0)
                    } else {
                        0.0
                    }
                });
                Lab {
                    name: self.text(LAB_HEADINGS[i]).trim().to_string(),
                    status: match lab.status {
                        lab_status::IDLE => LabStatus::Idle,
                        lab_status::RESEARCHING => LabStatus::Researching,
                        lab_status::PAUSED => LabStatus::Paused,
                        _ => LabStatus::Absent,
                    },
                    topic: topic.map(|t| t.tech as u32),
                    progress,
                }
            })
            .collect()
    }

    fn techs(&self) -> Vec<Tech> {
        let u = self.u();
        let p = &u.base.players[PLAYER];
        let table = research::table_for(u, p.research_table);
        let prefix = if self.race() == ShipRace::Beast {
            "R2"
        } else {
            "R1"
        };
        Technology::ALL
            .iter()
            .map(|&t| {
                let i = t.index();
                let depends_met = table.tech_depend[i]
                    .iter()
                    .all(|d| self.tech_has(&p.tech, d.index()));
                let lab = research::tech_lab(i);
                let lab_present = if self.race() == ShipRace::Beast {
                    p.labs[0].status != lab_status::ABSENT
                } else {
                    lab >= 0 && p.labs[lab as usize].status != lab_status::ABSENT
                };
                let state = if self.tech_has(&p.tech, i) {
                    TechState::Researched
                } else if p.topics.iter().any(|x| x.tech == i) {
                    TechState::Researching
                } else if depends_met && lab_present && table.tech_time[i] > 0 {
                    TechState::Available
                } else {
                    TechState::Unavailable
                };
                Tech {
                    id: i as u32,
                    name: self.text(&format!("str{}", t.name())).trim().to_string(),
                    cost: 0,
                    time: table.tech_time[i] as f32,
                    lab: lab.max(0) as usize,
                    state,
                    upgrade: t.name().contains("ADRIVE")
                        || t.name().ends_with('2')
                        || t.name().ends_with('3'),
                    description: self.text(&format!("str{prefix}{}info", t.name())),
                    key: t.name().to_string(),
                }
            })
            .collect()
    }

    fn research_rows(&self) -> Vec<hwc_ui::ingame::host::ResearchRow> {
        let techs = self.techs();
        let mut rows = Vec::new();
        let labs = if self.race() == ShipRace::Beast { 1 } else { 5 };
        for (lab, heading) in LAB_HEADINGS.iter().enumerate().take(labs) {
            rows.push(ResearchRow::Header {
                name: self.text(heading).trim().to_string(),
            });
            for t in techs
                .iter()
                .filter(|t| (labs == 1 || t.lab == lab) && t.state != TechState::Unavailable)
            {
                rows.push(ResearchRow::Tech(t.id));
            }
        }
        rows
    }

    fn launchers(&self) -> Vec<Launcher> {
        let u = self.u();
        let p = &u.base.players[PLAYER];
        self.factory_ships()
            .into_iter()
            .filter_map(|id| {
                let s = u.ship(id)?;
                let st = u.statics.ship(s.ship_type)?;
                let inside = s.base.inside.as_ref();
                let docked = inside
                    .map(|i| {
                        i.ships
                            .iter()
                            .filter_map(|&d| {
                                let ds = u.ship(d)?;
                                Some(DockedShip {
                                    ship: ui_id(d),
                                    ship_type: ds.ship_type.index() as u32,
                                    class: self
                                        .class_of(ds.ship_type)
                                        .map_or(6, |c| c.index() as u32),
                                    name: self.ship_name(ds.ship_type),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let fighters = inside.map_or(0, |i| i32::from(i.fighters));
                let corvettes = inside.map_or(0, |i| i32::from(i.corvettes));
                let bit = hwc_sim::dock::auto_launch_bit(u, id);
                Some(Launcher {
                    factory: self.factory(id)?,
                    docked,
                    auto_launch: p.auto_launch & bit != 0,
                    fighters: (fighters, i32::from(st.max_dockable_fighters)),
                    corvettes: (corvettes, i32::from(st.max_dockable_corvettes)),
                })
            })
            .collect()
    }

    fn modules(&self) -> Vec<Module> {
        let u = self.u();
        let Some(ms) = self.mothership() else {
            return Vec::new();
        };
        let modules: Vec<ShipId> = u
            .ship_list
            .iter()
            .copied()
            .filter(|&id| {
                u.ship(id).is_some_and(|s| {
                    s.player == PLAYER
                        && hwc_sim::modules::is_module(s.ship_type)
                        && (s.base.clamp.as_ref().and_then(|c| c.host) == Some(ms)
                            || self
                                .world
                                .level()
                                .clamps
                                .iter()
                                .any(|(m, h)| *m == id && *h == ms))
                })
            })
            .collect();
        modules
            .into_iter()
            .take(7)
            .filter_map(|id| {
                let s = u.ship(id)?;
                let max = u.statics.ship(s.ship_type)?.health.max_health;
                Some(Module {
                    ship: ui_id(id),
                    ship_type: s.ship_type.index() as u32,
                    name: self.ship_name(s.ship_type),
                    health: if max > 0.0 { s.health / max } else { 1.0 },
                    repair_priority: None,
                })
            })
            .collect()
    }

    fn selection(&self) -> Vec<ShipInfo> {
        self.selection
            .iter()
            .filter_map(|&id| self.ship_info(id))
            .collect()
    }

    fn selecting(&self) -> Vec<ShipInfo> {
        self.selecting
            .iter()
            .filter_map(|&id| self.ship_info(id))
            .collect()
    }

    fn available_commands(&self) -> Vec<Command> {
        let u = self.u();
        if self.selection.is_empty() {
            return Vec::new();
        }
        let mut c = vec![
            Command::Move,
            Command::Cancel,
            Command::Scuttle,
            Command::TacticsEvasive,
            Command::TacticsNeutral,
            Command::TacticsAggressive,
        ];
        let ships: Vec<_> = self.selection.iter().filter_map(|&id| u.ship(id)).collect();
        if !hwc_sim::command::attack_capable(u, self.selection).is_empty() {
            c.push(Command::Attack);
        }
        if ships.iter().any(|s| {
            !matches!(
                self.class_of(s.ship_type),
                Some(ShipClass::Mothership | ShipClass::Carrier | ShipClass::Destroyer)
            )
        }) {
            c.push(Command::Dock);
            c.push(Command::Retire);
        }
        if ships
            .iter()
            .any(|s| hwc_sim::resource::is_worker(s.ship_type))
        {
            c.push(Command::Harvest);
        }
        if ships.len() > 1 {
            for f in [
                Formation::Delta,
                Formation::Broad,
                Formation::Delta3D,
                Formation::Claw,
                Formation::Wall,
            ] {
                c.push(Command::Formation(f));
            }
        }
        c
    }

    fn objectives(&self) -> Vec<Objective> {
        // `moObjectiveAddFirst`: the "hyperspace" objective goes to the top and shows the
        // jump's label (`spMissionEndLabelGet`).
        let mut v: Vec<Objective> = Vec::new();
        for o in self.game().map_or(&[][..], |g| &g.objectives) {
            let hyperspace = o.label.eq_ignore_ascii_case("hyperspace");
            let obj = Objective {
                text: if hyperspace {
                    self.mission_end_label()
                } else {
                    o.brief.clone()
                },
                description: o.full.clone(),
                hyperspace,
                primary: o.primary,
                complete: o.complete,
            };
            if hyperspace {
                v.insert(0, obj);
            } else {
                v.push(obj);
            }
        }
        v
    }

    fn hyperspace_label(&self) -> Option<String> {
        // `singlePlayerCanHyperspaceGet`.
        self.game()?
            .hyper
            .player_can_hyperspace
            .then(|| self.mission_end_label())
    }

    fn roll_call(&self) -> Option<hwc_ui::ingame::host::RollCall> {
        let game = self.game()?;
        let h = &game.hyper;
        let up = h.rollcall_screen
            && game.sp_state == hwc_sim::mission::sp_state::WAITING_ROLLCALL;
        up.then(|| hwc_ui::ingame::host::RollCall {
            docked: h.rollcall_docked,
            remaining: h.rollcall_remaining,
            go_now_disabled: hwc_sim::campaign::rollcall_go_now_disabled(self.u()),
        })
    }

    fn messages(&self) -> Vec<GameMessage> {
        self.messages.to_vec()
    }

    fn fleet_intel(&self) -> String {
        self.game()
            .map_or(&[][..], |g| &g.objectives)
            .iter()
            .map(|o| o.full.clone())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn escape_pending(&self) -> bool {
        self.escape_pending
    }

    fn ship_screen_pos(&self, ship: u32) -> Option<(i32, i32, i32)> {
        (self.screen_pos)(sim_id(ship))
    }
}

/// Whether the local player may select this ship.
pub fn selectable(u: &Universe, id: ShipId) -> bool {
    u.ship(id).is_some_and(|s| {
        s.player == PLAYER
            && s.flags & (sof::HIDE | sof::DEAD) == 0
            && s.flags & SOF_MODULE == 0
    })
}

/// `spMissionEndLabelGet` for a mission.
pub fn mission_end_label(strings: &Strings, mission: u32) -> String {
    let key = if hwc_sim::campaign::skips_roll_call(mission) {
        "strSinglePlayerNextLevel"
    } else {
        "strSinglePlayerHyperspace"
    };
    strings.get(key).unwrap_or(if hwc_sim::campaign::skips_roll_call(mission) { "End Mission" } else { "Hyperspace" }).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_start_with_fighters() {
        assert_eq!(
            construction::build_table(ShipRace::Somtaaw, true)[0],
            (0, 6)
        );
        assert!(construction::build_table(ShipRace::Somtaaw, true).contains(&(1, 14)));
        assert_eq!(sim_id(ui_id(ShipId(42))), ShipId(42));
    }
}

