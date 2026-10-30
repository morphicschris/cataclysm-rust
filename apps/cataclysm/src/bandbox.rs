//! The band box's modifier modes and object filters (`mrSelectHold` / `mrBandBoxRelease`,
//! `selRectDragFunction` and the `MakeTargets*` filters of Cataclysm).
//!
//! While the left button drags a box, the objects inside are gathered into the "selecting"
//! list with the filters of the modifier held, and outlined; releasing the button issues the
//! modifier's order to the gathered list. The same code runs in the main view and in the
//! sensors manager (where the test is whether the object's point is inside the box).

use hwc_sim::object::{flags2, sof};
use hwc_sim::statics::ShipRace;
use hwc_sim::{command, ShipId, Universe};
use hwc_ui::ingame::keys::{self, Action};

/// `dBigGunUnit`: never band-boxed while permanently disabled is off, never attacked.
const BIG_GUN_UNIT: usize = 0x6f;
/// `trJunkYardHQ`: not an attack target unless forced.
const JUNKYARD_HQ: usize = 0x67;

/// What the band box does, from the modifier held (in the order the original tests them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `Bandbox_ForceAttack` (default Ctrl+Shift): attack anything targetable.
    ForceAttack,
    /// `Multiple_Select` (Shift): add own ships to the selection.
    MultipleSelect,
    /// `Bandbox_Guard` (G): guard friendly ships.
    Guard,
    /// `Ship_Special` (Z): the special-action menu for the ships in the box.
    Special,
    /// `Bandbox_Focus` (Alt): focus on everything in the box.
    Focus,
    /// `Bandbox_Attack` (Ctrl): attack the enemies in the box.
    Attack,
    /// No modifier: select own ships.
    Select,
}

impl Mode {
    /// `mrSelectHold` / `mrBandBoxRelease` test the actions in this order; bindings match on
    /// exact modifiers, so with the default keys Ctrl+Alt matches none and selects.
    pub fn from_held(held: impl Fn(Action) -> bool) -> Mode {
        if held(keys::BANDBOX_FORCE_ATTACK) {
            Mode::ForceAttack
        } else if held(keys::MULTIPLE_SELECT) {
            Mode::MultipleSelect
        } else if held(keys::BANDBOX_GUARD) {
            Mode::Guard
        } else if held(keys::SHIP_SPECIAL) {
            Mode::Special
        } else if held(keys::BANDBOX_FOCUS) {
            Mode::Focus
        } else if held(keys::BANDBOX_ATTACK) {
            Mode::Attack
        } else {
            Mode::Select
        }
    }

    /// The `selRectDragFunction` options the mode gathers with.
    pub fn gather(self) -> Gather {
        match self {
            Mode::ForceAttack | Mode::Attack => Gather {
                own_only: false,
                anything: true,
                attack: true,
            },
            Mode::Guard | Mode::Focus => Gather {
                own_only: false,
                anything: false,
                attack: false,
            },
            Mode::MultipleSelect | Mode::Select | Mode::Special => Gather {
                own_only: true,
                anything: false,
                attack: false,
            },
        }
    }
}

/// `selRectDragFunction`'s options: only the player's own ships; anything targetable (not
/// just ships); an attack gathering (some objects only answer to those).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gather {
    pub own_only: bool,
    pub anything: bool,
    pub attack: bool,
}

/// What the filters need to know about one ship, as the local player sees it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Facts {
    pub own: bool,
    /// `allianceIsShipAlly`: own ships and allies (crystals never).
    pub ally: bool,
    /// `SOF_Selectable`.
    pub selectable: bool,
    /// `SOF_Selectable | SOF_Targetable`.
    pub targetable: bool,
    pub hidden: bool,
    pub dead: bool,
    /// Has a selection circle (`selCircleRadius > 0`).
    pub on_screen: bool,
    /// Static capability bit 0x4000 (untargetable by type).
    pub untargetable_type: bool,
    /// `flags2` 0x10: disabled for good.
    pub permanently_disabled: bool,
    pub crystal: bool,
    /// Race `RACE_DERELICT`.
    pub derelict: bool,
    pub ship_type: usize,
    /// `cannotForceAttackIfOwnShip`.
    pub no_force_attack_if_own: bool,
    /// Cloaked, not the player's, and not seen by the player.
    pub cloaked_unseen: bool,
    /// An enemy Mimic disguised as one of the player's ships.
    pub mimic_as_own: bool,
    /// An enemy Mimic passing for a resource or a derelict.
    pub mimic_as_object: bool,
    /// `MakeTargetsNotIncludeUntargetable`'s other cases (Acolytes, the infected Ecclesiam,
    /// special flag 0x200000).
    pub untargetable: bool,
    /// A mine (`OBJ_MissileType` with `MISSILE_Mine`), not a ship.
    pub mine: bool,
}

