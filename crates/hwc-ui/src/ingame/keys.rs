//! Configurable key bindings (`UserKeys.c` / `Key.c`, Cataclysm-only): the 147 key actions in
//! the engine's order, up to three bindings each (virtual-key code plus Shift/Alt/Ctrl), read
//! from `keys.cfg` (or `default_keys.cfg`), matched on key down/up and dispatched.

use std::collections::HashSet;

/// Bindings one action can hold.
pub const MAX_BINDINGS: usize = 3;
/// Version line of the generated file.
pub const FILE_VERSION: u32 = 84;

/// A key action, 1-based like the engine's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Action(pub u16);

impl Action {
    pub fn name(self) -> &'static str {
        ACTION_NAMES
            .get(usize::from(self.0).wrapping_sub(1))
            .copied()
            .unwrap_or("")
    }

    pub fn from_name(name: &str) -> Option<Action> {
        ACTION_NAMES
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
            .map(|i| Action(i as u16 + 1))
    }

    pub fn all() -> impl Iterator<Item = Action> {
        (1..=ACTION_NAMES.len() as u16).map(Action)
    }

    /// Hot-key group 1..9 of the group actions: (kind, group).
    pub fn ship_group(self) -> Option<(GroupOp, u8)> {
        let i = self.0;
        let op = match i {
            108..=116 => GroupOp::Create,
            117..=125 => GroupOp::Select,
            126..=134 => GroupOp::AddSelect,
            _ => return None,
        };
        let first = match op {
            GroupOp::Create => 108,
            GroupOp::Select => 117,
            GroupOp::AddSelect => 126,
        };
        Some((op, (i - first + 1) as u8))
    }

    /// Actions read by polling the key state every frame instead of a callback (pan/zoom,
    /// band-box modifiers).
    pub fn is_polled(self) -> bool {
        matches!(
            self,
            PAN_SENSORS_LEFT
                | PAN_SENSORS_RIGHT
                | PAN_SENSORS_UP
                | PAN_SENSORS_DOWN
                | ZOOM_IN
                | ZOOM_OUT
                | BANDBOX_GUARD
                | BANDBOX_ATTACK
                | BANDBOX_FORCE_ATTACK
                | BANDBOX_FOCUS
        )
    }

    /// Actions with a key-up handler too.
    pub fn has_up(self) -> bool {
        matches!(self, SHIP_SPECIAL | ENABLE_HEIGHT_MOVE | MULTIPLE_SELECT)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupOp {
    Create,
    Select,
    /// `AddTo_ShipGroup_n`: the group joins the selection (`kbcbGroupAddSelect`); the group
    /// itself does not change.
    AddSelect,
}

/// Configuration names of the actions, in table order (`keys.cfg` names).
pub const ACTION_NAMES: [&str; 147] = [
    "Move",
    "Dock",
    "Retire",
    "Cancel_Orders",
    "Scuttle_Ships",
    "Kamikaze",
    "Ship_Special",
    "Hyperspace",
    "Waypoints",
    "Toggle_Waypoint_Modes",
    "Waypoint_Mode_Normal",
    "Waypoint_Mode_EndToEnd",
    "Waypoint_Mode_Circular",
    "Cancel_Last_Waypoint",
    "Upgrade_Ships",
    "Enter_Slipgate",
    "Harvest",
    "Repair",
    "Salvage",
    "Select_Leech_Type",
    "Leech_Spy",
    "Leech_Leech",
    "Leech_Vect",
    "Leech",
    "UnLeech",
    "Link",
    "Unlink",
    "Select_Mimic_Type",
    "Select_Mimic_Item",
    "Cancel_Mimic",
    "Force_Field_Options",
    "Force_Field_Unlink",
    "Cloak",
    "Cancel_Cloak",
    "Launch_Swarmers",
    "Cancel_Swarmers",
    "Big_Gun_Attack",
    "Cancel_BigGun",
    "Beast_Attack",
    "EMP",
    "Ram",
    "Repulse",
    "Fire_Missiles",
    "Tactics_Evasive",
    "Tactics_Neutral",
    "Tactics_Aggresive",
    "Next_Tactic_Level",
    "Prev_Tactic_Level",
    "Formation_Delta",
    "Formation_Broad",
    "Formation_Delta3D",
    "Formation_Claw",
    "Formation_Wall",
    "Formation_Sphere",
    "Formation_Custom",
    "Formation_Parade",
    "Next_Formation",
    "Prev_Formation",
    "Focus",
    "Focus_On_Mothership",
    "Focus_On_LastShipToSpeak",
    "Cancel_Focus",
    "Forward_Focus",
    "Fleet_View",
    "Pan_Sensors_Left",
    "Pan_Sensors_Right",
    "Pan_Sensors_Up",
    "Pan_Sensors_Down",
    "Zoom_In",
    "Zoom_Out",
    "Toggle_Chase_Mode",
    "Select_All_Visible_Ships",
    "Select_None",
    "Bandbox_Guard",
    "Bandbox_Attack",
    "Bandbox_ForceAttack",
    "Bandbox_Focus",
    "Multiple_Select",
    "Toggle_SensorsManager",
    "Open_BuildManager",
    "Open_ResearchManager",
    "Open_LaunchManager",
    "Toggle_Tactical_Display",
    "Toggle_Ship_Info",
    "Next_Overlay_View",
    "Open_Menu",
    "Enable_Height_Move",
    "PageDown_GameChat",
    "PageUp_GameChat",
    "Escape",
    "Start_Ally_Message",
    "Start_Global_Message",
    "Accelerate_Time",
    "Decelerate_Time",
    "Toggle_Game_Speed",
    "Next_Music_Track",
    "Prev_Music_Track",
    "Toggle_Overlay_Options",
    "Change_Overlay_Backdrops_Toggle",
    "Show_Keys_1",
    "Show_Keys_2",
    "Show_Keys_3",
    "GammaUp",
    "GammaDown",
    "Pause",
    "Take_Screenshot",
    "Skip_Sensors",
    "Create_ShipGroup_1",
    "Create_ShipGroup_2",
    "Create_ShipGroup_3",
    "Create_ShipGroup_4",
    "Create_ShipGroup_5",
    "Create_ShipGroup_6",
    "Create_ShipGroup_7",
    "Create_ShipGroup_8",
    "Create_ShipGroup_9",
    "Select_ShipGroup_1",
    "Select_ShipGroup_2",
    "Select_ShipGroup_3",
    "Select_ShipGroup_4",
    "Select_ShipGroup_5",
    "Select_ShipGroup_6",
    "Select_ShipGroup_7",
    "Select_ShipGroup_8",
    "Select_ShipGroup_9",
    "AddTo_ShipGroup_1",
    "AddTo_ShipGroup_2",
    "AddTo_ShipGroup_3",
    "AddTo_ShipGroup_4",
    "AddTo_ShipGroup_5",
    "AddTo_ShipGroup_6",
    "AddTo_ShipGroup_7",
    "AddTo_ShipGroup_8",
    "AddTo_ShipGroup_9",
    "Remove_Selected_Ship_Group",
    "ToggleSV_Asteroids",
    "ToggleSV_FOWRanges",
    "ToggleSV_AreasVisited",
    "ToggleSV_AreasOccupied",
    "ToggleSV_Icons",
    "ToggleSV_ShipMeshes",
    "ToggleSV_PieplateLines",
    "ToggleSV_Background",
    "ToggleSV_DecorativeAsteroids",
    "ToggleSV_Compass",
    "Toggle_SV_ObjPoint",
    "Toggle_SV_SlipgatePaths",
];

macro_rules! actions {
    ($($name:ident = $n:literal),* $(,)?) => {
        $(pub const $name: Action = Action($n);)*
    };
}

actions! {
    MOVE = 1, DOCK = 2, RETIRE = 3, CANCEL_ORDERS = 4, SCUTTLE = 5, KAMIKAZE = 6,
    SHIP_SPECIAL = 7, HYPERSPACE = 8, WAYPOINTS = 9, TOGGLE_WAYPOINT_MODES = 10,
    WAYPOINT_MODE_NORMAL = 11, WAYPOINT_MODE_END_TO_END = 12, WAYPOINT_MODE_CIRCULAR = 13,
    CANCEL_LAST_WAYPOINT = 14, UPGRADE = 15, ENTER_SLIPGATE = 16, HARVEST = 17, REPAIR = 18,
    SALVAGE = 19, SELECT_LEECH_TYPE = 20, LEECH_SPY = 21, LEECH_LEECH = 22, LEECH_VECT = 23,
    LEECH = 24, UNLEECH = 25, LINK = 26, UNLINK = 27, SELECT_MIMIC_TYPE = 28,
    SELECT_MIMIC_ITEM = 29, CANCEL_MIMIC = 30, FORCE_FIELD_OPTIONS = 31,
    FORCE_FIELD_UNLINK = 32, CLOAK = 33, CANCEL_CLOAK = 34, LAUNCH_SWARMERS = 35,
    CANCEL_SWARMERS = 36, BIG_GUN_ATTACK = 37, CANCEL_BIG_GUN = 38, BEAST_ATTACK = 39,
    EMP = 40, RAM = 41, REPULSE = 42, FIRE_MISSILES = 43, TACTICS_EVASIVE = 44,
    TACTICS_NEUTRAL = 45, TACTICS_AGGRESSIVE = 46, NEXT_TACTIC = 47, PREV_TACTIC = 48,
    FORMATION_DELTA = 49, FORMATION_BROAD = 50, FORMATION_DELTA3D = 51, FORMATION_CLAW = 52,
    FORMATION_WALL = 53, FORMATION_SPHERE = 54, FORMATION_CUSTOM = 55, FORMATION_PARADE = 56,
    NEXT_FORMATION = 57, PREV_FORMATION = 58, FOCUS = 59, FOCUS_ON_MOTHERSHIP = 60,
    FOCUS_ON_LAST_SPEAKER = 61, CANCEL_FOCUS = 62, FORWARD_FOCUS = 63, FLEET_VIEW = 64,
    PAN_SENSORS_LEFT = 65, PAN_SENSORS_RIGHT = 66, PAN_SENSORS_UP = 67, PAN_SENSORS_DOWN = 68,
    ZOOM_IN = 69, ZOOM_OUT = 70, TOGGLE_CHASE_MODE = 71, SELECT_ALL_VISIBLE = 72,
    SELECT_NONE = 73, BANDBOX_GUARD = 74, BANDBOX_ATTACK = 75, BANDBOX_FORCE_ATTACK = 76,
    BANDBOX_FOCUS = 77, MULTIPLE_SELECT = 78, TOGGLE_SENSORS_MANAGER = 79,
    OPEN_BUILD_MANAGER = 80, OPEN_RESEARCH_MANAGER = 81, OPEN_LAUNCH_MANAGER = 82,
    TOGGLE_TACTICAL_DISPLAY = 83, TOGGLE_SHIP_INFO = 84, NEXT_OVERLAY_VIEW = 85,
    OPEN_MENU = 86, ENABLE_HEIGHT_MOVE = 87, PAGE_DOWN_GAME_CHAT = 88, PAGE_UP_GAME_CHAT = 89,
    ESCAPE = 90, START_ALLY_MESSAGE = 91, START_GLOBAL_MESSAGE = 92, ACCELERATE_TIME = 93,
    DECELERATE_TIME = 94, TOGGLE_GAME_SPEED = 95, NEXT_MUSIC_TRACK = 96, PREV_MUSIC_TRACK = 97,
    TOGGLE_OVERLAY_OPTIONS = 98, CHANGE_OVERLAY_BACKDROPS = 99, SHOW_KEYS_1 = 100,
    SHOW_KEYS_2 = 101, SHOW_KEYS_3 = 102, GAMMA_UP = 103, GAMMA_DOWN = 104, PAUSE = 105,
    TAKE_SCREENSHOT = 106, SKIP_SENSORS = 107, REMOVE_SELECTED_SHIP_GROUP = 135,
}

/// Modifier flags of a binding.
pub const MOD_SHIFT: u8 = 1;
pub const MOD_ALT: u8 = 2;
pub const MOD_CTRL: u8 = 4;

/// One key binding: a virtual-key code and the modifiers that must be held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Binding {
    pub key: u16,
    pub mods: u8,
}

