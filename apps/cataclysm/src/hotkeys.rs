//! Hot-key ship groups (`selHotKeyGroup` and the `kbcbGroup*` key actions).
//!
//! Ten group slots exist; the keys reach groups 1 to 9 (`Create_ShipGroup_n` = Ctrl+n,
//! `Select_ShipGroup_n` = n, `AddTo_ShipGroup_n` = Shift+n, `Remove_Selected_Ship_Group` =
//! Ctrl+`). A ship belongs to one group at most: assigning, selecting or adding a group takes its
//! ships out of every other group (`selHotKeyNumbersSet`), and the group a ship is in is the
//! number drawn over it.
//!
//! Selecting a group that is already exactly the selection focuses the camera on it (in the
//! sensors manager: closes it onto the group). Cataclysm has no timed double press and no
//! Alt+number: `mrNumberDoublePressTime` is gone from the binary.
//!
//! Ships leave their groups when they die (the last one lost says the group is defeated), when
//! they are given away or removed from the game. Docked ships stay in their group; they are
//! skipped while inside. A linked ship takes the group of its parts, and the two parts of an
//! unlinked ship the group of the linked ship (`processLinkToDo` / `processUnlinkToDo`).
//! Groups carry over to the next mission with the ships that jump
//! (`univupdateCloseAllObjectsAndMissionSpheres` keeps them when a new level loads).

use std::collections::BTreeMap;

use hwc_math::Vector;
use hwc_sim::campaign::Carry;
use hwc_sim::mission::FleetShip;
use hwc_sim::object::sof;
use hwc_sim::statics::ShipType;
use hwc_sim::{ShipId, Universe};

/// Group slots (`SEL_NumberHotKeyGroups`); the keys use 1..=9.
pub const GROUP_COUNT: usize = 10;

/// The number printed for each group (0x8889c8; slot 0 has no key).
pub const LABELS: [&str; GROUP_COUNT] = [">", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// `UI_ClickAccept`: a group was assigned.
pub const SOUND_ASSIGN: u32 = 0x109;
/// `UI_Click`: a group was selected, added or removed.
pub const SOUND_SELECT: u32 = 0x108;

/// `COMM_AssGrp_Select`: the group answers when selected.
pub const SPEECH_SELECT: u32 = 0x400_105c;
/// `COMM_F_Group_Assigning`.
pub const SPEECH_ASSIGNING: u32 = 0x400_1063;
/// `COMM_F_AssGrp_AddingShips`: the new group mostly holds the old one's ships, and more.
pub const SPEECH_REINFORCED: u32 = 0x400_1064;
/// `STAT_F_AssGrp_Defeat`: the last ship of a group died.
pub const SPEECH_DEFEAT: u32 = 0x200_2076;

/// How near the parts of a link or unlink are to the ship they made (world units).
const LINK_RANGE: f32 = 3000.0;

/// What a group key asks of the game: a sound, a speech event (with the group number) and
/// whether to focus on the new selection.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Response {
    pub sound: Option<u32>,
    pub speech: Option<(u32, u8)>,
    pub focus: bool,
}

/// The ten hot-key groups.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HotKeyGroups {
    groups: [Vec<ShipId>; GROUP_COUNT],
}

/// Whether a group member is taken when the group is selected (`selSelectHotKeyGroup`):
/// selectable, and not hidden (docked), crazy, disabled or in hyperspace.
pub fn takes_part(u: &Universe, id: ShipId) -> bool {
    u.ship(id).is_some_and(|s| {
        s.flags & (sof::HIDE | sof::CRAZY | sof::DISABLED | sof::HYPERSPACE) == 0
            && s.flags & sof::SELECTABLE != 0
    })
}

/// Whether a member is added by Shift+n (`kbcbGroupAddSelect`): in the world (it has a
/// collision blob) and selectable.
pub fn can_add(u: &Universe, id: ShipId) -> bool {
    u.ship(id)
        .is_some_and(|s| s.coll_my_blob.is_some() && s.flags & sof::SELECTABLE != 0)
}

/// Same ships, whatever the order (`selSelectionCompare`).
fn same_ships(a: &[ShipId], b: &[ShipId]) -> bool {
    let mut a = a.to_vec();
    let mut b = b.to_vec();
    a.sort();
    b.sort();
    a == b
}