impl Facts {
    /// The facts of a ship for `player`.
    pub fn of(u: &Universe, id: ShipId, player: usize) -> Option<Facts> {
        use hwc_sim::ships::{crystal, mimic};
        use hwc_sim::statics::ShipType;
        let s = u.ship(id)?;
        let st = u.statics.ship(s.ship_type);
        let own = s.player == player;
        let crystal = crystal::is_crystal(u, id);
        let ally = !crystal
            && (own
                || s.attributes & 0x100 != 0
                || u.players.get(player).is_some_and(|p| p.is_ally(s.player)));
        let disguise = !own && s.flags2 & flags2::DISGUISED != 0;
        let caps = st.map_or(0, |st| st.capability_flags);
        Some(Facts {
            own,
            ally,
            selectable: s.flags & sof::SELECTABLE != 0 && !crate::host::handed_over(s),
            targetable: s.flags & (sof::SELECTABLE | sof::TARGETABLE) != 0,
            hidden: s.flags & sof::HIDE != 0,
            dead: s.flags & sof::DEAD != 0,
            on_screen: true,
            untargetable_type: caps & 0x4000 != 0,
            permanently_disabled: s.flags2 & flags2::PERMANENTLY_DISABLED != 0,
            crystal,
            derelict: st.is_some_and(|st| st.ship_race == ShipRace::Derelict),
            ship_type: s.ship_type.index(),
            no_force_attack_if_own: st.is_some_and(|st| st.cannot_force_attack_if_own_ship),
            cloaked_unseen: !own
                && hwc_sim::cloak::is_cloaked(s)
                && !hwc_sim::visibility::visible_to_player(u, id, player),
            mimic_as_own: disguise && mimic::is_disguised_as_player(u, id, player),
            mimic_as_object: disguise
                && (mimic::fools_player_as_resource(u, id, player)
                    || mimic::fools_player_as_derelict(u, id, player)),
            untargetable: caps & 0x4000 != 0
                || s.ship_type == ShipType::sAcolyte
                || s.special_flags & 0x20_0000 != 0
                || s.ship_type == ShipType::dEcclesiamInfected,
            mine: false,
        })
    }
}

impl Facts {
    /// The facts of a mine (`id` is its selection id) for the local player. `sensors`: the box
    /// is dragged in the sensors manager, where a mine the player's ships do not see now is
    /// left out (`selFowPointHidden`), as are Mission 3's mines until the script shows them
    /// (`ShowMinesInSensors`).
    pub fn of_mine(
        u: &Universe,
        id: ShipId,
        sensors: bool,
        mines_shown: bool,
        fog_hidden: bool,
    ) -> Option<Facts> {
        let m = u.missiles.get(hwc_sim::target::live_mine(u, id)?)?;
        let mission3 = u.single_player && u.base.options.mission == 3;
        Some(Facts {
            selectable: m.flags & sof::SELECTABLE != 0,
            targetable: m.flags & (sof::SELECTABLE | sof::TARGETABLE) != 0,
            hidden: m.flags & sof::HIDE != 0 || (sensors && mission3 && !mines_shown),
            dead: m.flags & sof::DEAD != 0,
            on_screen: true,
            ship_type: usize::MAX,
            cloaked_unseen: sensors && fog_hidden,
            mine: true,
            ..Facts::default()
        })
    }
}

/// The ship a pick stands for: a component clamped to a ship (a mothership's module) picks
/// its host (`selRectDragFunction`, `selSelectionClick`, `selShipUnderMouse`).
pub fn stand_in(u: &Universe, id: ShipId) -> ShipId {
    let component = u
        .ship(id)
        .and_then(|s| u.statics.ship(s.ship_type))
        .is_some_and(|st| st.ship_class == Some(hwc_sim::statics::ShipClass::Component));
    if !component {
        return id;
    }
    hwc_sim::clamp::host_of(u, id)
        .filter(|&h| u.ship(h).is_some())
        .unwrap_or(id)
}

/// `selRectDragFunction`'s gathering: the ships inside the box that pass [`gathered`], each
/// module standing for its host ([`stand_in`]) and every ship once. `facts` gives a host's
/// facts.
pub fn gather_inside(
    inside: &[(ShipId, Facts)],
    g: Gather,
    stand_in: impl Fn(ShipId) -> ShipId,
    facts: impl Fn(ShipId) -> Option<Facts>,
) -> Vec<(ShipId, Facts)> {
    let mut out: Vec<(ShipId, Facts)> = Vec::new();
    for &(id, f) in inside {
        if !gathered(&f, g) {
            continue;
        }
        let host = stand_in(id);
        let entry = if host == id {
            (id, f)
        } else {
            match facts(host) {
                Some(hf) => (host, hf),
                None => continue,
            }
        };
        if !out.iter().any(|(x, _)| *x == entry.0) {
            out.push(entry);
        }
    }
    out
}