impl Binding {
    /// Parses `ALT+SHIFT+CTRL+<vk decimal>` (any subset of the prefixes, any order).
    pub fn parse(text: &str) -> Option<Binding> {
        let text = text.trim().trim_matches('"');
        let mut mods = 0;
        let mut key = None;
        for part in text.split('+') {
            match part.trim().to_ascii_uppercase().as_str() {
                "SHIFT" => mods |= MOD_SHIFT,
                "ALT" => mods |= MOD_ALT,
                "CTRL" => mods |= MOD_CTRL,
                p => key = Some(p.parse::<u16>().ok()?),
            }
        }
        let key = key?;
        (key != 0).then_some(Binding { key, mods })
    }

    /// The configuration text (`ALT+SHIFT+CTRL+<vk>` order as the engine writes it).
    pub fn to_config(self) -> String {
        let mut s = String::new();
        if self.mods & MOD_ALT != 0 {
            s.push_str("ALT+");
        }
        if self.mods & MOD_SHIFT != 0 {
            s.push_str("SHIFT+");
        }
        if self.mods & MOD_CTRL != 0 {
            s.push_str("CTRL+");
        }
        s.push_str(&self.key.to_string());
        s
    }

    /// Readable text for the options list (`SHIFT+S`, `F2`, `NUMPAD +`...).
    pub fn display(self) -> String {
        let mut s = String::new();
        if self.mods & MOD_ALT != 0 {
            s.push_str("ALT+");
        }
        if self.mods & MOD_SHIFT != 0 {
            s.push_str("SHIFT+");
        }
        if self.mods & MOD_CTRL != 0 {
            s.push_str("CTRL+");
        }
        s.push_str(&key_name(self.key));
        s
    }

