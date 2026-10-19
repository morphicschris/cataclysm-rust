//! A stand-in game for tests and screenshots: a Somtaaw (or Beast) fleet with a mothership,
//! two carriers, some research and a selection. It applies the UI's requests to its own state
//! the way the game would (queues, refunds, labs, launches), so flows can be tested end to end.

use crate::ingame::host::*;

/// Mothership and carrier ids of the mock fleet.
pub const MOTHERSHIP: ShipId = 1;
pub const CARRIER_1: ShipId = 2;
pub const CARRIER_2: ShipId = 3;

#[derive(Debug, Clone)]
pub struct MockHost {
    pub player: PlayerInfo,
    pub time: f32,
    pub factories: Vec<Factory>,
    pub options: Vec<BuildRow>,
    pub jobs: Vec<BuildJob>,
    pub labs: Vec<Lab>,
    pub techs: Vec<Tech>,
    pub rows: Vec<ResearchRow>,
    pub launchers: Vec<Launcher>,
    pub modules: Vec<Module>,
    pub beast_systems: u32,
    pub selection: Vec<ShipInfo>,
    pub commands: Vec<Command>,
    pub objectives: Vec<Objective>,
    pub messages: Vec<GameMessage>,
    pub fleet_intel: String,
    pub paused: bool,
    /// Every request applied, in order.
    pub applied: Vec<GameRequest>,
    next_ship: ShipId,
}

fn factory(ship: ShipId, ship_type: ShipType, name: &str, mothership: bool) -> Factory {
    Factory {
        ship,
        ship_type,
        name: name.into(),
        mothership,
    }
}

fn option(
    ship_type: ShipType,
    name: &str,
    class: u32,
    cost: i32,
    su: i32,
    status: BuildStatus,
) -> BuildRow {
    BuildRow::Ship(BuildOption {
        ship_type,
        name: name.into(),
        class,
        cost,
        support_units: su,
        status,
        cap_left: None,
        total_cap: false,
        description: format!("{name}: a ship of the Kiith Somtaaw fleet."),
    })
}

fn class(name: &str) -> BuildRow {
    BuildRow::Class(format!("{name} CLASS"))
}

fn tech(id: TechId, key: &str, name: &str, lab: usize, state: TechState) -> Tech {
    Tech {
        id,
        name: name.into(),
        cost: 500 + id as i32 * 100,
        time: 60.0 + id as f32 * 10.0,
        lab,
        state,
        description: format!("{name}.\n\nResearch description of {name}."),
        upgrade: key.contains("ADRIVE") || key.ends_with('2'),
        key: key.into(),
    }
}

fn ship(id: ShipId, ship_type: ShipType, name: &str, class: u32, health: f32) -> ShipInfo {
    ShipInfo {
        ship: id,
        ship_type,
        name: name.into(),
        class,
        health,
        max_health: 1000.0,
        upgrades: 0x1,
        upgrades_researched: 0x3,
        upgrades_possible: 0x7,
        tech_level: 1,
        tech_level_raisable: true,
        rank: 0,
        hotkey_group: None,
        rank_name: "ROOKIE".into(),
        class_name: if class == 0 { "FIGHTER" } else { "FRIGATE" }.into(),
        owner: Owner::Own,
        guns: vec![GunInfo {
            size: "SMALL".into(),
            kind: "PROJECTILE".into(),
            damage: 12.0,
            upgrade_pending: true,
        }],
        order: OrderKind::Idle,
        tactics: 1,
        fuel: None,
        resources: None,
    }
}

impl Default for MockHost {
    fn default() -> Self {
        Self::somtaaw()
    }
}