/// `selRectDragFunction`'s test of one ship inside the box.
pub fn gathered(f: &Facts, g: Gather) -> bool {
    if (!f.untargetable_type && (!f.targetable || f.hidden)) || !f.on_screen || f.dead {
        return false;
    }
    if f.permanently_disabled && f.ship_type != BIG_GUN_UNIT {
        return false;
    }
    if !g.anything && g.own_only {
        // Own selectable ships, and enemy Mimics passing for them.
        if !f.selectable || (!f.own && (!f.mimic_as_own || f.mimic_as_object)) {
            return false;
        }
    }
    if f.cloaked_unseen {
        return false;
    }
    if f.mimic_as_object && !g.anything {
        return false;
    }
    if g.attack
        && ((f.no_force_attack_if_own && f.own)
            || f.ship_type == BIG_GUN_UNIT
            || f.untargetable_type)
    {
        return false;
    }
    true
}

/// `MakeTargetsOnlyNonForceAttackTargets`: what a plain attack may target: enemy ships that
/// are not crystals, disabled for good, or Mimics in disguise, derelicts that are not the
/// player's, and mines.
pub fn plain_attack_target(f: &Facts) -> bool {
    if f.mine {
        return true;
    }
    if f.derelict && !f.own && !f.crystal {
        return true;
    }
    if f.ally || f.ship_type == JUNKYARD_HQ {
        return false;
    }
    !f.crystal && !f.permanently_disabled && !(f.mimic_as_own || f.mimic_as_object)
}

/// The selected ships an order goes to.
pub struct Selected<'a> {
    pub ships: &'a [ShipId],
    /// `MakeShipsAttackCapable` found some.
    pub attack_capable: bool,
    /// `makeShipsHoldCapable` found some.
    pub hold_capable: bool,
    /// What a `Ship_Special` box gathers for these ships ([`special_gather`]).
    pub special: Option<SpecialGather>,
}

/// What a `Ship_Special` band box gathers (`mrSelectHold`), from the selected ships with a
/// special-target routine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecialGather {
    pub gather: Gather,
    /// Some ship may target friendlies (`canSpecialBandBoxFriendlies`): the player's and
    /// allied ships stay in the box.
    pub friendlies: bool,
    /// A salvager is selected: derelicts stay in the box.
    pub salvager: bool,
}

/// `mrSelectHold`'s `Ship_Special` gathering: `None` when no selected ship has a special
/// target routine.
pub fn special_gather(u: &Universe, ships: &[ShipId]) -> Option<SpecialGather> {
    let (mut friendlies, mut plain, mut anything) = (false, false, false);
    for &id in ships {
        let Some(s) = u.ship(id) else { continue };
        let Some(st) = u.statics.ship(s.ship_type) else {
            continue;
        };
        if !hwc_sim::ships::has_special_target(s.ship_type) {
            continue;
        }
        if !st.can_special_band_box_friendlies {
            plain = true;
        } else {
            friendlies = true;
            if st.unknown_flag_212 {
                plain = true;
            }
        }
        if st.unknown_flag_213 {
            anything = true;
        }
    }
    let gather = if anything {
        Gather {
            own_only: false,
            anything: true,
            attack: false,
        }
    } else if plain {
        Gather {
            own_only: false,
            anything: false,
            attack: false,
        }
    } else if friendlies {
        Gather {
            own_only: true,
            anything: false,
            attack: false,
        }
    } else {
        return None;
    };
    let salvager = ships.iter().any(|&id| {
        u.ship(id)
            .is_some_and(|s| hwc_sim::salvage::is_salvager(s.ship_type))
    });
    Some(SpecialGather {
        gather,
        friendlies,
        salvager,
    })
}

/// `mrRemoveDerelictsAndAllPlayerShipsFromSelecting`: without friendly targets, the
/// player's and allied ships (and derelicts, unless a salvager is selected) leave the box.
fn special_keeps(f: &Facts, g: &SpecialGather) -> bool {
    if g.friendlies {
        return true;
    }
    if f.derelict {
        return g.salvager;
    }
    !(f.own || f.ally || f.permanently_disabled)
}

