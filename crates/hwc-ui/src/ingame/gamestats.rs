//! The statistics at the end of a multiplayer game (`GameStats.c`'s `gstat*`): seventeen rows
//! (survival time, players killed, RUs and crystals harvested, technologies researched, and
//! fighters, frigates, destroyers and non-combat ships built, lost and killed), a column per
//! player in the order of the time they survived, and under them a bar for each player of the
//! row picked (a click on a row, or the arrow keys); Done goes back to the menu
//! (`gstatClose` → `utyGameQuitToMainAfterStats`).

use crate::canvas::{Canvas, Rect};

use super::host::GameRequest;
use super::Fe;

/// The rows, as `gstatDraw` lists them (string, which value of [`StatPlayer::values`]).
pub const ROWS: [(&str, &str); 17] = [
    ("str_GSD_SurvivalTime", "SURVIVAL TIME"),
    ("str_GSD_PlayersKilled", "PLAYERS KILLED"),
    ("str_GSD_ResourcesHarvested", "RESOURCES HARVESTED"),
    ("str_GSD_CrystalsHarvested", "CRYSTALS HARVESTED"),
    ("str_GSD_TechResearched", "TECHNOLOGY RESEARCHED"),
    ("str_GSD_FightersBuilt", "FIGHTERS BUILT"),
    ("str_GSD_FightersLost", "FIGHTERS LOST"),
    ("str_GSD_FightersKilled", "FIGHTERS KILLED"),
    ("str_GSD_FrigatesBuilt", "FRIGATES BUILT"),
    ("str_GSD_FrigatesLost", "FRIGATES LOST"),
    ("str_GSD_FrigatesKilled", "FRIGATES KILLED"),
    ("str_GSD_DestroyersBuilt", "DESTROYERS BUILT"),
    ("str_GSD_DestroyersLost", "DESTROYERS LOST"),
    ("str_GSD_DestroyersKilled", "DESTROYERS KILLED"),
    ("str_GSD_NonCombatBuilt", "NON-COMBAT BUILT"),
    ("str_GSD_NonCombatLost", "NON-COMBAT LOST"),
    ("str_GSD_NonCombatKilled", "NON-COMBAT KILLED"),
];

/// One player's column.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatPlayer {
    pub name: String,
    pub colour: [u8; 4],
    /// The local player (green), an ally (blue) or an enemy (red).
    pub local: bool,
    pub ally: bool,
    /// The row values in [`ROWS`] order (the first in seconds).
    pub values: [i32; 17],
}

/// The screen's state.
#[derive(Debug, Clone, Default)]
pub struct State {
    /// The row whose bars show (`gstatSelectedStat`).
    pub selected: usize,
    /// Seconds since the row was picked (the bars grow over half a second).
    pub age: f32,
    /// The layout of the last frame: the rows' rectangles and the Done button.
    rows: Vec<Rect>,
    done: Option<Rect>,
}

/// The players in the order of the time they survived, longest first (`gstatOpen`'s sort).
pub fn order(players: &[StatPlayer]) -> Vec<usize> {
    let mut o: Vec<usize> = (0..players.len()).collect();
    o.sort_by(|&a, &b| players[b].values[0].cmp(&players[a].values[0]));
    o
}

/// A value as the screen prints it: the time as h:mm:ss, the rest as numbers.
pub fn value_text(row: usize, v: i32) -> String {
    if row == 0 {
        let m = v / 60;
        format!("{}:{:02}:{:02}", m / 60, m % 60, v % 60)
    } else {
        v.to_string()
    }
}

/// `gstatProcessClick`: a row picks it, Done asks to leave.
pub fn click(fe: &mut Fe, x: i32, y: i32) -> bool {
    if fe.game.ig.view.end_stats.is_empty() {
        return false;
    }
    let st = &mut fe.game.ig.stats;
    if st.done.is_some_and(|r| r.contains(x, y)) {
        fe.game.ig.request(GameRequest::StatsDone);
        return true;
    }
    if let Some(i) = st.rows.iter().position(|r| r.contains(x, y)) {
        st.selected = i;
        st.age = 0.0;
    }
    true
}

/// The arrow keys move the picked row (`gstatDraw`'s key test).
pub fn key(fe: &mut Fe, down: bool) {
    let st = &mut fe.game.ig.stats;
    st.selected = if down { (st.selected + 1) % ROWS.len() } else { (st.selected + ROWS.len() - 1) % ROWS.len() };
    st.age = 0.0;
}

pub fn tick(fe: &mut Fe, dt: f32) {
    fe.game.ig.stats.age += dt;
}

