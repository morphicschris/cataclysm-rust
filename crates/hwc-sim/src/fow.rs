//! Fog of war (Cataclysm's `fow*` grid, 0x479d70..0x47c280, and the ship viewers in
//! `UnivUpdate.c`): a per-player grid of 10 km cells over the world, each counting the ships
//! of that player that see it and remembering whether it was ever seen. Every ship except
//! components and swarmers is a viewer covering a cube of cells around it, `vision range /
//! cell size + 1` cells in each direction. Moving into a new cell updates the counts and, when
//! the new cube holds unexplored cells, the local player hears about the first enemy (or
//! crystal) found in one of them (at most every 10 seconds).
//!
//! As in the original only the local player gets a grid (`levelFogOfWarCreate`); other players
//! see everything that is not cloaked.

use hwc_math::fpu::{ftol_i32, ld, st};
use hwc_math::Vector;

use crate::object::ShipId;
use crate::statics::{ShipClass, ShipType};
use crate::universe::{SpecialEvent, Universe};

/// Cell size (0x875634).
pub const CELL_SIZE: f32 = 10_000.0;
/// Least time between two "enemy discovered" messages (seconds of real time).
const DISCOVERY_INTERVAL: f32 = 10.0;

/// One cell: viewers and explored flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Cell {
    pub viewers: i32,
    pub explored: bool,
}

/// A fog-of-war grid (`FOWGrid`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FowGrid {
    /// Cells along x, y, z.
    pub dims: [i32; 3],
    /// Lowest corner.
    pub origin: Vector,
    pub cell_size: f32,
    pub cells: Vec<Cell>,
}

/// The value `fowCellGetViewerCount` returns for a cell outside the grid.
pub const OUTSIDE: i32 = -0x80000;

impl FowGrid {
    /// `fowCreate` (0x479f10) over the box `a`..`b`.
    pub fn new(a: Vector, b: Vector, cell_size: f32) -> Self {
        let lo = Vector::new(if b.x <= a.x { b.x } else { a.x }, if b.y <= a.y { b.y } else { a.y }, if b.z <= a.z { b.z } else { a.z });
        let hi = Vector::new(if b.x <= a.x { a.x } else { b.x }, if b.y <= a.y { a.y } else { b.y }, if b.z <= a.z { a.z } else { b.z });
        let span = |h: f32, l: f32| st(ld(h) - ld(l));
        let dims = [
            ftol_i32(ld(span(hi.x, lo.x)) / ld(cell_size)),
            ftol_i32(ld(span(hi.y, lo.y)) / ld(cell_size)),
            ftol_i32((ld(hi.z) - ld(lo.z)) / ld(cell_size)),
        ];
        let n = (dims[0].max(0) * dims[1].max(0) * dims[2].max(0)) as usize;
        FowGrid { dims, origin: lo, cell_size, cells: vec![Cell::default(); n] }
    }

    fn index(&self, c: [i32; 3]) -> Option<usize> {
        if (0..3).all(|i| c[i] >= 0 && c[i] < self.dims[i]) {
            Some(((self.dims[1] * c[2] + c[1]) * self.dims[0] + c[0]) as usize)
        } else {
            None
        }
    }

    /// `fowPointToCell`: the cell of a point and the radius in cells of a range.
    pub fn point_to_cell(&self, p: Vector, range: f32) -> ([i32; 3], i32) {
        let inv = 1.0 / ld(self.cell_size);
        let c = [
            ftol_i32((ld(p.x) - ld(self.origin.x)) * inv),
            ftol_i32((ld(p.y) - ld(self.origin.y)) * inv),
            ftol_i32((ld(p.z) - ld(self.origin.z)) * inv),
        ];
        (c, ftol_i32(inv * ld(range)) + 1)
    }