/// `selSelectionIsReinforced`: the new selection is larger and keeps at least 215/256 of the
/// old group (all of a one-ship group).
pub fn is_reinforced(old: &[ShipId], new: &[ShipId]) -> bool {
    if new.len() <= old.len() || old.is_empty() {
        return false;
    }
    let needed = if old.len() == 1 {
        1
    } else {
        old.len() * 215 / 256
    };
    old.iter().filter(|id| new.contains(id)).count() >= needed
}

impl HotKeyGroups {
    pub fn new() -> Self {
        Self::default()
    }

    /// The ships of group `n`.
    #[cfg(test)]
    pub fn group(&self, n: u8) -> &[ShipId] {
        self.groups.get(usize::from(n)).map_or(&[], Vec::as_slice)
    }

    /// The groups that have ships, in number order.
    pub fn non_empty(&self) -> impl Iterator<Item = (u8, &[ShipId])> {
        self.groups
            .iter()
            .enumerate()
            .filter(|(_, g)| !g.is_empty())
            .map(|(n, g)| (n as u8, g.as_slice()))
    }

    /// The group a ship is in (the number drawn over it).
    pub fn group_of(&self, id: ShipId) -> Option<u8> {
        self.groups
            .iter()
            .position(|g| g.contains(&id))
            .map(|n| n as u8)
    }

    /// `selHotKeyNumbersSet`: group `n`'s ships leave the other numbered groups.
    fn claim(&mut self, n: usize) {
        let ships = self.groups[n].clone();
        for (m, g) in self.groups.iter_mut().enumerate().skip(1) {
            if m != n {
                g.retain(|id| !ships.contains(id));
            }
        }
    }

    /// `Create_ShipGroup_n` (`kbcbGroupAssignN`): the selection becomes group `n`.
    pub fn assign(&mut self, n: u8, selection: &[ShipId]) -> Response {
        let n = usize::from(n);
        if selection.is_empty() || n >= GROUP_COUNT {
            return Response::default();
        }
        let reinforced = is_reinforced(&self.groups[n], selection);
        self.groups[n] = selection.to_vec();
        self.claim(n);
        let speech = if reinforced {
            SPEECH_REINFORCED
        } else {
            SPEECH_ASSIGNING
        };
        Response {
            sound: Some(SOUND_ASSIGN),
            speech: Some((speech, n as u8)),
            focus: false,
        }
    }

    /// `Select_ShipGroup_n` (`kbcbGroupSelect`): the group's ships that can be selected become
    /// the selection; selecting the group again focuses on it.
    pub fn select(
        &mut self,
        n: u8,
        selection: &mut Vec<ShipId>,
        takes_part: impl Fn(ShipId) -> bool,
    ) -> Response {
        let n = usize::from(n);
        if n >= GROUP_COUNT || self.groups[n].is_empty() {
            return Response::default();
        }
        let before = std::mem::take(selection);
        selection.extend(self.groups[n].iter().copied().filter(|&id| takes_part(id)));
        self.claim(n);
        if selection.is_empty() {
            return Response::default();
        }
        Response {
            sound: Some(SOUND_SELECT),
            speech: Some((SPEECH_SELECT, n as u8)),
            focus: !before.is_empty() && same_ships(&before, selection),
        }
    }

    /// `AddTo_ShipGroup_n` (`kbcbGroupAddSelect`): the group's ships join the selection.
    pub fn add_to_selection(
        &mut self,
        n: u8,
        selection: &mut Vec<ShipId>,
        can_add: impl Fn(ShipId) -> bool,
    ) -> Response {
        let n = usize::from(n);
        if n >= GROUP_COUNT {
            return Response::default();
        }
        for &id in self.groups[n].iter().rev() {
            if can_add(id) && !selection.contains(&id) {
                selection.push(id);
            }
        }
        self.claim(n);
        if self.groups[n].is_empty() {
            return Response::default();
        }
        Response {
            sound: Some(SOUND_SELECT),
            speech: Some((SPEECH_SELECT, n as u8)),
            focus: false,
        }
    }

    /// `Remove_Selected_Ship_Group` (`kbcbRemoveSelectedShipGroup`): when the selection is
    /// exactly the group of its first ship, that group is emptied; otherwise, if any selected
    /// ship is in a group, the selected ships leave their groups.
    pub fn remove_selected(&mut self, selection: &[ShipId]) -> Response {
        let Some((i, n)) = selection
            .iter()
            .enumerate()
            .find_map(|(i, &id)| self.group_of(id).map(|n| (i, usize::from(n))))
        else {
            return Response::default();
        };
        if i == 0 && same_ships(selection, &self.groups[n]) {
            self.groups[n].clear();
            return Response {
                sound: Some(SOUND_SELECT),
                ..Response::default()
            };
        }
        for g in &mut self.groups {
            g.retain(|id| !selection.contains(id));
        }
        Response::default()
    }

