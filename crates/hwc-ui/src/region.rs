//! The region tree (`Region.c`): rectangles that receive mouse and key events and draw
//! themselves. Children are kept in list order; the head of a list is processed first and drawn
//! last (on top).

use crate::canvas::Rect;
use crate::controls::Control;
use crate::fer::Cutout;

/// Handle of a region. Stale handles (of deleted regions) are detected by a generation count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegionId {
    index: u32,
    generation: u32,
}

/// Reference to an atom: screen index and atom index within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AtomRef {
    pub screen: usize,
    pub atom: usize,
}

/// Keyboard-navigation handlers attached to every screen with tab stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Tab,
    Right,
    Left,
    Up,
    Down,
    Space,
    Return,
    Escape,
    Home,
    End,
}

/// What a region does (its process function).
#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    /// Root or main region: no behaviour of its own.
    Root,
    /// A screen's base region.
    Base,
    /// Static text, rectangle, decoration, user region, cutout: draw only.
    Static,
    /// A UI control (button, slider, list...).
    Control,
    /// One key of the per-screen keyboard navigation.
    Key { action: KeyAction, buttons: u32 },
    /// Up or down arrow of a scroll bar.
    ScrollButton { scrollbar: RegionId, up: bool },
    /// A user region given a process function by name (`regFunctionSet`).
    Custom(&'static str),
}

/// How a region draws itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Draw {
    None,
    Base,
    StaticText,
    StaticRect,
    Decorative,
    User,
    Button,
    Toggle,
    CheckBox,
    Radio,
    BitmapButton,
    HorizSlider,
    VertSlider,
    ScrollBar,
    ScrollButton,
    ListWindow,
    TextEntry,
}

#[derive(Debug, Clone)]
pub struct Region {
    pub rect: Rect,
    /// Event filter (`RPE_*`).
    pub filter: u32,
    /// Status (`RSF_*`).
    pub status: u32,
    pub parent: Option<RegionId>,
    pub children: Vec<RegionId>,
    pub tabstop: u16,
    pub keys: Vec<u16>,
    pub atom: Option<AtomRef>,
    pub role: Role,
    pub draw: Draw,
    pub control: Control,
    pub cutouts: Vec<Cutout>,
    pub drawstyle: [u32; 2],
    /// Whether `feFindChildren` may adopt later siblings lying on top of this region.
    pub stackable: bool,
}

impl Region {
    pub fn new(rect: Rect, filter: u32, role: Role) -> Self {
        Self {
            rect,
            filter,
            status: crate::consts::RSF_DRAW_THIS_FRAME,
            parent: None,
            children: Vec::new(),
            tabstop: 0,
            keys: Vec::new(),
            atom: None,
            role,
            draw: Draw::None,
            control: Control::None,
            cutouts: Vec::new(),
            drawstyle: [0; 2],
            stackable: false,
        }
    }

    pub fn has(&self, status: u32) -> bool {
        self.status & status != 0
    }
}

struct Slot {
    generation: u32,
    region: Option<Region>,
}

/// Region storage with stable handles.
#[derive(Default)]
pub struct Regions {
    slots: Vec<Slot>,
    free: Vec<u32>,
}

