//! Per-control state (`UIControls.c`): sliders, scroll bars, list windows and text entries.

use crate::canvas::Rect;
use crate::region::RegionId;

/// What a slider does when pressed or dragged: by default it calls the atom's callback like a
/// button; option callbacks replace that with a value handler when the screen is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliderHandler {
    /// `feButtonProcess`: run the atom's callback.
    Callback,
    /// A registered slider handler, by name.
    Named(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Slider {
    pub value: u32,
    pub max: u32,
    /// Lowest value of a vertical slider (unused by the shipped screens).
    pub min: u32,
    pub handler: SliderHandler,
}

impl Default for Slider {
    fn default() -> Self {
        Self {
            value: 0,
            max: 100,
            min: 0,
            handler: SliderHandler::Callback,
        }
    }
}

/// One entry of a list window.
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub text: String,
    /// Caller data (index into the callback's own table).
    pub data: u32,
    pub can_select: bool,
    pub selected: bool,
}

/// `uiclistwindow`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListWindow {
    pub items: Vec<ListItem>,
    /// Index of the first visible item (`UpperIndex`).
    pub upper: usize,
    /// Number of visible rows (`MaxIndex`).
    pub max_visible: usize,
    pub item_height: i32,
    pub title_height: i32,
    pub selected: Option<usize>,
    /// Font used by the default item drawer.
    pub font: Option<String>,
    pub can_have_focus: bool,
    /// Last message for the callback (`CM_*`).
    pub message: u32,
    pub scrollbar: Option<RegionId>,
}

impl ListWindow {
    /// `uicListWindowInit`: rows that fit between the margins.
    pub fn init(&mut self, rect: Rect, item_height: i32, title_height: i32) {
        self.item_height = item_height.max(1);
        self.title_height = title_height;
        self.max_visible = ((rect.y1 - title_height - rect.y0 - LIST_MARGIN * 2) / self.item_height)
            .max(0) as usize;
    }

    pub fn add(&mut self, text: impl Into<String>, data: u32, selected: bool) {
        let index = self.items.len();
        self.items.push(ListItem {
            text: text.into(),
            data,
            can_select: true,
            selected,
        });
        if selected {
            self.select(index);
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.upper = 0;
        self.selected = None;
    }

    pub fn select(&mut self, index: usize) {
        if let Some(old) = self.selected.and_then(|i| self.items.get_mut(i)) {
            old.selected = false;
        }
        if let Some(item) = self.items.get_mut(index) {
            item.selected = true;
            self.selected = Some(index);
        }
    }

    /// `uicListGetItemClicked`: the selectable item under screen row `y`.
    pub fn item_at(&self, rect: Rect, y: i32) -> Option<usize> {
        let mut row_y = rect.y0 + self.title_height + LIST_MARGIN;
        let mut index = self.upper;
        let mut count = 0;
        while index < self.items.len() && count < self.max_visible {
            if y < row_y + self.item_height {
                break;
            }
            row_y += self.item_height;
            count += 1;
            index += 1;
        }
        // Like the engine, a click in the bottom margin picks the first row below the window.
        if index >= self.items.len() {
            return None;
        }
        self.items[index].can_select.then_some(index)
    }

    /// Wheel up: three rows back (`feWheelNegative`).
    pub fn wheel_up(&mut self) {
        self.upper = self.upper.saturating_sub(3);
    }

    /// Wheel down: three rows on, without scrolling past the end (`feWheelPositive`).
    pub fn wheel_down(&mut self) {
        let last = self.items.len().saturating_sub(self.max_visible);
        self.upper = (self.upper + 3).min(last);
    }

    /// Scrolls so the selection is visible (`uicListWindowViewSelection`).
    pub fn view_selection(&mut self) {
        if let Some(sel) = self.selected {
            if sel < self.upper {
                self.upper = sel;
            } else if self.max_visible > 0 && sel >= self.upper + self.max_visible {
                self.upper = sel + 1 - self.max_visible;
            }
        }
    }

    /// Moves the selection by `delta` rows among selectable items.
    pub fn step(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let cur = self.selected.map_or(0, |i| i as isize);
        let next = (cur + delta).clamp(0, self.items.len() as isize - 1) as usize;
        if self.items[next].can_select {
            self.select(next);
            self.view_selection();
        }
    }
}

/// Inner margin of list windows (`uicListWindowMargin`).
pub const LIST_MARGIN: i32 = 6;

/// `uicscrollbar` (vertical only, like every scroll bar the game uses).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScrollBar {
    /// The tab region the thumb moves in.
    pub thumb_region: Rect,
    pub thumb: Rect,
    pub div_size: f32,
    pub up: Option<RegionId>,
    pub down: Option<RegionId>,
    /// List window this bar scrolls.
    pub list: Option<RegionId>,
    /// Thumb drag origin.
    pub drag_y: Option<i32>,
}