    /// After a universe update: ships that died, were removed or now belong to another player
    /// leave their groups. Returns the groups that the deaths emptied (each says
    /// `STAT_F_AssGrp_Defeat`). `died` are this update's deaths.
    pub fn prune(&mut self, u: &Universe, player: usize, died: &[ShipId]) -> Vec<u8> {
        let mut defeated = Vec::new();
        for (n, g) in self.groups.iter_mut().enumerate() {
            if g.is_empty() {
                continue;
            }
            let mut deaths = 0;
            g.retain(|&id| match u.ship(id) {
                Some(s) if s.flags & sof::DEAD != 0 || died.contains(&id) => {
                    deaths += 1;
                    false
                }
                Some(s) => s.player == player,
                None => {
                    if died.contains(&id) {
                        deaths += 1;
                    }
                    false
                }
            });
            if g.is_empty() && deaths > 0 {
                defeated.push(n as u8);
            }
        }
        defeated
    }

    /// Where the groups' ships are before an update (for [`relink`](Self::relink)).
    pub fn snapshot(&self, u: &Universe) -> Snapshot {
        let mut members = BTreeMap::new();
        for (n, g) in self.groups.iter().enumerate() {
            for &id in g {
                if let Some(s) = u.ship(id) {
                    members.insert(id, (n as u8, s.ship_type, s.posinfo.position));
                }
            }
        }
        Snapshot {
            members,
            last_id: u.ship_list.iter().copied().max(),
        }
    }

    /// After an update: a ship linked from group members joins the group of the first part
    /// (the one that flew to the other) or else the second's; the two ships an unlinked group
    /// member splits into join its group.
    pub fn relink(&mut self, before: &Snapshot, u: &Universe) {
        let gone: Vec<(ShipId, u8, ShipType, Vector)> = before
            .members
            .iter()
            .filter(|(id, _)| u.ship(**id).is_none())
            .map(|(&id, &(n, t, p))| (id, n, t, p))
            .collect();
        if gone.is_empty() {
            return;
        }
        let new: Vec<ShipId> = u
            .ship_list
            .iter()
            .copied()
            .filter(|&id| before.last_id.is_none_or(|l| id > l))
            .collect();
        for id in new {
            let Some(s) = u.ship(id) else { continue };
            let (t, p) = (s.ship_type, s.posinfo.position);
            let near = |q: Vector| (q - p).mag_sq() < LINK_RANGE * LINK_RANGE;
            // Linked: the parts are of the type that links into this one; the second part
            // stood where the new ship is.
            let mut parts: Vec<&(ShipId, u8, ShipType, Vector)> = gone
                .iter()
                .filter(|g| hwc_sim::link::linked_type(g.2) == Some(t) && near(g.3))
                .collect();
            parts.sort_by(|a, b| (b.3 - p).mag_sq().total_cmp(&(a.3 - p).mag_sq()));
            // Unlinked: the linked ship it came from.
            let whole = gone
                .iter()
                .find(|g| hwc_sim::link::linked_type(t) == Some(g.2) && near(g.3));
            let group = parts.first().map(|g| g.1).or(whole.map(|g| g.1));
            if let Some(n) = group {
                let g = &mut self.groups[usize::from(n)];
                if !g.contains(&id) {
                    g.push(id);
                }
            }
        }
    }

