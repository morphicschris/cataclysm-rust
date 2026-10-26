//! The mouse of a restyled screen handed on to the original screen underneath.
//!
//! The modern menus ([`super::modern_menu`]) and the modern systems manager
//! ([`super::modern_launch_manager`]) keep the original screen open but undrawn. Each of their
//! controls stands for a part of that screen (a button, a check box, one radio button, a
//! slider, a list row, a text entry): a [`Target`] pairs the control's rectangle with the
//! original region's. The pointer over a control is moved onto the original region at the
//! same relative place, and the buttons go through unchanged, so the original's region
//! processor sees exactly the presses, holds, drags and releases it would have seen there:
//! a button clicks on its release over it and a press dragged off cancels it, a check box
//! toggles, a slider follows the pointer while held (and keeps it, as the original clips
//! the pointer to the slider), a list selects on its release and binds a key on its double
//! click, a text entry takes the keyboard. The pointer over nothing of the original's goes
//! far off the screen, where no region is.

use crate::canvas::Rect;
use crate::ingame::Fe;

/// Where the pointer goes while it is over none of the original's parts.
pub const AWAY: (i32, i32) = (-30_000, -30_000);

/// A modern control standing for a part of the original screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// The control on the modern screen.
    pub modern: Rect,
    /// The original region it stands for.
    pub original: Rect,
    /// The pointer is kept inside the control first (a slider's track: a press just past
    /// its end is its end).
    pub clamp: bool,
}

impl Target {
    pub fn new(modern: Rect, original: Rect) -> Target {
        Target {
            modern,
            original,
            clamp: false,
        }
    }

    /// A target that keeps the pointer inside the control.
    pub fn clamped(modern: Rect, original: Rect) -> Target {
        Target {
            clamp: true,
            ..Target::new(modern, original)
        }
    }

    /// The point of the original region at the same relative place as (`x`, `y`) in the
    /// modern control (outside it when outside the control).
    pub fn map(&self, x: i32, y: i32) -> (i32, i32) {
        let m = self.modern;
        let (x, y) = if self.clamp {
            (
                x.clamp(m.x0, (m.x1 - 1).max(m.x0)),
                y.clamp(m.y0, (m.y1 - 1).max(m.y0)),
            )
        } else {
            (x, y)
        };
        let along = |v: i32, m0: i32, m1: i32, o0: i32, o1: i32| {
            let mw = (m1 - m0).max(1) as f32;
            let ow = (o1 - o0).max(1) as f32;
            // Pixel centres map to pixel centres, so the far edge stays inside.
            let t = (v - m0) as f32 + 0.5;
            o0 + (t * ow / mw).floor() as i32
        };
        (
            along(
                x,
                self.modern.x0,
                self.modern.x1,
                self.original.x0,
                self.original.x1,
            ),
            along(
                y,
                self.modern.y0,
                self.modern.y1,
                self.original.y0,
                self.original.y1,
            ),
        )
    }
}

/// The buttons held through a target (left, right): moves map through it until released.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Proxy {
    held: [Option<Target>; 2],
    /// Where the pointer was put on the original screen last.
    pub placed: Option<(i32, i32)>,
}

impl Proxy {
    /// A button is held through a target.
    pub fn holding(&self) -> bool {
        self.held.iter().any(Option::is_some)
    }

    /// The target the left button holds.
    pub fn held_left(&self) -> Option<Target> {
        self.held[0]
    }

    /// The target the pointer maps through: the held one, else the one under it.
    fn through(&self, under: Option<Target>) -> Option<Target> {
        self.held[0].or(self.held[1]).or(under)
    }

    /// The pointer moved to (`x`, `y`), over `under`.
    pub fn moved(&mut self, fe: &mut Fe, x: i32, y: i32, under: Option<Target>) {
        let (ox, oy) = self.through(under).map_or(AWAY, |t| t.map(x, y));
        self.placed = Some((ox, oy));
        fe.mouse_move(ox, oy);
    }

    /// A button went down or up at (`x`, `y`), over `under`.
    pub fn button(
        &mut self,
        fe: &mut Fe,
        left: bool,
        down: bool,
        (x, y): (i32, i32),
        under: Option<Target>,
    ) {
        let slot = usize::from(!left);
        if down {
            if self.held[slot].is_none() {
                self.held[slot] = under;
            }
            self.moved(fe, x, y, under);
            fe.mouse_button(left, true);
        } else {
            // Released where the held control maps it: over the original region when the
            // pointer is still over the control.
            self.moved(fe, x, y, under);
            fe.mouse_button(left, false);
            self.held[slot] = None;
        }
    }

    /// Forgets what was held (the screen changed under it).
    pub fn reset(&mut self) {
        self.held = [None; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_proportionally() {
        let t = Target::new(Rect::new(100, 100, 300, 140), Rect::new(10, 20, 60, 30));
        assert_eq!(t.map(100, 100), (10, 20));
        assert_eq!(t.map(299, 139), (59, 29), "the far edge stays inside");
        assert_eq!(t.map(200, 120), (35, 25));
        // Outside the control is outside the region.
        let (x, _) = t.map(400, 120);
        assert!(x >= 60);
        let (x, _) = t.map(50, 120);
        assert!(x < 10);
        // Unless it keeps the pointer in.
        let c = Target::clamped(t.modern, t.original);
        assert_eq!(c.map(50, 120).0, 10);
        assert_eq!(c.map(400, 120).0, 59);
    }

    #[test]
    fn one_to_one_when_the_same_size() {
        let r = Rect::new(5, 5, 25, 15);
        let t = Target::new(r, r);
        for (x, y) in [(5, 5), (24, 14), (12, 9)] {
            assert_eq!(t.map(x, y), (x, y));
        }
    }
}