    /// Whether the held modifiers are exactly the binding's (`kbBindingModifiersMatch`); a
    /// binding of Shift, Ctrl or Alt itself ignores that modifier's state.
    pub fn modifiers_match(self, mods: u8) -> bool {
        let ignore = match self.key {
            0x10 => MOD_SHIFT,
            0x11 => MOD_CTRL,
            0x12 => MOD_ALT,
            _ => 0,
        };
        self.mods & !ignore == mods & !ignore
    }
}

/// Whether two actions may share a key combination (`kbFindActionForKey`): height move with
/// multiple select, sensors with skip sensors, and the cancel actions among themselves.
pub fn may_share(a: Action, b: Action) -> bool {
    const CANCELS: [u16; 7] = [25, 27, 30, 32, 34, 36, 38];
    let pair = |x: u16, y: u16| (a.0 == x && b.0 == y) || (a.0 == y && b.0 == x);
    pair(87, 78) || pair(79, 107) || (CANCELS.contains(&a.0) && CANCELS.contains(&b.0))
}

/// Whether a key has a printable name (else the rebinding screen calls it invalid).
pub fn key_has_name(vk: u16) -> bool {
    !key_name(vk).starts_with('#')
}

/// Name of a virtual-key code as the key lists show it.
pub fn key_name(vk: u16) -> String {
    let named = match vk {
        0x08 => "BACKSPACE",
        0x09 => "TAB",
        0x0d => "ENTER",
        0x10 => "SHIFT",
        0x11 => "CTRL",
        0x12 => "ALT",
        0x13 => "PAUSE",
        0x14 => "CAPS LOCK",
        0x1b => "ESC",
        0x20 => "SPACE",
        0x21 => "PAGE UP",
        0x22 => "PAGE DOWN",
        0x23 => "END",
        0x24 => "HOME",
        0x25 => "LEFT",
        0x26 => "UP",
        0x27 => "RIGHT",
        0x28 => "DOWN",
        0x2d => "INSERT",
        0x2e => "DELETE",
        0x6a => "NUMPAD *",
        0x6b => "NUMPAD +",
        0x6d => "NUMPAD -",
        0x6e => "NUMPAD .",
        0x6f => "NUMPAD /",
        0x90 => "NUM LOCK",
        0x91 => "SCROLL LOCK",
        0xba => ";",
        0xbb => "=",
        0xbc => ",",
        0xbd => "-",
        0xbe => ".",
        0xbf => "/",
        0xc0 => "`",
        0xdb => "[",
        0xdc => "\\",
        0xdd => "]",
        0xde => "'",
        _ => "",
    };
    if !named.is_empty() {
        return named.into();
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5a => char::from(vk as u8).to_string(),
        0x60..=0x69 => format!("NUMPAD {}", vk - 0x60),
        0x70..=0x87 => format!("F{}", vk - 0x6f),
        _ => format!("#{vk}"),
    }
}