    fn cube(&self, c: [i32; 3], r: i32) -> impl Iterator<Item = [i32; 3]> + '_ {
        let lo = |i: usize| (c[i] - r).max(0);
        let hi = |i: usize| (c[i] + r).min(self.dims[i] - 1);
        let (x0, x1, y0, y1, z0, z1) = (lo(0), hi(0), lo(1), hi(1), lo(2), hi(2));
        (z0..=z1).flat_map(move |z| (y0..=y1).flat_map(move |y| (x0..=x1).map(move |x| [x, y, z])))
    }

    /// `fowAddViewerCells`.
    pub fn add_viewer_cells(&mut self, c: [i32; 3], r: i32) {
        let cells: Vec<[i32; 3]> = self.cube(c, r).collect();
        for k in cells {
            if let Some(i) = self.index(k) {
                self.cells[i].viewers += 1;
                self.cells[i].explored = true;
            }
        }
    }

    /// `fowRemoveViewerCells` (counts never go below zero).
    pub fn remove_viewer_cells(&mut self, c: [i32; 3], r: i32) {
        let cells: Vec<[i32; 3]> = self.cube(c, r).collect();
        for k in cells {
            if let Some(i) = self.index(k) {
                if self.cells[i].viewers > 0 {
                    self.cells[i].viewers -= 1;
                }
            }
        }
    }

    /// `fowCellsHaveUnexplored`.
    pub fn cells_have_unexplored(&self, c: [i32; 3], r: i32) -> bool {
        self.cube(c, r).any(|k| self.index(k).is_some_and(|i| !self.cells[i].explored))
    }

    /// `fowCellGetViewerCount` (`OUTSIDE` outside the grid).
    pub fn viewer_count(&self, c: [i32; 3]) -> i32 {
        self.index(c).map_or(OUTSIDE, |i| self.cells[i].viewers)
    }

    /// `fowCellIsExplored` (outside the grid counts as explored).
    pub fn is_explored(&self, c: [i32; 3]) -> bool {
        self.index(c).is_none_or(|i| self.cells[i].explored)
    }

    /// `fowPointGetViewerCount`.
    pub fn point_viewer_count(&self, p: Vector) -> i32 {
        self.viewer_count(self.point_to_cell(p, 0.0).0)
    }

    /// `fowPointIsExplored`.
    pub fn point_is_explored(&self, p: Vector) -> bool {
        self.is_explored(self.point_to_cell(p, 0.0).0)
    }
}

/// The fog state of the universe.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct FogOfWar {
    /// One grid per player (only the local player's exists).
    pub grids: Vec<Option<FowGrid>>,
    /// When the last discovery message was given (real time; the simulation uses game time).
    pub last_discovery: f32,
}

/// `fowShipProvidesVision`: everything but components and swarmers.
pub fn provides_vision(u: &Universe, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    let component = u.statics.ship(s.ship_type).is_some_and(|st| st.ship_class == Some(ShipClass::Component));
    !component && !matches!(s.ship_type, ShipType::sSwarmer | ShipType::bSwarmer)
}

/// `levelFogOfWarCreate` (0x49ef20): the local player's grid over the world, and every ship of
/// that player becomes a viewer.
pub fn create_for_local_player(u: &mut Universe, local: usize) {
    let b = u.world_bounds;
    let mut grids = vec![None; u.players.len()];
    if local < grids.len() {
        grids[local] = Some(FowGrid::new(Vector::new(-b.x, -b.y, -b.z), b, CELL_SIZE));
    }
    u.fog.grids = grids;
    let ships: Vec<ShipId> = u.ship_list.iter().copied().filter(|&id| u.ships.get(id).is_some_and(|s| s.player == local)).collect();
    for id in ships {
        add_viewer(u, id);
    }
}

fn grid_of(u: &Universe, player: usize) -> Option<&FowGrid> {
    u.fog.grids.get(player).and_then(Option::as_ref)
}

/// `univShipFowAddViewer` (0x53d2e0).
pub fn add_viewer(u: &mut Universe, id: ShipId) {
    let Some(player) = u.ships.get(id).map(|s| s.player) else { return };
    if grid_of(u, player).is_none() || !provides_vision(u, id) {
        return;
    }
    let r = st(crate::visibility::vision_range(u, id));
    let p = u.ships.ship(id).posinfo.position;
    let g = u.fog.grids[player].as_mut().expect("grid");
    let (c, cells) = g.point_to_cell(p, r);
    g.add_viewer_cells(c, cells);
}

/// `univShipFowRemoveViewer` (0x53d350).
pub fn remove_viewer(u: &mut Universe, id: ShipId) {
    let Some(player) = u.ships.get(id).map(|s| s.player) else { return };
    if grid_of(u, player).is_none() || !provides_vision(u, id) {
        return;
    }
    let r = st(crate::visibility::vision_range(u, id));
    let p = u.ships.ship(id).posinfo.position;
    let g = u.fog.grids[player].as_mut().expect("grid");
    let (c, cells) = g.point_to_cell(p, r);
    g.remove_viewer_cells(c, cells);
}