/// The "selecting" list while the box is dragged (`mrSelectHold`): the gathered ships the
/// mode's filters keep. `inside` lists the ships in the box with their facts; modules stand
/// for their hosts as [`gather_inside`] picks them.
#[cfg(test)]
pub fn selecting(mode: Mode, inside: &[(ShipId, Facts)], sel: &Selected) -> Vec<ShipId> {
    selecting_with(mode, inside, sel, |id| id, |_| None)
}

/// [`selecting`] with the hosts modules stand for and their facts.
pub fn selecting_with(
    mode: Mode,
    inside: &[(ShipId, Facts)],
    sel: &Selected,
    stand_in: impl Fn(ShipId) -> ShipId,
    facts: impl Fn(ShipId) -> Option<Facts>,
) -> Vec<ShipId> {
    match mode {
        Mode::ForceAttack | Mode::Attack | Mode::Guard
            if !(if mode == Mode::Guard {
                sel.hold_capable
            } else {
                sel.attack_capable
            }) =>
        {
            return Vec::new();
        }
        _ => {}
    }
    let special = match (mode, sel.special) {
        (Mode::Special, None) => return Vec::new(),
        (_, s) => s,
    };
    let g = match (mode, special) {
        (Mode::Special, Some(s)) => s.gather,
        _ => mode.gather(),
    };
    gather_inside(inside, g, stand_in, facts)
        .iter()
        .filter(|(id, f)| match mode {
            Mode::Attack => plain_attack_target(f),
            Mode::Guard => f.ally && !sel.ships.contains(id),
            Mode::Special => special.is_some_and(|s| special_keeps(f, &s)),
            _ => true,
        })
        .map(|&(id, _)| id)
        .collect()
}

/// The targets of an attack band box on release (`mrBandBoxRelease`): the selecting list less
/// the untargetable and the attackers themselves (and, unforced, only plain targets).
pub fn attack_targets(
    selecting: &[(ShipId, Facts)],
    attackers: &[ShipId],
    forced: bool,
) -> Vec<ShipId> {
    selecting
        .iter()
        .filter(|(id, f)| !f.untargetable && !attackers.contains(id))
        .filter(|(_, f)| forced || plain_attack_target(f))
        .map(|&(id, _)| id)
        .collect()
}

/// `makeShipsHoldCapable(sel, TRUE)`: ships that are not disabled and not launching,
/// building, hyperspacing or linking.
pub fn hold_capable(u: &Universe, ships: &[ShipId]) -> Vec<ShipId> {
    ships
        .iter()
        .copied()
        .filter(|&id| {
            u.ship(id).is_some_and(|s| {
                !s.base.disabled
                    && !s
                        .command
                        .and_then(|c| u.commands.get(c))
                        .is_some_and(|c| matches!(c.order, 4 | 6 | 10..=12 | 14..=17))
            })
        })
        .collect()
}