/// Every action's bindings (`0x9142e0`, one record of up to three per action).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBindings {
    pub bindings: Vec<Vec<Binding>>,
    /// Lines of the file for commands outside the 147 (debug keys), kept in order.
    pub extra: Vec<(String, Binding)>,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            bindings: vec![Vec::new(); ACTION_NAMES.len()],
            extra: Vec::new(),
        }
    }
}

impl KeyBindings {
    /// Reads `default_keys.cfg` / `keys.cfg`: a `v <version>` line, then `key <Action> <binding>`
    /// lines; an action listed twice gets both bindings (up to three).
    pub fn parse(text: &str) -> KeyBindings {
        let mut kb = KeyBindings::default();
        for line in text.lines() {
            let line = line.split(';').next().unwrap_or("").trim();
            let mut it = line.split_whitespace();
            if it.next() != Some("key") {
                continue;
            }
            let (Some(name), Some(b)) = (it.next(), it.next()) else {
                continue;
            };
            let Some(b) = Binding::parse(b) else { continue };
            match Action::from_name(name) {
                Some(a) => kb.add(a, b),
                None => kb.extra.push((name.to_string(), b)),
            }
        }
        kb
    }

    /// The file text, as the engine writes `keys.cfg`.
    pub fn to_config(&self) -> String {
        let mut s =
            format!("v {FILE_VERSION}\n; Generated Keys Configuration File - DO NOT EDIT!\n\n");
        for a in Action::all() {
            for b in self.of(a) {
                s.push_str(&format!("key  {}  {}\n", a.name(), b.to_config()));
            }
        }
        for (name, b) in &self.extra {
            s.push_str(&format!("key  {name}  {}\n", b.to_config()));
        }
        s
    }