impl MockHost {
    /// A Somtaaw fleet in the middle of a single-player mission.
    pub fn somtaaw() -> Self {
        let factories = vec![
            factory(MOTHERSHIP, 0x12, "COMMAND SHIP", true),
            factory(CARRIER_1, 0x0c, "CARRIER", false),
        ];
        use BuildStatus::{Buildable as B, Greyed as G, Hidden as H};
        let options = vec![
            class("FIGHTER"),
            option(0, "RECON", 6, 75, 1, B),
            option(1, "ACOLYTE", 6, 85, 2, B),
            option(133, "SUPER ACOLYTE", 6, 120, 2, H),
            option(2, "MIMIC", 6, 180, 2, G),
            class("CORVETTE"),
            option(4, "ACV", 5, 250, 4, H),
            option(5, "MCV", 5, 250, 4, H),
            class("FRIGATE"),
            option(6, "RAMMING FRIGATE", 4, 500, 10, B),
            option(7, "MULTIBEAM FRIGATE", 4, 750, 12, B),
            option(8, "HIVE FRIGATE", 4, 650, 12, B),
            class("SUPER-CAPITAL"),
            option(25, "DESTROYER", 3, 2200, 38, B),
            option(10, "DREADNOUGHT", 3, 4000, 80, H),
            option(12, "CARRIER", 2, 2000, 0, B),
            class("MICROSHIP"),
            option(3, "LEECH", 11, 150, 2, H),
            option(17, "SENTINEL", 11, 125, 2, B),
            class("NON-COMBAT"),
            option(14, "WORKER", 7, 250, 3, B),
            option(16, "PROCESSOR", 7, 850, 8, B),
            class("COMPONENT"),
            option(26, "SUPPORT MODULE", 10, 400, 0, B),
            option(24, "HANGAR MODULE", 10, 1000, 0, G),
            option(22, "ENGINEERING MODULE", 10, 1000, 0, B),
        ];
        let jobs = vec![
            BuildJob {
                factory: MOTHERSHIP,
                ship_type: 0x01,
                count: 3,
                batch_total: 5,
                paused: false,
                progress: 0.4,
                cost: 85,
                cost_so_far: 34,
            },
            BuildJob {
                factory: MOTHERSHIP,
                ship_type: 14,
                count: 1,
                batch_total: 1,
                paused: true,
                progress: 0.75,
                cost: 250,
                cost_so_far: 187,
            },
        ];
        let lab = |name: &str, status, topic, progress| Lab {
            name: name.into(),
            status,
            topic,
            progress,
        };
        let labs = vec![
            lab("HANGAR MODULE", LabStatus::Researching, Some(2), 0.3),
            lab("ARMOR MODULE", LabStatus::Researching, Some(5), 0.8),
            lab("ENGINEERING MODULE", LabStatus::Idle, None, 0.0),
            lab("ADV ENGINEERING MODULE", LabStatus::Absent, None, 0.0),
            lab("WEAPONS MODULE", LabStatus::Idle, None, 0.0),
        ];
        let techs = vec![
            tech(
                1,
                "TECH_DRIVE_FIGHTER",
                "FIGHTER DRIVE",
                0,
                TechState::Researched,
            ),
            tech(
                2,
                "TECH_ADRIVE_FIGHTER",
                "ADVANCED FIGHTER DRIVE",
                0,
                TechState::Researching,
            ),
            tech(
                3,
                "TECH_LINK",
                "LINKING TECHNOLOGY",
                0,
                TechState::Available,
            ),
            tech(
                4,
                "TECH_DRIVE_FRIGATE",
                "FRIGATE DRIVE",
                0,
                TechState::Unavailable,
            ),
            tech(
                5,
                "TECH_ARMOUR2",
                "ARMOR PLATING 2",
                1,
                TechState::Researching,
            ),
            tech(6, "TECH_MIMIC", "MIMIC", 2, TechState::Available),
            tech(7, "TECH_REPAIR", "REPAIR", 2, TechState::Researched),
            tech(8, "TECH_ION", "ION CANNON", 4, TechState::Available),
            tech(9, "TECH_MISSILE", "MISSILES", 4, TechState::Unavailable),
            tech(10, "TECH_ENERGY", "ENERGY CANNON", 4, TechState::Researched),
        ];
        let header = |n: &str| ResearchRow::Header { name: n.into() };
        let rows = vec![
            header("HANGAR MODULE"),
            ResearchRow::Tech(1),
            ResearchRow::Tech(2),
            ResearchRow::Tech(3),
            ResearchRow::Tech(4),
            header("ARMOR MODULE"),
            ResearchRow::Tech(5),
            header("ENGINEERING MODULE"),
            ResearchRow::Tech(6),
            ResearchRow::Tech(7),
            header("ADV ENGINEERING MODULE"),
            header("WEAPONS MODULE"),
            ResearchRow::Tech(8),
            ResearchRow::Tech(9),
            ResearchRow::Tech(10),
        ];
        let docked = |ids: &[(ShipId, ShipType, &str)]| {
            ids.iter()
                .map(|&(ship, ship_type, name)| DockedShip {
                    ship,
                    ship_type,
                    class: match ship_type {
                        4 | 5 => 5,
                        14 => 7,
                        _ => 6,
                    },
                    name: name.into(),
                })
                .collect::<Vec<_>>()
        };
        let launchers = vec![
            Launcher {
                factory: factories[0].clone(),
                docked: docked(&[
                    (20, 0x01, "ACOLYTE"),
                    (21, 0x01, "ACOLYTE"),
                    (22, 0x01, "ACOLYTE"),
                    (23, 0, "RECON"),
                    (24, 14, "WORKER"),
                ]),
                auto_launch: false,
                fighters: (4, 15),
                corvettes: (0, 6),
            },
            Launcher {
                factory: factories[1].clone(),
                docked: docked(&[(25, 2, "MIMIC")]),
                auto_launch: true,
                fighters: (1, 10),
                corvettes: (0, 4),
            },
        ];
        let module = |ship, ship_type, name: &str, health| Module {
            ship,
            ship_type,
            name: name.into(),
            health,
            repair_priority: Some(1),
        };
        let modules = vec![
            module(40, 0x18, "HANGAR MODULE", 1.0),
            module(41, 0x17, "WEAPONS MODULE", 0.62),
            module(42, 0x15, "ARMOR MODULE", 0.3),
            module(43, 0x16, "ENGINEERING MODULE", 0.9),
        ];
        let mut selection = vec![
            ship(10, 0x01, "ACOLYTE", 0, 820.0),
            ship(11, 0x01, "ACOLYTE", 0, 400.0),
            ship(12, 8, "HIVE FRIGATE", 2, 950.0),
        ];
        selection[0].hotkey_group = Some(1);
        selection[2].order = OrderKind::Attacking;
        Self {
            player: PlayerInfo {
                race: Race::Somtaaw,
                resource_units: 3450,
                multiplayer: false,
                paused: false,
                in_sensors: false,
                support_capacity: 60,
                support_used: 31,
                support_pending: 6,
                ship_count: 24,
                single_player: true,
                tutorial: false,
                mission: 3,
            },
            time: 754.0,
            factories,
            options,
            jobs,
            labs,
            techs,
            rows,
            launchers,
            modules,
            beast_systems: 4,
            selection,
            commands: vec![
                Command::Move,
                Command::Attack,
                Command::Guard,
                Command::Dock,
                Command::Cancel,
                Command::Scuttle,
                Command::Link,
                Command::TacticsEvasive,
                Command::TacticsNeutral,
                Command::TacticsAggressive,
                Command::Formation(Formation::Delta),
                Command::Formation(Formation::Wall),
            ],
            objectives: vec![
                Objective {
                    text: "Defend the Kuun-Lan".into(),
                    description: "The Kuun-Lan is under attack by an unknown force. Defend                                   the Command Ship until reinforcements arrive."
                        .into(),
                    hyperspace: false,
                    primary: true,
                    complete: false,
                },
                Objective {
                    text: "Harvest the asteroid field".into(),
                    description: "Harvest the nearby asteroids to fund new construction."
                        .into(),
                    hyperspace: false,
                    primary: false,
                    complete: true,
                },
            ],
            messages: vec![
                GameMessage {
                    text: "Construction complete.".into(),
                    colour: [255, 255, 255, 255],
                    ship: Some(10),
                    time: 748.0,
                },
                GameMessage {
                    text: "Research complete: Fighter Drive.".into(),
                    colour: [0, 255, 0, 255],
                    ship: None,
                    time: 750.0,
                },
            ],
            fleet_intel: "The Kuun-Lan is under attack by an unknown force. Defend the \
                          Command Ship until reinforcements arrive.\n\nHarvest the nearby \
                          asteroids to fund new construction."
                .into(),
            paused: false,
            applied: Vec::new(),
            next_ship: 100,
        }
    }