    /// The groups as they leave with the fleet (`carry` is the mission's [`Carry::take`]):
    /// each member that jumped is kept by its place in the next mission's fleet. `fleet` are the
    /// ships in hyperspace (`HyperState::in_hyperspace`).
    pub fn carry_out(
        &self,
        u: &Universe,
        fleet: &[ShipId],
        player: usize,
        carry: &Carry,
        next_mission: u32,
    ) -> CarriedGroups {
        // Old ships to their place in the carry.
        let mut places: BTreeMap<ShipId, Vec<usize>> = BTreeMap::new();
        let top: Vec<ShipId> = fleet
            .iter()
            .copied()
            .filter(|&id| u.ship(id).is_some_and(|s| s.player == player))
            .collect();
        map_level(u, &top, &carry.fleet, &mut Vec::new(), &mut places);
        // The modules the next mission does not have are wiped: places in the wiped list.
        let wiped = hwc_sim::campaign::wipe_unavailable_modules(&carry.fleet, next_mission);
        let mut kept = vec![None; carry.fleet.len()];
        let mut j = 0;
        for (i, f) in carry.fleet.iter().enumerate() {
            if let Some(w) = wiped.get(j) {
                if w.ship_type == f.ship_type && w.module == f.module {
                    kept[i] = Some(j);
                    j += 1;
                }
            }
        }
        let mut groups = Vec::new();
        for (n, g) in self.groups.iter().enumerate() {
            let members: Vec<FleetPlace> = g
                .iter()
                .filter_map(|id| {
                    let path = places.get(id)?;
                    let top = kept[path[0]]?;
                    let mut p = path.clone();
                    p[0] = top;
                    Some(FleetPlace {
                        path: p,
                        ship_type: u.ship(*id)?.ship_type,
                    })
                })
                .collect();
            if !members.is_empty() {
                groups.push((n as u8, members));
            }
        }
        CarriedGroups { groups }
    }
}

/// Group members before an update.
#[derive(Debug, Default, Clone)]
pub struct Snapshot {
    members: BTreeMap<ShipId, (u8, ShipType, Vector)>,
    last_id: Option<ShipId>,
}

/// Pairs ships with fleet entries of the same type in order (an entry the carry left out is
/// skipped), then their docked ships with the entries' docked lists.
fn map_level(
    u: &Universe,
    ships: &[ShipId],
    entries: &[FleetShip],
    prefix: &mut Vec<usize>,
    out: &mut BTreeMap<ShipId, Vec<usize>>,
) {
    let mut k = 0;
    for &id in ships {
        let Some(s) = u.ship(id) else { continue };
        let Some(off) = entries[k..].iter().position(|f| f.ship_type == s.ship_type) else {
            continue;
        };
        let i = k + off;
        k = i + 1;
        prefix.push(i);
        out.insert(id, prefix.clone());
        if let Some(inside) = &s.base.inside {
            map_level(u, &inside.ships, &entries[i].docked, prefix, out);
        }
        prefix.pop();
    }
}

/// A group member's place in the fleet that arrives: index in the fleet list, then in each
/// docked list down to the ship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetPlace {
    pub path: Vec<usize>,
    pub ship_type: ShipType,
}

/// The hot-key groups on their way to the next mission.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CarriedGroups {
    pub groups: Vec<(u8, Vec<FleetPlace>)>,
}

