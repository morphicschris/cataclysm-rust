//! KAS runtime functions on the simulation: see the parent module.
//!
//! The `kasf*` functions of this group are written as methods of the `KasRuntime` impl in
//! the macro below, which the parent invokes inside that impl.
//!
//! This group holds the computer-player switches (technology, AI features, the salvage depot,
//! the Mimic / MCV disguise menus) and the presentation functions (objectives, pings, tutorial
//! pointers, camera, wide screen, sound, subtitles, screen lockout). Where the simulation has a
//! model of the state the original changes, the method changes it; UI, sound and camera
//! requests without a field on [`crate::mission::Game`] are logged as `LogKind::Flow`.

#![allow(unused_imports)]

use crate::ships::mimic::DisguiseTables;

/// The disguise-menu slot a KAS item type names (`kasfSetCurrentMimicItem`,
/// `kasfSetMCVItemIndex`): 0 is the player, 1 the computer player, 2 the asteroid list (slot 8);
/// any other value is used as a slot number (its low byte), as the original does.
pub(super) fn disguise_slot(item_type: i32) -> usize {
    match item_type {
        0 => 0,
        1 => 1,
        2 => 8,
        n => (n as u32 & 0xff) as usize,
    }
}

/// `kasfSetCurrentMimicItem` on the disguise menus: `MimicAddDisguiseType(slot, type)`, the
/// type being `StrToShipType` of the name (-1 when unknown). Slot 8 resets the asteroid list.
pub(super) fn add_mimic_item(tables: &mut DisguiseTables, item_type: i32, ship_type: i32) {
    tables.add_type(disguise_slot(item_type), ship_type);
}

/// `kasfSetMCVItemIndex` on the disguise menus: `MimicSetDisguiseDefault(slot, index)`.
pub(super) fn set_mcv_item_index(tables: &mut DisguiseTables, item_type: i32, index: i32) {
    tables.set_default(disguise_slot(item_type), index);
}

/// Objective labels as `objectiveSet` compares them: `strncmp(stored, label, 32)` against the
/// stored label, which `objectiveAndFleetIntelligenceCreate` kept to 31 characters. Case
/// matters.
pub(super) fn objective_label_matches(stored: &str, label: &str) -> bool {
    let a = stored.as_bytes();
    let b = label.as_bytes();
    a[..a.len().min(31)] == b[..b.len().min(32)]
}

/// The labels `kasfPingMultipleAddShips` gives its pings: `"%s%d"` of the label and the ship's
/// index in the selection.
pub(super) fn multiple_ping_labels(label: &str, count: usize) -> Vec<String> {
    (0..count).map(|i| format!("{label}{i}")).collect()
}

impl super::SimHost<'_> {
    /// `spMainScreen(1)` / `spMainScreenAndLockout`: back to the main screen, closing the
    /// sensors manager for good (the KAS sensors flag is left as it is). The build, research and
    /// launch managers and the front-end menus it also closes have no model.
    pub(super) fn main_screen(&mut self) {
        if self.g.sensors_open {
            self.g.sensors_open = false;
        }
    }

    /// `tutSetPointerTargetShip`: a tutorial pointer of that name (case-insensitive) is aimed
    /// again, otherwise a new one is made. Only the names are recorded.
    pub(super) fn set_ship_pointer(&mut self, name: String) {
        if !self.g.pointers.iter().any(|p| p.eq_ignore_ascii_case(&name)) {
            self.g.pointers.push(name);
        }
    }

    /// The focus of `kasfTutCameraFocus` (`ccFocus`) / `kasfCameraFocusClose`
    /// (`ccFocusCloseScripted`, a cut closing in) for the front end's camera. A non-empty
    /// selection first goes to a clean main screen (the original also leaves point mode); an
    /// empty one focuses the player's current selection, which only the front end has.
    pub(super) fn scripted_focus(&mut self, close: bool, ships: Vec<crate::object::ShipId>) {
        if !ships.is_empty() {
            self.main_screen();
            self.g.camera.focus = ships.clone();
            self.g.camera.scripted = true;
        }
        self.g.camera_orders.push(crate::mission::CameraOrder::Focus { ships, close, cut: close, unless_same: false });
    }

    /// The angle and declination of [`Game::camera`](crate::mission::Game) (degrees) to the
    /// front end's camera, in radians (`ccChangeAngleDeclination` + `cameraSetEyePosition`).
    pub(super) fn camera_angles(&mut self) {
        let c = &self.g.camera;
        let (angle, declination) = (c.angle * 0.017453292, c.declination * 0.017453292);
        self.g.camera_orders.push(crate::mission::CameraOrder::AngleDeclination { angle, declination });
    }
}