    pub fn of(&self, a: Action) -> &[Binding] {
        self.bindings
            .get(usize::from(a.0).wrapping_sub(1))
            .map_or(&[], Vec::as_slice)
    }

    /// Adds a binding (ignored once the action has three, or if it is already there).
    pub fn add(&mut self, a: Action, b: Binding) {
        if let Some(v) = self.bindings.get_mut(usize::from(a.0).wrapping_sub(1)) {
            if v.len() < MAX_BINDINGS && !v.contains(&b) {
                v.push(b);
            }
        }
    }

    /// Replaces an action's bindings with a single one.
    pub fn set(&mut self, a: Action, b: Option<Binding>) {
        if let Some(v) = self.bindings.get_mut(usize::from(a.0).wrapping_sub(1)) {
            v.clear();
            v.extend(b);
        }
    }

    /// Actions bound to this key with these modifiers, in table order (`kbProcessKeyDown`
    /// walks every action and fires each match).
    pub fn actions_for(&self, key: u16, mods: u8) -> Vec<Action> {
        Action::all()
            .filter(|&a| {
                self.of(a)
                    .iter()
                    .any(|b| b.key == key && b.modifiers_match(mods))
            })
            .collect()
    }

    /// Actions whose binding uses this key, whatever the modifiers (key-up handling).
    pub fn actions_with_key(&self, key: u16) -> Vec<Action> {
        Action::all()
            .filter(|&a| self.of(a).iter().any(|b| b.key == key))
            .collect()
    }