    /// The same fleet as the Beast (one lab, Beast screens).
    pub fn beast() -> Self {
        let mut h = Self::somtaaw();
        h.player.race = Race::Beast;
        h.factories[0].name = "BEAST MOTHERSHIP".into();
        h.factories[0].ship_type = 0x35;
        h.launchers[0].factory = h.factories[0].clone();
        h.beast_systems = 4;
        h.labs.truncate(1);
        h.modules.clear();
        h
    }

    /// Applies what the UI asked for, as the game would.
    pub fn apply(&mut self, req: &GameRequest) {
        self.applied.push(req.clone());
        match req {
            GameRequest::Build {
                factory,
                ship_type,
                count,
            } => {
                let (cost, su) = self
                    .option(*ship_type)
                    .map_or((0, 0), |o| (o.cost, o.support_units));
                self.player.support_pending += su * count;
                match self
                    .jobs
                    .iter_mut()
                    .find(|j| j.factory == *factory && j.ship_type == *ship_type)
                {
                    Some(j) => {
                        j.count += count;
                        j.batch_total += count;
                    }
                    None => self.jobs.push(BuildJob {
                        factory: *factory,
                        ship_type: *ship_type,
                        count: *count,
                        batch_total: *count,
                        paused: false,
                        progress: 0.0,
                        cost,
                        cost_so_far: 0,
                    }),
                }
            }
            GameRequest::Unqueue {
                factory,
                ship_type,
                count,
            } => {
                let su = self.option(*ship_type).map_or(0, |o| o.support_units);
                if let Some(j) = self
                    .jobs
                    .iter_mut()
                    .find(|j| j.factory == *factory && j.ship_type == *ship_type)
                {
                    let n = (*count).min(j.count - 1).max(0);
                    j.count -= n;
                    j.batch_total -= n;
                    self.player.support_pending -= su * n;
                }
            }
            GameRequest::CancelBuild { factory, ship_type } => {
                let su = self.option(*ship_type).map_or(0, |o| o.support_units);
                if let Some(i) = self
                    .jobs
                    .iter()
                    .position(|j| j.factory == *factory && j.ship_type == *ship_type)
                {
                    let j = self.jobs.remove(i);
                    self.player.resource_units += j.cost_so_far;
                    self.player.support_pending -= su * j.count;
                }
            }
            GameRequest::PauseBuild {
                factory,
                ship_type,
                paused,
            } => {
                for j in self
                    .jobs
                    .iter_mut()
                    .filter(|j| j.factory == *factory && j.ship_type == *ship_type)
                {
                    j.paused = *paused;
                }
            }
            GameRequest::Research { tech, lab } => {
                if let Some(l) = self.labs.get_mut(*lab) {
                    l.status = LabStatus::Researching;
                    l.topic = Some(*tech);
                    l.progress = 0.0;
                }
                if let Some(t) = self.techs.iter_mut().find(|t| t.id == *tech) {
                    t.state = TechState::Researching;
                }
            }
            GameRequest::ClearLab { lab } => {
                if let Some(l) = self.labs.get_mut(*lab) {
                    if let Some(topic) = l.topic.take() {
                        if let Some(t) = self.techs.iter_mut().find(|t| t.id == topic) {
                            t.state = TechState::Available;
                        }
                    }
                    if l.status != LabStatus::Absent {
                        l.status = LabStatus::Idle;
                    }
                    l.progress = 0.0;
                }
            }
            GameRequest::Launch { launcher, ships } => {
                if let Some(l) = self
                    .launchers
                    .iter_mut()
                    .find(|l| l.factory.ship == *launcher)
                {
                    l.docked.retain(|d| !ships.contains(&d.ship));
                }
            }
            GameRequest::SetAutoLaunch { launcher, on } => {
                if let Some(l) = self
                    .launchers
                    .iter_mut()
                    .find(|l| l.factory.ship == *launcher)
                {
                    l.auto_launch = *on;
                }
            }
            GameRequest::BeastSystems(v) => self.beast_systems = *v,
            GameRequest::SetPaused(p) => self.paused = *p,
            _ => {}
        }
    }