macro_rules! ai_ui_methods {
    () => {
        // ---- computer player -------------------------------------------------------------

        /// `kasfGiveTechToAI` (0x494230): an empty name clears the computer player's
        /// technologies (`rmTechSetClear`: no research update follows); otherwise the
        /// technology is added and the player is updated from research. An unknown name only
        /// warns.
        fn give_tech_to_ai(&mut self, _cx: &Cx, sz_tech_name: Str<'_>) {
            let name = Self::str_text(sz_tech_name);
            let ai = level::COMPUTER_PLAYER;
            if name.is_empty() {
                if let Some(p) = self.u.base.players.get_mut(ai) {
                    p.tech = crate::base::Techs::default();
                }
                return;
            }
            match crate::statics::Technology::from_name(&name) {
                Some(tech) => {
                    crate::fow::tech_add(self.u, ai, tech.index());
                    crate::upgrade::player_update_from_research(self.u, ai);
                }
                None => self.log(LogKind::Warning, format!("GiveTechToAI: {name:?} is not a valid technology")),
            }
        }

        /// `kasfDisableAllAIFeatures` (0x48f560): zeroes the current computer player's
        /// resource, attack and defence feature masks. The simulation has no computer-player
        /// (aiplayer) model to hold them, so the call is logged.
        fn disable_all_ai_features(&mut self, _cx: &Cx) {
            self.log(LogKind::Flow, "DisableAllAIFeatures: computer player's resource/attack/defence features off (no AI player model)");
        }

        /// `kasfSetAIDepotShip` (0x495a80): `salvSetGlobal` with the selection's first ship,
        /// the depot single-player salvagers of the computer player tow their targets to
        /// (when it accepts them). The salvage code has no field for it yet: logged.
        fn set_ai_depot_ship(&mut self, _cx: &Cx, p_ship: Ships) {
            let first = self.ships(p_ship).first().copied();
            let what = first.and_then(|id| self.u.ships.get(id).map(|s| format!("{:?} {id:?}", s.ship_type))).unwrap_or_else(|| "none".into());
            self.log(LogKind::Flow, format!("SetAIDepotShip: AI salvage depot = {what} (not modelled)"));
        }

        /// `kasfSetCurrentMimicItem` (0x493530): adds the ship type to a disguise menu (item
        /// type 0 the player's, 1 the computer's, 2 resets the asteroid list). The log line
        /// rebuilds the menus on load ([`replay_disguise_log`]).
        fn set_current_mimic_item(&mut self, _cx: &Cx, item_type: i32, ship_name: Str<'_>) {
            let name = Self::str_text(ship_name);
            let t = ShipType::from_name(&name).map_or(-1, |t| t.index() as i32);
            ai_ui::add_mimic_item(&mut self.g.disguise_tables, item_type, t);
            self.log(LogKind::Flow, format!("{MIMIC_ITEM_LOG}{item_type}, {t}) {name:?}"));
        }

        /// `kasfSetMCVItemIndex` (0x493590): how many entries of a disguise menu a single Mimic
        /// may take (see [`ai_ui::set_mcv_item_index`]).
        fn set_mcv_item_index(&mut self, _cx: &Cx, item_type: i32, index: i32) {
            ai_ui::set_mcv_item_index(&mut self.g.disguise_tables, item_type, index);
            self.log(LogKind::Flow, format!("{MCV_INDEX_LOG}{item_type}, {index})"));
        }

        // ---- objectives, pings, pointers -----------------------------------------------

        /// `kasfObjectiveSet` (0x490530, `objectiveSet`): the status of the first objective
        /// whose label matches (see [`ai_ui::objective_label_matches`]); nothing when none does.
        fn objective_set(&mut self, _cx: &Cx, label: Str<'_>, status: i32) {
            let label = Self::str_text(label);
            let Some(o) = self.g.objectives.iter_mut().find(|o| ai_ui::objective_label_matches(&o.label, &label)) else { return };
            o.complete = status != 0;
            let l = o.label.clone();
            self.log(LogKind::Objective, format!("ObjectiveSet {l:?} = {status}"));
        }

        /// `kasfPingAddSingleShip` (0x491440): an anomaly ping on the selection's first ship.
        fn ping_add_single_ship(&mut self, _cx: &Cx, ships: Ships, label: Str<'_>) {
            if let Some(&ship) = self.ships(ships).first() {
                let n = Self::str_text(label);
                self.g.pings.push(n.clone());
                self.g.ping_orders.push(crate::mission::PingOrder::Ship { label: n, ship });
            }
        }

        /// `kasfPingMultipleAddShips` (0x4941a0): one ping per ship, labelled `label` followed
        /// by the ship's index.
        fn ping_multiple_add_ships(&mut self, _cx: &Cx, ships: Ships, label: Str<'_>) {
            let v = self.ships(ships);
            if !v.is_empty() {
                let l = Self::str_text(label);
                let labels = ai_ui::multiple_ping_labels(&l, v.len());
                for (label, &ship) in labels.iter().zip(&v) {
                    self.g.ping_orders.push(crate::mission::PingOrder::Ships { label: label.clone(), ships: vec![ship] });
                }
                self.g.pings.extend(labels);
            }
        }

        /// `kasfTutSetPointerTargetShip` (0x4919a0): a tutorial pointer on the selection's
        /// first ship.
        fn tut_set_pointer_target_ship(&mut self, _cx: &Cx, name: Str<'_>, p_ship_list: Ships) {
            if !self.ships(p_ship_list).is_empty() {
                let n = Self::str_text(name);
                self.set_ship_pointer(n);
            }
        }

        /// `kasfTutSetObjPointerTargetShip` (0x4919e0): the same as an objective pointer.
        fn tut_set_obj_pointer_target_ship(&mut self, _cx: &Cx, name: Str<'_>, p_ship_list: Ships) {
            if !self.ships(p_ship_list).is_empty() {
                let n = Self::str_text(name);
                self.set_ship_pointer(n);
            }
        }

        // ---- camera ----------------------------------------------------------------------

        /// `kasfTutCameraFocus` (0x491e00).
        fn tut_camera_focus(&mut self, _cx: &Cx, p_ship_list: Ships) {
            let v = self.ships(p_ship_list);
            self.scripted_focus(false, v);
        }

        /// `kasfCameraFocusClose` (0x491f70): as `TutCameraFocus`, but `ccFocusCloseScripted`:
        /// a cut to the ships, the camera as close as it may go (the front end's distance).
        fn camera_focus_close(&mut self, _cx: &Cx, ships: Ships) {
            let v = self.ships(ships);
            self.scripted_focus(true, v);
        }

        /// `kasfCameraDistance` (0x491f50): `ccDistanceScripted` to the distance.
        fn camera_distance(&mut self, _cx: &Cx, distance: i32) {
            self.g.camera.distance = st(fild_i32(distance));
            let distance = self.g.camera.distance;
            self.g.camera_orders.push(crate::mission::CameraOrder::Distance { distance, scripted: true });
        }

        /// `kasfCameraSetChaseMode` (0x496380): sets or clears the camera's chase bit (8), for
        /// the front end.
        fn camera_set_chase_mode(&mut self, _cx: &Cx, chase_mode: i32) {
            self.log(LogKind::Flow, format!("CameraSetChaseMode({})", i32::from(chase_mode != 0)));
            self.g.camera_orders.push(crate::mission::CameraOrder::Chase(chase_mode != 0));
        }

        /// `kasfWideScreenIn` (0x492a40): the letterbox comes in (time compression back to
        /// normal, chase mode off, effects ducked: front-end matters). Its body is the same as
        /// `WideScreenInDontCloseSensors`.
        fn wide_screen_in(&mut self, _cx: &Cx, frames: i32) {
            self.g.wide_screen_call(self.u, crate::mission::WideScreenKind::In, frames);
        }

        // ---- sound -----------------------------------------------------------------------

        /// `kasfMusicStop` (0x492630): `soundEventStopMusic(fade)`.
        fn music_stop(&mut self, _cx: &Cx, fade_time: i32) {
            self.g.music = None;
            self.log(LogKind::Flow, format!("MusicStop (fade {fade_time})"));
        }

        /// `kasfSoundEvent` (0x492480): `soundEventPlay(NULL, event, 0)`; logged.
        fn sound_event(&mut self, _cx: &Cx, event: i32) {
            self.log(LogKind::Flow, format!("SoundEvent {event:#x}"));
        }

        // ---- intel and subtitles ---------------------------------------------------------

        /// `kasfSubtitleSimulate` (0x492cc0): a subtitle for the actor for `milliseconds`; the
        /// intel event is running until the subtitle has gone (`subTitlesDraw` ends it), which
        /// is when `IntelEventEnded` sees the end here.
        fn subtitle_simulate(&mut self, _cx: &Cx, actor: i32, milliseconds: i32, text: Str<'_>) {
            let t = Self::str_text(text);
            let dur = st(fild_i32(milliseconds) * 0.001);
            self.g.intel_ended = false;
            self.g.speech_end = Some(st(ld(self.u.total_time_elapsed) + ld(dur)));
            self.log(LogKind::Flow, format!("SubtitleSimulate actor {actor}, {milliseconds} ms: {t:?}"));
        }

        /// `kasfForceIntelEventEnded` (0x4931b0).
        fn force_intel_event_ended(&mut self, _cx: &Cx) {
            self.g.intel_ended = true;
        }

        // ---- screens ---------------------------------------------------------------------

        /// `kasfLockoutMainRegion` (0x493870): 1 goes to the main screen and locks out the
        /// mouse and menus (`spMainScreenAndLockout(7)`, which also clears the player's
        /// selection); anything else unlocks. The sensors manager closes; the lockout itself
        /// is logged.
        fn lockout_main_region(&mut self, _cx: &Cx, lock: i32) {
            if lock == 1 {
                self.main_screen();
                self.log(LogKind::Flow, "LockoutMainRegion: locked");
            } else {
                self.log(LogKind::Flow, "LockoutMainRegion: unlocked");
            }
        }

        /// `kasfDisableBigGunFiring` (0x496250): the Siege Cannon cannot fire in this level;
        /// logged.
        fn disable_big_gun(&mut self, _cx: &Cx) {
            self.g.big_gun_disabled = true;
            self.log(LogKind::Flow, crate::mission::BIG_GUN_DISABLED_LOG.to_string());
        }

        /// `kasfShowMinesInSensors` (0x493ab0): the sensors manager's show-mines flag (0x900604),
        /// which lets `smMinesDraw` show Mission 3's mines; logged.
        fn show_mines_in_sensors(&mut self, _cx: &Cx, show_mines: i32) {
            self.g.show_mines_in_sensors = show_mines != 0;
            self.log(LogKind::Flow, format!("{}{show_mines})", crate::mission::SHOW_MINES_LOG));
        }

        /// `kasfBuilderCloseIfOpen` (0x44fd50): closes the build manager when it is open; the
        /// simulation has none open.
        fn builder_close_if_open(&mut self, _cx: &Cx) {}

        /// `kasfSetTraderDialog` (0x4923d0, `tmSetDialog`): the trader screen's dialog text;
        /// logged.
        fn trader_gui_dialog_set(&mut self, _cx: &Cx, dialog_num: i32, dialog_text: Str<'_>) {
            let t = Self::str_text(dialog_text);
            self.log(LogKind::Flow, format!("TraderGUIDialogSet {dialog_num}: {t:?}"));
        }
    };
}
pub(super) use ai_ui_methods;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ships::mimic::DisguiseEntry;

    fn empty_tables() -> DisguiseTables {
        DisguiseTables { entries: Default::default(), defaults: [0; 9], flag: false }
    }

    #[test]
    fn disguise_slots() {
        assert_eq!(disguise_slot(0), 0);
        assert_eq!(disguise_slot(1), 1);
        assert_eq!(disguise_slot(2), 8);
        assert_eq!(disguise_slot(5), 5);
        assert_eq!(disguise_slot(0x103), 3);
    }

    #[test]
    fn mimic_items_fill_menus() {
        let mut t = empty_tables();
        add_mimic_item(&mut t, 0, 4);
        add_mimic_item(&mut t, 0, 7);
        add_mimic_item(&mut t, 1, 33);
        add_mimic_item(&mut t, 2, -1);
        set_mcv_item_index(&mut t, 0, 1);
        set_mcv_item_index(&mut t, 1, 0);
        assert_eq!(t.entries[0], vec![DisguiseEntry { ship_type: 4, asteroid: 0 }, DisguiseEntry { ship_type: 7, asteroid: 0 }]);
        assert_eq!(t.entries[1], vec![DisguiseEntry { ship_type: 33, asteroid: 0 }]);
        assert_eq!(t.entries[8].len(), 4);
        assert_eq!(t.defaults[0], 1);
        assert_eq!(t.defaults[1], 0);
        assert_eq!(t.defaults[8], 3);
    }

    #[test]
    fn objective_labels_compare_like_strncmp() {
        assert!(objective_label_matches("Hyperspace", "Hyperspace"));
        assert!(!objective_label_matches("Hyperspace", "hyperspace"));
        assert!(!objective_label_matches("Hyper", "Hyperspace"));
        let long = "A".repeat(40);
        // The stored label keeps 31 characters, so a longer query never matches it.
        assert!(!objective_label_matches(&long, &long));
        let l31 = "B".repeat(31);
        assert!(objective_label_matches(&l31, &l31));
    }

    #[test]
    fn multiple_pings_are_numbered() {
        assert_eq!(multiple_ping_labels("Beast", 3), vec!["Beast0", "Beast1", "Beast2"]);
        assert!(multiple_ping_labels("x", 0).is_empty());
    }
}