const WHITE: [u8; 4] = [255, 255, 255, 255];
const GREY: [u8; 4] = [180, 180, 180, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

/// `gstatDraw`.
pub fn draw(fe: &mut Fe, canvas: &mut Canvas) {
    let players = fe.game.ig.view.end_stats.clone();
    if players.is_empty() {
        return;
    }
    let Some(f) = fe.resources().font("default.hff").or_else(|| fe.resources().font("HW_EuroseCond_11.hff")) else { return };
    let h = crate::text::height(&f) + 2;
    let txt = |s: &str| crate::text::to_latin1(s);
    let labels: Vec<String> = ROWS.iter().map(|(k, d)| fe.game.ig.strings.get(k).unwrap_or(d).trim().to_string()).collect();
    let label_w = labels.iter().map(|l| crate::text::width(&f, &txt(l))).max().unwrap_or(100);
    let n = players.len();
    let col = 90;
    let (cw, ch) = (canvas.width as i32, canvas.height as i32);
    let width = (label_w + 30 + col * n as i32).min(cw - 20);
    // The rows (with a gap after each group), the names over them, the bars under them.
    let rows_h = h * ROWS.len() as i32 + 4 * (h / 2);
    let bars_h = (h + 4) * n as i32;
    let height = 3 * h + rows_h + h + bars_h + 3 * h;
    let x0 = (cw - width) / 2;
    let y0 = ((ch - height) / 2).max(4);
    let panel = Rect::new(x0 - 10, y0 - 10, x0 + width + 10, y0 + height + 10);
    canvas.blend_rect(panel, [0, 0, 0, 200]);
    canvas.outline_rect(panel, [90, 110, 160, 255]);
    let ord = order(&players);
    // The players' names over their columns.
    let col_x = |i: usize| x0 + label_w + 30 + col * i as i32;
    for (i, &p) in ord.iter().enumerate() {
        let pl = &players[p];
        let c = if pl.local {
            GREEN
        } else if pl.ally {
            [0, 120, 255, 255]
        } else {
            [255, 60, 60, 255]
        };
        let name: String = format!("{}. {}", i + 1, pl.name).chars().take(14).collect();
        let w = crate::text::width(&f, &txt(&name));
        crate::text::print(canvas, &f, col_x(i) + col - w - 6, y0, c, &txt(&name), (crate::text::Shadow::None, [0, 0, 0, 255]));
    }
    let mut rows = Vec::new();
    let mut y = y0 + 2 * h;
    let sel = fe.game.ig.stats.selected;
    for (r, label) in labels.iter().enumerate() {
        if matches!(r, 5 | 8 | 11 | 14) {
            y += h / 2;
        }
        let c = if r == sel {
            GREEN
        } else if r % 2 == 1 {
            GREY
        } else {
            WHITE
        };
        let w = crate::text::width(&f, &txt(label));
        crate::text::print(canvas, &f, x0 + label_w - w, y, c, &txt(label), (crate::text::Shadow::None, [0, 0, 0, 255]));
        for (i, &p) in ord.iter().enumerate() {
            let v = value_text(r, players[p].values[r]);
            let vw = crate::text::width(&f, &txt(&v));
            crate::text::print(canvas, &f, col_x(i) + col - vw - 6, y, c, &txt(&v), (crate::text::Shadow::None, [0, 0, 0, 255]));
        }
        rows.push(Rect::new(x0, y, x0 + width, y + h));
        y += h;
    }
    // The bars of the picked row, growing.
    y += h;
    let max = players.iter().map(|p| p.values[sel]).max().unwrap_or(0).max(1);
    let grow = (fe.game.ig.stats.age / 0.5).clamp(0.0, 1.0);
    let bar_x = x0 + label_w + 30;
    let bar_w = width - label_w - 40;
    crate::text::print(canvas, &f, x0, y, GREEN, &txt(&labels[sel]), (crate::text::Shadow::None, [0, 0, 0, 255]));
    for (i, &p) in ord.iter().enumerate() {
        let pl = &players[p];
        let by = y + i as i32 * (h + 4);
        let name: String = format!("{}. {}", i + 1, pl.name).chars().take(20).collect();
        let w = crate::text::width(&f, &txt(&name));
        crate::text::print(canvas, &f, bar_x - w - 8, by, WHITE, &txt(&name), (crate::text::Shadow::None, [0, 0, 0, 255]));
        let len = ((pl.values[sel].max(0) as f32 / max as f32) * grow * bar_w as f32) as i32;
        if len > 0 {
            canvas.fill_rect(Rect::new(bar_x, by + 1, bar_x + len, by + h - 1), pl.colour);
        }
        canvas.outline_rect(Rect::new(bar_x, by, bar_x + bar_w, by + h), [90, 110, 160, 255]);
    }
    // Done.
    let label = fe.game.ig.strings.get("strDone").unwrap_or("DONE").trim().to_string();
    let bw = crate::text::width(&f, &txt(&label)) + 40;
    let by = y0 + height - 2 * h;
    let done = Rect::new(x0 + (width - bw) / 2, by, x0 + (width + bw) / 2, by + h + 8);
    let (mx, my) = fe.mouse_pos();
    let fill = if done.contains(mx, my) { [70, 90, 140, 255] } else { [40, 50, 80, 255] };
    canvas.fill_rect(done, fill);
    canvas.outline_rect(done, [140, 160, 210, 255]);
    let lw = crate::text::width(&f, &txt(&label));
    crate::text::print(canvas, &f, done.x0 + (done.width() - lw) / 2, done.y0 + 4, WHITE, &txt(&label), (crate::text::Shadow::None, [0, 0, 0, 255]));
    let st = &mut fe.game.ig.stats;
    st.rows = rows;
    st.done = Some(done);
}