impl CarriedGroups {
    /// The groups over the arriving fleet: `fleet` are the ships made for the fleet list, in
    /// its order (`HyperState::fleet_ids`); docked ships are found inside their hosts.
    pub fn arrive(&self, u: &Universe, fleet: &[Option<ShipId>]) -> HotKeyGroups {
        let mut out = HotKeyGroups::new();
        for (n, members) in &self.groups {
            let Some(g) = out.groups.get_mut(usize::from(*n)) else {
                continue;
            };
            for m in members {
                let Some(&first) = m.path.first() else {
                    continue;
                };
                let mut id = fleet.get(first).copied().flatten();
                for &k in &m.path[1..] {
                    id = id.and_then(|h| u.ship(h)?.base.inside.as_ref()?.ships.get(k).copied());
                }
                if let Some(id) =
                    id.filter(|&id| u.ship(id).is_some_and(|s| s.ship_type == m.ship_type))
                {
                    if !g.contains(&id) {
                        g.push(id);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[u16]) -> Vec<ShipId> {
        v.iter().map(|&i| ShipId(i)).collect()
    }

    #[test]
    fn assign_takes_ships_from_other_groups() {
        let mut g = HotKeyGroups::new();
        let r = g.assign(1, &ids(&[1, 2, 3]));
        assert_eq!(r.sound, Some(SOUND_ASSIGN));
        assert_eq!(r.speech, Some((SPEECH_ASSIGNING, 1)));
        g.assign(2, &ids(&[3, 4]));
        assert_eq!(g.group(1), ids(&[1, 2]).as_slice());
        assert_eq!(g.group(2), ids(&[3, 4]).as_slice());
        assert_eq!(g.group_of(ShipId(3)), Some(2));
        assert_eq!(g.group_of(ShipId(9)), None);
        // Nothing selected: nothing happens.
        assert_eq!(g.assign(1, &[]), Response::default());
        assert_eq!(g.group(1).len(), 2);
    }

    #[test]
    fn reinforcing_a_group() {
        assert!(!is_reinforced(&[], &ids(&[1])));
        assert!(is_reinforced(&ids(&[1]), &ids(&[1, 2])));
        assert!(!is_reinforced(&ids(&[1]), &ids(&[2, 3])));
        assert!(!is_reinforced(&ids(&[1, 2]), &ids(&[1, 2])));
        // 10 ships: 8 of them (10 * 215 / 256) must stay.
        let old: Vec<u16> = (1..=10).collect();
        let mut new: Vec<u16> = (3..=12).collect();
        new.push(13);
        assert!(is_reinforced(&ids(&old), &ids(&new)));
        let new: Vec<u16> = (4..=14).collect();
        assert!(!is_reinforced(&ids(&old), &ids(&new)));
        let mut g = HotKeyGroups::new();
        g.assign(4, &ids(&[1]));
        assert_eq!(
            g.assign(4, &ids(&[1, 2])).speech,
            Some((SPEECH_REINFORCED, 4))
        );
    }

    #[test]
    fn selecting_twice_focuses() {
        let mut g = HotKeyGroups::new();
        g.assign(1, &ids(&[1, 2, 3]));
        let mut sel = ids(&[7]);
        let r = g.select(1, &mut sel, |id| id != ShipId(3));
        assert_eq!(sel, ids(&[1, 2]));
        assert_eq!(r.sound, Some(SOUND_SELECT));
        assert_eq!(r.speech, Some((SPEECH_SELECT, 1)));
        assert!(!r.focus);
        sel.reverse();
        let r = g.select(1, &mut sel, |id| id != ShipId(3));
        assert!(r.focus, "the group was already the selection");
        // Nothing selected before: no focus.
        let mut empty = Vec::new();
        assert!(!g.select(1, &mut empty, |_| true).focus);
        // An empty group leaves the selection alone.
        let mut sel = ids(&[5]);
        assert_eq!(g.select(6, &mut sel, |_| true), Response::default());
        assert_eq!(sel, ids(&[5]));
        // A group with no selectable ship clears the selection, silently.
        assert_eq!(g.select(1, &mut sel, |_| false), Response::default());
        assert!(sel.is_empty());
    }

    #[test]
    fn shift_adds_the_group_to_the_selection() {
        let mut g = HotKeyGroups::new();
        g.assign(3, &ids(&[1, 2, 3]));
        let mut sel = ids(&[9, 2]);
        let r = g.add_to_selection(3, &mut sel, |id| id != ShipId(1));
        assert_eq!(sel, ids(&[9, 2, 3]));
        assert_eq!(r.speech, Some((SPEECH_SELECT, 3)));
        assert_eq!(g.group(3).len(), 3, "the group itself is unchanged");
        assert_eq!(
            g.add_to_selection(5, &mut sel, |_| true),
            Response::default()
        );
    }

    #[test]
    fn removing_the_selected_group() {
        let mut g = HotKeyGroups::new();
        g.assign(1, &ids(&[1, 2]));
        g.assign(2, &ids(&[3, 4]));
        // The selection is group 1: group 1 goes.
        assert_eq!(g.remove_selected(&ids(&[2, 1])).sound, Some(SOUND_SELECT));
        assert!(g.group(1).is_empty());
        // Part of group 2 and a loose ship: the selected ships leave their groups.
        assert_eq!(g.remove_selected(&ids(&[5, 3])), Response::default());
        assert_eq!(g.group(2), ids(&[4]).as_slice());
        // No grouped ship selected: nothing.
        assert_eq!(g.remove_selected(&ids(&[5])), Response::default());
        assert_eq!(g.group(2).len(), 1);
    }

    #[test]
    fn groups_listed_in_order() {
        let mut g = HotKeyGroups::new();
        g.assign(5, &ids(&[1]));
        g.assign(2, &ids(&[2]));
        let n: Vec<u8> = g.non_empty().map(|(n, _)| n).collect();
        assert_eq!(n, vec![2, 5]);
        assert_eq!(LABELS[5], "5");
    }

    /// Game data (`HWC_GAME_DIR`): groups made over the fleet in hyperspace at the start of
    /// Mission 2 come back on the same ships of a second load of that fleet, docked ships
    /// included.
    #[test]
    fn groups_ride_the_carry() {
        let Some(dir) = std::env::var_os("HWC_GAME_DIR") else {
            eprintln!("skipped: HWC_GAME_DIR not set");
            return;
        };
        let Ok(data) = hwc_formats::GameData::open(std::path::Path::new(&dir)) else {
            eprintln!("skipped: no game data in HWC_GAME_DIR");
            return;
        };
        let assets = crate::data::Assets(std::rc::Rc::new(data));
        let mut loader = crate::mission::Loader::new(2, 2);
        loader.load_statics(&assets);
        let statics = loader.statics.clone().expect("statics");
        let tweaks = loader.tweaks.clone().expect("tweaks");
        let mut carry = Carry::warp(statics, tweaks, &assets, 2).expect("warp fleet");
        // Two acolytes docked in the mothership, for the docked places.
        let ms = carry
            .fleet
            .iter_mut()
            .find(|f| f.ship_type == ShipType::sMothership)
            .expect("mothership");
        ms.docked.push(FleetShip::new(ShipType::sAcolyte));
        ms.docked.push(FleetShip::new(ShipType::sAcolyte));
        loader.carry = Some(carry.clone());
        let run = loader.start(&assets).expect("mission 2");
        let u = &run.universe;
        let player = crate::host::PLAYER;
        let top: Vec<ShipId> = run.game.hyper.fleet_ids.iter().flatten().copied().collect();
        assert!(!top.is_empty(), "no fleet in hyperspace");
        let docked: Vec<ShipId> = top
            .iter()
            .filter_map(|&id| u.ship(id)?.base.inside.as_ref())
            .flat_map(|i| i.ships.iter().copied())
            .collect();
        assert_eq!(docked.len(), 2, "the acolytes are not docked");
        let mut g = HotKeyGroups::new();
        g.assign(1, &top[..top.len().min(4)]);
        g.assign(2, &docked[1..]);
        let out = g.carry_out(u, &run.game.hyper.in_hyperspace, player, &carry, 2);
        let types = |u: &Universe, ids: &[ShipId]| {
            let mut t: Vec<String> = ids
                .iter()
                .map(|&id| u.ship(id).unwrap().ship_type.to_string())
                .collect();
            t.sort();
            t
        };
        let want1 = types(u, g.group(1));
        let want2 = types(u, g.group(2));
        let mut again = crate::mission::Loader::new(2, 2);
        again.statics = loader.statics.clone();
        again.tweaks = loader.tweaks.clone();
        again.carry = Some(carry);
        let mut run2 = again.start(&assets).expect("mission 2 again");
        let back = out.arrive(&run2.universe, &run2.game.hyper.fleet_ids);
        assert_eq!(types(&run2.universe, back.group(1)), want1);
        assert_eq!(types(&run2.universe, back.group(2)), want2);
        assert_eq!(back.group(1).len(), g.group(1).len());
        assert_eq!(back.group(2).len(), 1);
        // The second docked acolyte, not the first.
        let ms2 = run2
            .game
            .hyper
            .fleet_ids
            .iter()
            .flatten()
            .find(|&&id| run2.universe.ship(id).unwrap().ship_type == ShipType::sMothership)
            .copied()
            .expect("mothership");
        let inside = &run2
            .universe
            .ship(ms2)
            .unwrap()
            .base
            .inside
            .as_ref()
            .unwrap()
            .ships;
        assert_eq!(back.group(2), &inside[1..]);
        // The ships are the second run's own.
        for id in back.group(1) {
            assert!(run2.game.hyper.fleet_ids.contains(&Some(*id)));
        }
        // Clean-up: the docked acolyte stays (hidden, not taken by a select); a ship given away
        // leaves silently; the last ship dying says the group is defeated.
        let mut back = back;
        let acolyte = back.group(2)[0];
        assert!(back.prune(&run2.universe, player, &[]).is_empty());
        assert_eq!(back.group(2), &[acolyte]);
        assert!(!takes_part(&run2.universe, acolyte));
        let g1 = back.group(1).to_vec();
        run2.universe.ships.ship_mut(g1[0]).player = 1;
        assert!(back.prune(&run2.universe, player, &[]).is_empty());
        assert_eq!(back.group(1), &g1[1..]);
        for &id in &g1[1..] {
            run2.universe.ships.ship_mut(id).flags |= sof::DEAD;
        }
        assert_eq!(back.prune(&run2.universe, player, &[]), vec![1]);
        assert!(back.group(1).is_empty());
        eprintln!(
            "carried: group 1 {} ships, group 2 {} docked ships",
            want1.len(),
            want2.len()
        );
    }
}