impl ScrollBar {
    /// `scAdjustThumbwheel`.
    pub fn adjust(&mut self, upper: usize, visible: usize, total: usize) {
        let height = self.thumb_region.height().max(0);
        if total <= visible {
            self.div_size = 0.0;
            self.thumb.y0 = self.thumb_region.y0;
            self.thumb.y1 = self.thumb_region.y0 + height;
        } else {
            let thumb_h = (height as f32 * visible as f32 / total as f32) as i32;
            let room = height - thumb_h;
            self.div_size = room as f32 / (total - visible) as f32;
            let top = (self.div_size * upper as f32) as i32;
            self.thumb.y0 = self.thumb_region.y0 + top;
            self.thumb.y1 = self.thumb_region.y0 + top + thumb_h;
        }
    }
}

/// `uictextentry` (single line).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextEntry {
    pub text: String,
    pub cursor: usize,
    pub max_len: usize,
    pub message: u32,
}

/// Control payload of a region.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Control {
    #[default]
    None,
    Button,
    Slider(Slider),
    ScrollBar(ScrollBar),
    List(ListWindow),
    Text(TextEntry),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(n: usize) -> ListWindow {
        let mut l = ListWindow::default();
        l.init(Rect::new(0, 0, 100, 6 * 2 + 10 * 5), 10, 0);
        for i in 0..n {
            l.add(format!("item {i}"), i as u32, false);
        }
        l
    }

    #[test]
    fn rows_and_hit_testing() {
        let mut l = list(8);
        assert_eq!(l.max_visible, 5);
        assert_eq!(l.item_at(Rect::new(0, 0, 100, 62), 6), Some(0));
        assert_eq!(l.item_at(Rect::new(0, 0, 100, 62), 27), Some(2));
        l.upper = 3;
        assert_eq!(l.item_at(Rect::new(0, 0, 100, 62), 6), Some(3));
        assert_eq!(l.item_at(Rect::new(0, 0, 100, 62), 60), None);
    }

    #[test]
    fn wheel_and_selection() {
        let mut l = list(8);
        l.wheel_down();
        assert_eq!(l.upper, 3);
        l.wheel_down();
        assert_eq!(l.upper, 3);
        l.wheel_up();
        assert_eq!(l.upper, 0);
        l.select(7);
        l.view_selection();
        assert_eq!(l.upper, 3);
        l.step(-5);
        assert_eq!(l.selected, Some(2));
        assert_eq!(l.upper, 2);
        assert_eq!(l.items.iter().filter(|i| i.selected).count(), 1);
    }

    #[test]
    fn thumb() {
        let mut s = ScrollBar {
            thumb_region: Rect::new(0, 20, 16, 120),
            ..Default::default()
        };
        s.adjust(0, 5, 3);
        assert_eq!((s.thumb.y0, s.thumb.y1), (20, 120));
        s.adjust(5, 5, 10);
        assert_eq!((s.thumb.y0, s.thumb.y1), (70, 120));
    }
}