    /// Whether an action is held: its key is down with exactly its modifiers (polled actions).
    pub fn is_held(&self, a: Action, down: &HashSet<u16>, mods: u8) -> bool {
        self.of(a)
            .iter()
            .any(|b| down.contains(&b.key) && b.modifiers_match(mods))
    }

    /// `kbSetBindingFromList`: the same combo again clears the action; the combo is taken from
    /// actions that may not share it; a fourth binding replaces the three.
    pub fn set_from_list(&mut self, a: Action, b: Binding) {
        let i = usize::from(a.0).wrapping_sub(1);
        if i >= self.bindings.len() {
            return;
        }
        if self.bindings[i].contains(&b) {
            self.bindings[i].clear();
            return;
        }
        for other in Action::all() {
            if other != a && !may_share(a, other) {
                self.bindings[usize::from(other.0) - 1].retain(|x| *x != b);
            }
        }
        if self.bindings[i].len() >= MAX_BINDINGS {
            self.bindings[i].clear();
        }
        self.bindings[i].push(b);
    }

    /// Other actions already using a binding (conflicts shown by the rebinding screen).
    pub fn conflicts(&self, b: Binding, except: Action) -> Vec<Action> {
        Action::all()
            .filter(|&a| a != except && self.of(a).contains(&b))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_147_actions() {
        assert_eq!(ACTION_NAMES.len(), 147);
        assert_eq!(Action::from_name("Escape"), Some(ESCAPE));
        assert_eq!(OPEN_BUILD_MANAGER.name(), "Open_BuildManager");
        assert_eq!(
            REMOVE_SELECTED_SHIP_GROUP.name(),
            "Remove_Selected_Ship_Group"
        );
        assert_eq!(Action(147).name(), "Toggle_SV_SlipgatePaths");
        assert_eq!(Action(117).ship_group(), Some((GroupOp::Select, 1)));
        assert_eq!(Action(134).ship_group(), Some((GroupOp::AddSelect, 9)));
    }

    #[test]
    fn bindings_round_trip() {
        let b = Binding::parse("SHIFT+CTRL+76").unwrap();
        assert_eq!(
            b,
            Binding {
                key: 76,
                mods: MOD_SHIFT | MOD_CTRL
            }
        );
        assert_eq!(b.to_config(), "SHIFT+CTRL+76");
        assert_eq!(b.display(), "SHIFT+CTRL+L");
        assert_eq!(
            Binding::parse("ALT+SHIFT+CTRL+78").unwrap().to_config(),
            "ALT+SHIFT+CTRL+78"
        );
        assert!(Binding::parse("SHIFT+").is_none());
        let kb = KeyBindings::parse(
            "v 84\nkey  Move  77\nkey  Zoom_In  187\nkey  Zoom_In  107\nkey  Play_NIS  SHIFT+CTRL+48\n",
        );
        assert_eq!(kb.of(ZOOM_IN).len(), 2);
        assert_eq!(kb.extra.len(), 1);
        let again = KeyBindings::parse(&kb.to_config());
        assert_eq!(again, kb);
        assert_eq!(kb.actions_for(77, 0), vec![MOVE]);
        assert!(kb.actions_for(77, MOD_SHIFT).is_empty());
    }

    #[test]
    fn rebinding_steals_and_toggles() {
        let mut kb = KeyBindings::parse(
            "key  Move  77
key  Dock  68
",
        );
        let m = Binding { key: 77, mods: 0 };
        kb.set_from_list(DOCK, m);
        assert!(kb.of(MOVE).is_empty());
        assert_eq!(kb.of(DOCK), &[Binding { key: 68, mods: 0 }, m]);
        kb.set_from_list(DOCK, m);
        assert!(kb.of(DOCK).is_empty());
        // Cancel actions share a combo.
        let c = Binding {
            key: 192,
            mods: MOD_SHIFT,
        };
        kb.set_from_list(Action(25), c);
        kb.set_from_list(Action(27), c);
        assert_eq!(kb.of(Action(25)), &[c]);
        // Shift bound alone ignores the shift state.
        let shift = Binding { key: 0x10, mods: 0 };
        assert!(shift.modifiers_match(MOD_SHIFT));
    }
}