/// `univShipFowMoveViewer` (0x53fbb0): a viewer moved from `old` to `new`.
pub fn move_viewer(u: &mut Universe, id: ShipId, old: Vector, new: Vector) {
    let Some(player) = u.ships.get(id).map(|s| s.player) else { return };
    if grid_of(u, player).is_none() || !provides_vision(u, id) {
        return;
    }
    let r = st(crate::visibility::vision_range(u, id));
    let g = grid_of(u, player).expect("grid");
    let (oc, cells) = g.point_to_cell(old, r);
    let (nc, _) = g.point_to_cell(new, 0.0);
    if oc == nc {
        return;
    }
    u.fog.grids[player].as_mut().expect("grid").remove_viewer_cells(oc, cells);
    let now = u.total_time_elapsed;
    let g = grid_of(u, player).expect("grid");
    if g.cells_have_unexplored(nc, cells) && ld(u.fog.last_discovery) + ld(DISCOVERY_INTERVAL) < ld(now) {
        // The first enemy ship (or crystal) in an unexplored cell of the new cube.
        let mut found = None;
        for &other in &u.ship_list {
            let Some(o) = u.ships.get(other) else { continue };
            if u.players.get(o.player).is_some_and(|p| p.is_ally(player)) {
                continue;
            }
            let crystal = matches!(o.ship_type, ShipType::dCrystal0 | ShipType::dCrystal1);
            if crystal {
                continue;
            }
            let (oc2, _) = g.point_to_cell(o.posinfo.position, 0.0);
            let within = (0..3).all(|i| oc2[i] <= nc[i] + cells && nc[i] - cells <= oc2[i]);
            if within && !g.is_explored(oc2) {
                found = Some(other);
                break;
            }
        }
        if let Some(other) = found {
            u.fog.last_discovery = now;
            let o = u.ships.ship(other);
            let skip_mission7 = u.single_player && u.base.options.mission == 7 && o.ship_type == ShipType::dPrisonShip;
            if !skip_mission7 && crate::visibility::visible_to_player(u, other, player) {
                u.special_events.push(SpecialEvent::EnemyDiscovered { by: id, ship: other });
            }
        }
    }
    u.fog.grids[player].as_mut().expect("grid").add_viewer_cells(nc, cells);
}

/// `rmTechSetAdd` / `rmTechSetSetMode` around a change of a technology set: for the keys that
/// may change vision ranges (technology or mode -1, 2 and the sensors technology 0x1c) every
/// viewer leaves the grid before `change` and comes back after it, with its new range.
pub fn with_vision_refresh(u: &mut Universe, key: i32, change: impl FnOnce(&mut Universe)) {
    let refresh = matches!(key, -1 | 2 | 0x1c) && u.fog.grids.iter().any(Option::is_some);
    let ships = if refresh { u.ship_list.clone() } else { Vec::new() };
    for &id in &ships {
        remove_viewer(u, id);
    }
    change(u);
    for &id in &ships {
        add_viewer(u, id);
    }
}

/// `rmTechSetAdd` on a player's technologies, with the fog refresh it makes.
pub fn tech_add(u: &mut Universe, player: usize, tech: usize) {
    with_vision_refresh(u, tech as i32, |u| {
        if let Some(p) = u.base.players.get_mut(player) {
            p.tech.add(tech);
        }
    });
}

/// `fowShipVisibleToPlayer` (0x479d90): whether `player` sees the ship through its fog (cloaked
/// ships only when revealed to that player).
pub fn ship_visible_to_player(u: &Universe, id: ShipId, player: usize) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    let cloak_ok = |u: &Universe| !crate::cloak::is_cloaked(s) || crate::visibility::visible_to_player(u, id, player);
    match grid_of(u, player) {
        None => {
            let ai = u.base.players.get(player).is_some_and(|p| p.ai);
            !ai || cloak_ok(u)
        }
        Some(g) => {
            if !u.ship_list.contains(&id) {
                return false;
            }
            let count = g.point_viewer_count(s.posinfo.position);
            let mut v = (count > 0 && cloak_ok(u)) || count == OUTSIDE;
            if s.flags & crate::object::sof::HIDE != 0 {
                v = false;
            }
            v
        }
    }
}