/// `MakeShipsAttackCapable`.
pub fn attack_capable(u: &Universe, ships: &[ShipId]) -> Vec<ShipId> {
    command::attack_capable(u, ships)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ship(own: bool) -> Facts {
        Facts {
            own,
            ally: own,
            selectable: own,
            targetable: true,
            on_screen: true,
            ship_type: 3,
            ..Facts::default()
        }
    }

    #[test]
    fn modifiers_pick_the_mode_in_the_original_order() {
        let only = |a: Action| move |x: Action| x == a;
        assert_eq!(Mode::from_held(only(keys::BANDBOX_ATTACK)), Mode::Attack);
        assert_eq!(Mode::from_held(only(keys::BANDBOX_FOCUS)), Mode::Focus);
        assert_eq!(Mode::from_held(only(keys::BANDBOX_GUARD)), Mode::Guard);
        assert_eq!(Mode::from_held(|_| false), Mode::Select);
        // Force attack wins over everything; guard over focus and attack.
        assert_eq!(Mode::from_held(|_| true), Mode::ForceAttack);
        assert_eq!(
            Mode::from_held(|a| a == keys::BANDBOX_GUARD
                || a == keys::BANDBOX_ATTACK
                || a == keys::BANDBOX_FOCUS),
            Mode::Guard
        );
    }

    #[test]
    fn ctrl_band_box_highlights_only_enemies() {
        let mut ally = ship(false);
        ally.ally = true;
        let mut crystal = ship(false);
        crystal.crystal = true;
        let mut derelict = ship(false);
        derelict.derelict = true;
        let mut own_derelict = derelict;
        own_derelict.own = true;
        own_derelict.ally = true;
        own_derelict.selectable = true;
        let mut junk_hq = ship(false);
        junk_hq.ship_type = JUNKYARD_HQ;
        let mut cloaked = ship(false);
        cloaked.cloaked_unseen = true;
        let mut untargetable = ship(false);
        untargetable.untargetable_type = true;
        let inside: Vec<(ShipId, Facts)> = [
            ship(true),
            ship(false),
            ally,
            crystal,
            derelict,
            own_derelict,
            junk_hq,
            cloaked,
            untargetable,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, f)| (ShipId(i as u16 + 10), f))
        .collect();
        let mine = [ShipId(10)];
        let sel = Selected {
            ships: &mine,
            attack_capable: true,
            hold_capable: true,
            special: None,
        };
        // Ctrl: the enemy and the foreign derelict.
        assert_eq!(
            selecting(Mode::Attack, &inside, &sel),
            vec![ShipId(11), ShipId(14)]
        );
        // Nobody able to attack: nothing lights up.
        let none = Selected {
            ships: &mine,
            attack_capable: false,
            hold_capable: true,
            special: None,
        };
        assert!(selecting(Mode::Attack, &inside, &none).is_empty());
        // Ctrl+Shift: anything targetable, own ships included (not the cloaked or untargetable).
        assert_eq!(
            selecting(Mode::ForceAttack, &inside, &sel),
            (10..=16).map(ShipId).collect::<Vec<_>>()
        );
        // On release the attackers are not their own targets.
        let gathered: Vec<_> = inside
            .iter()
            .copied()
            .filter(|(id, _)| selecting(Mode::ForceAttack, &inside, &sel).contains(id))
            .collect();
        assert_eq!(
            attack_targets(&gathered, &mine, true),
            (11..=16).map(ShipId).collect::<Vec<_>>()
        );
        assert_eq!(
            attack_targets(&gathered, &mine, false),
            vec![ShipId(11), ShipId(14)]
        );
        // Alt: everything in the box; G: the friendly ships but the selection.
        assert_eq!(selecting(Mode::Focus, &inside, &sel).len(), 8);
        assert_eq!(
            selecting(Mode::Guard, &inside, &sel),
            vec![ShipId(12), ShipId(15)]
        );
        // No modifier: own ships only.
        assert_eq!(
            selecting(Mode::Select, &inside, &sel),
            vec![ShipId(10), ShipId(15)]
        );
    }

    #[test]
    fn special_band_box_gathers_for_the_selected_specials() {
        let mut derelict = ship(false);
        derelict.derelict = true;
        let inside: Vec<(ShipId, Facts)> = [ship(true), ship(false), derelict]
            .into_iter()
            .enumerate()
            .map(|(i, f)| (ShipId(i as u16 + 10), f))
            .collect();
        let mine = [ShipId(10)];
        let plain = Gather {
            own_only: false,
            anything: false,
            attack: false,
        };
        let with = |special| Selected {
            ships: &mine,
            attack_capable: true,
            hold_capable: true,
            special,
        };
        // No ship with a special-target routine: nothing is gathered.
        assert!(selecting(Mode::Special, &inside, &with(None)).is_empty());
        // Enemy targets only: own ships and (without a salvager) derelicts leave.
        let enemies = SpecialGather {
            gather: plain,
            friendlies: false,
            salvager: false,
        };
        assert_eq!(
            selecting(Mode::Special, &inside, &with(Some(enemies))),
            vec![ShipId(11)]
        );
        let salvage = SpecialGather {
            salvager: true,
            ..enemies
        };
        assert_eq!(
            selecting(Mode::Special, &inside, &with(Some(salvage))),
            vec![ShipId(11), ShipId(12)]
        );
        // Friendly targets: everything stays.
        let friends = SpecialGather {
            friendlies: true,
            ..enemies
        };
        assert_eq!(
            selecting(Mode::Special, &inside, &with(Some(friends))).len(),
            3
        );
    }
    /// A box around the Kuun-Lan's modules picks the Kuun-Lan, once.
    #[test]
    fn modules_stand_for_their_mothership() {
        let inside: Vec<(ShipId, Facts)> = [10u16, 11, 12, 20]
            .iter()
            .map(|&i| (ShipId(i), ship(true)))
            .collect();
        let host = |id: ShipId| {
            if id.0 == 11 || id.0 == 12 {
                ShipId(1)
            } else {
                id
            }
        };
        let mine = [ShipId(10)];
        let sel = Selected {
            ships: &mine,
            attack_capable: true,
            hold_capable: true,
            special: None,
        };
        assert_eq!(
            selecting_with(Mode::Select, &inside, &sel, host, |_| Some(ship(true))),
            vec![ShipId(10), ShipId(1), ShipId(20)]
        );
    }
}