    /// Advances the build queues and labs by `dt` seconds (completed ships are dropped).
    pub fn tick(&mut self, dt: f32) {
        self.time += dt;
        for j in self.jobs.iter_mut().filter(|j| !j.paused) {
            let t = (j.cost as f32 / 20.0).max(0.1);
            j.progress += dt / t;
            j.cost_so_far = (j.progress.min(1.0) * j.cost as f32) as i32;
            while j.progress >= 1.0 && j.count > 0 {
                j.progress -= 1.0;
                j.count -= 1;
                self.player.resource_units -= j.cost;
                self.next_ship += 1;
            }
        }
        self.jobs.retain(|j| j.count > 0);
        for l in self
            .labs
            .iter_mut()
            .filter(|l| l.status == LabStatus::Researching)
        {
            l.progress = (l.progress + dt / 60.0).min(1.0);
        }
    }

    /// A ship of the construction list.
    pub fn option(&self, ship_type: ShipType) -> Option<&BuildOption> {
        self.options.iter().find_map(|r| match r {
            BuildRow::Ship(o) if o.ship_type == ship_type => Some(o),
            _ => None,
        })
    }
}

impl GameUiHost for MockHost {
    fn player(&self) -> PlayerInfo {
        self.player.clone()
    }
    fn game_time(&self) -> f32 {
        self.time
    }
    fn factories(&self) -> Vec<Factory> {
        self.factories.clone()
    }
    fn build_list(&self, _factory: ShipId) -> Vec<BuildRow> {
        self.options.clone()
    }
    fn build_jobs(&self) -> Vec<BuildJob> {
        self.jobs.clone()
    }
    fn labs(&self) -> Vec<Lab> {
        self.labs.clone()
    }
    fn techs(&self) -> Vec<Tech> {
        self.techs.clone()
    }
    fn research_rows(&self) -> Vec<ResearchRow> {
        self.rows.clone()
    }
    fn launchers(&self) -> Vec<Launcher> {
        self.launchers.clone()
    }
    fn modules(&self) -> Vec<Module> {
        self.modules.clone()
    }
    fn beast_systems(&self) -> u32 {
        self.beast_systems
    }
    fn selection(&self) -> Vec<ShipInfo> {
        self.selection.clone()
    }
    fn available_commands(&self) -> Vec<Command> {
        self.commands.clone()
    }
    fn objectives(&self) -> Vec<Objective> {
        self.objectives.clone()
    }
    fn messages(&self) -> Vec<GameMessage> {
        self.messages.clone()
    }
    fn fleet_intel(&self) -> String {
        self.fleet_intel.clone()
    }
    fn trader_offers(&self) -> Vec<Tech> {
        self.techs.iter().take(4).cloned().collect()
    }
    fn ship_screen_pos(&self, ship: ShipId) -> Option<(i32, i32, i32)> {
        let i = self.selection.iter().position(|s| s.ship == ship)? as i32;
        Some((260 + i * 60, 220 + (i % 2) * 40, 18))
    }
}