impl Regions {
    pub fn alloc(&mut self, region: Region) -> RegionId {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.generation += 1;
            slot.region = Some(region);
            RegionId {
                index,
                generation: slot.generation,
            }
        } else {
            self.slots.push(Slot {
                generation: 0,
                region: Some(region),
            });
            RegionId {
                index: self.slots.len() as u32 - 1,
                generation: 0,
            }
        }
    }

    pub fn get(&self, id: RegionId) -> Option<&Region> {
        let slot = self.slots.get(id.index as usize)?;
        (slot.generation == id.generation).then_some(())?;
        slot.region.as_ref()
    }

    pub fn get_mut(&mut self, id: RegionId) -> Option<&mut Region> {
        let slot = self.slots.get_mut(id.index as usize)?;
        (slot.generation == id.generation).then_some(())?;
        slot.region.as_mut()
    }

    pub fn contains(&self, id: RegionId) -> bool {
        self.get(id).is_some()
    }

    /// Frees one region (its links must already be gone).
    pub fn free(&mut self, id: RegionId) {
        if let Some(slot) = self.slots.get_mut(id.index as usize) {
            if slot.generation == id.generation && slot.region.take().is_some() {
                self.free.push(id.index);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.region.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Appends `child` to `parent`'s children (`regChildInsert`).
    pub fn insert_child(&mut self, child: RegionId, parent: RegionId) {
        if let Some(c) = self.get_mut(child) {
            c.parent = Some(parent);
        }
        if let Some(p) = self.get_mut(parent) {
            p.children.push(child);
        }
    }

    /// Unlinks a region from its parent (`regLinkRemove`).
    pub fn unlink(&mut self, id: RegionId) {
        let Some(parent) = self.get(id).and_then(|r| r.parent) else {
            return;
        };
        if let Some(p) = self.get_mut(parent) {
            p.children.retain(|&c| c != id);
        }
        if let Some(r) = self.get_mut(id) {
            r.parent = None;
        }
    }

    /// Moves a region to the front of its siblings, behind any priority regions
    /// (`regSiblingMoveToFront`).
    pub fn move_to_front(&mut self, id: RegionId) {
        let Some(parent) = self.get(id).and_then(|r| r.parent) else {
            return;
        };
        let priority = self
            .get(id)
            .is_some_and(|r| r.has(crate::consts::RSF_PRIORITY_REGION));
        let siblings = self
            .get(parent)
            .map(|p| p.children.clone())
            .unwrap_or_default();
        let mut list: Vec<RegionId> = siblings.into_iter().filter(|&c| c != id).collect();
        let pos = if priority {
            0
        } else {
            list.iter()
                .position(|&c| {
                    !self
                        .get(c)
                        .is_some_and(|r| r.has(crate::consts::RSF_PRIORITY_REGION))
                })
                .unwrap_or(list.len())
        };
        list.insert(pos, id);
        if let Some(p) = self.get_mut(parent) {
            p.children = list;
        }
    }

    /// Sets `RSF_DRAW_THIS_FRAME` on a region and its subtree.
    pub fn set_dirty_recursive(&mut self, id: RegionId) {
        let children = match self.get_mut(id) {
            Some(r) => {
                r.status |= crate::consts::RSF_DRAW_THIS_FRAME;
                r.children.clone()
            }
            None => return,
        };
        for c in children {
            self.set_dirty_recursive(c);
        }
    }

    /// Clears all event bits of a subtree (`regRecursiveDisable`).
    pub fn disable_recursive(&mut self, id: RegionId) {
        let children = match self.get_mut(id) {
            Some(r) => {
                r.filter &= !crate::consts::RPM_ALL_EVENTS;
                r.children.clone()
            }
            None => return,
        };
        for c in children {
            self.disable_recursive(c);
        }
    }
}

/// `regRegionInside`: whether `needle` lies on `haystack` closely enough to become its child:
/// within its vertical extent and overlapping at least two thirds of its own width.
pub fn region_inside(needle: Rect, haystack: Rect) -> bool {
    if !(needle.y0 >= haystack.y0 && needle.y1 <= haystack.y1) {
        return false;
    }
    if needle.x0 >= haystack.x0 && needle.x1 <= haystack.x1 {
        return true;
    }
    let h_width = haystack.x1 - haystack.x0;
    let n_width = needle.x1 - needle.x0;
    let overlap = if needle.x0 < haystack.x0 {
        if needle.x1 > haystack.x1 {
            h_width
        } else if needle.x1 > haystack.x0 {
            needle.x1 - haystack.x0
        } else {
            -1
        }
    } else if haystack.x1 > needle.x0 {
        haystack.x1 - needle.x0
    } else {
        -1
    };
    overlap >= n_width * 2 / 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_detect_reuse() {
        let mut regs = Regions::default();
        let a = regs.alloc(Region::new(Rect::default(), 0, Role::Root));
        regs.free(a);
        let b = regs.alloc(Region::new(Rect::default(), 0, Role::Root));
        assert!(regs.get(a).is_none());
        assert!(regs.get(b).is_some());
        assert_eq!(regs.len(), 1);
    }

    #[test]
    fn move_to_front_respects_priority() {
        let mut regs = Regions::default();
        let p = regs.alloc(Region::new(Rect::default(), 0, Role::Root));
        let mut pri = Region::new(Rect::default(), 0, Role::Base);
        pri.status |= crate::consts::RSF_PRIORITY_REGION;
        let a = regs.alloc(pri);
        let b = regs.alloc(Region::new(Rect::default(), 0, Role::Base));
        let c = regs.alloc(Region::new(Rect::default(), 0, Role::Base));
        for id in [a, b, c] {
            regs.insert_child(id, p);
        }
        regs.move_to_front(c);
        assert_eq!(regs.get(p).unwrap().children, vec![a, c, b]);
    }

    #[test]
    fn inside_rule() {
        let button = Rect::new(218, 208, 424, 240);
        let label = Rect::new(278, 217, 365, 231);
        assert!(region_inside(label, button));
        assert!(!region_inside(button, label));
        // Two-thirds horizontal overlap is enough when vertically contained.
        assert!(region_inside(Rect::new(400, 210, 430, 230), button));
        assert!(!region_inside(Rect::new(410, 210, 440, 230), button));
    }
}
