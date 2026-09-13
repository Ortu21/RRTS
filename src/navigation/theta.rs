//! Navigazione — Theta* any-angle + LOS + scratch arena.
//!
//! Opera su `grid::NavGrid` via `impl NavGrid` esterno (stesso crate).
//! Nessuno stato condiviso oltre la griglia: deterministico bit-identico.

use super::congestion::CONGESTION_WEIGHT;
use super::grid::{CELL_SIZE, NavGrid, UNIT_CLEARANCE};
use bevy::prelude::*;
use std::{
    cell::RefCell,
    cmp::{Ordering, Reverse},
    collections::BinaryHeap,
};

type OpenNode = Reverse<(FOrd, FOrd, usize)>;

#[derive(Default)]
pub(crate) struct PathScratch {
    pub(crate) cost: Vec<f32>,
    pub(crate) parent: Vec<usize>,
    pub(crate) open: BinaryHeap<OpenNode>,
    pub(crate) touched: Vec<usize>,
}

impl PathScratch {
    pub(crate) fn prepare(&mut self, len: usize) {
        self.open.clear();
        if self.cost.len() != len {
            self.cost = vec![f32::INFINITY; len];
            self.parent = vec![usize::MAX; len];
            self.touched.clear();
            return;
        }
        while let Some(index) = self.touched.pop() {
            self.cost[index] = f32::INFINITY;
            self.parent[index] = usize::MAX;
        }
    }

    pub(crate) fn set_node(&mut self, index: usize, cost: f32, parent: usize) {
        if self.cost[index].is_infinite() {
            self.touched.push(index);
        }
        self.cost[index] = cost;
        self.parent[index] = parent;
    }
}

thread_local! {
    /// One reusable search arena per Bevy compute worker (and per serial
    /// caller). Searches on a worker are sequential, so no locking is needed.
    static PATH_SCRATCH: RefCell<PathScratch> = RefCell::new(PathScratch::default());
}

/// Total order over finite path costs for the Theta* heap. Costs never go
/// NaN (sums of finite distances), so `total_cmp` is a valid ordering and
/// repeated runs agree exactly.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct FOrd(pub(crate) f32);

impl Eq for FOrd {}

impl PartialOrd for FOrd {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FOrd {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl NavGrid {
    /// Legacy scout-margin routing; gameplay uses [`Self::find_path_for`].
    /// Kept for tests and comparable baselines.
    #[allow(dead_code)]
    pub fn find_path(&self, start: Vec3, goal: Vec3) -> Option<Vec<Vec3>> {
        self.find_path_inner(None, start, goal)
    }

    /// Theta* with live congestion costs: entering a crowded cell costs
    /// extra, so fresh routes flow around battles instead of through them.
    /// Only new plans see congestion (no mid-route replanning, no churn);
    /// density comes from the pre-movement spatial index, so results stay
    /// deterministic across repeats.
    /// Legacy scout-margin variant; gameplay uses
    /// [`Self::find_path_congested_for`]. Kept for tests and baselines.
    #[allow(dead_code)]
    pub fn find_path_congested(
        &self,
        spatial: &crate::spatial::SpatialGrid,
        start: Vec3,
        goal: Vec3,
    ) -> Option<Vec<Vec3>> {
        self.find_path_inner(Some(spatial), start, goal)
    }

    /// Body-aware routing for a hull `radius`. Endpoints must fit the hull
    /// ([`Self::has_clearance_for`]); otherwise `None` is returned explicitly
    /// instead of an unexecutable 0.8-margin path that `constrain_motion`
    /// would later reject (e.g. Commander x=299 on an empty map passes the
    /// scout check but fails `segment_clear_for` with margin 1.7).
    /// Small hulls delegate to the legacy fast path bit-for-bit; larger
    /// hulls search the same grid but only through cells whose centers fit
    /// the hull, with shortcuts validated by both grid LOS and exact swept
    /// hull clearance. Deterministic like [`Self::find_path`].
    pub fn find_path_for(&self, start: Vec3, goal: Vec3, radius: f32) -> Option<Vec<Vec3>> {
        self.find_path_inner_for(None, start, goal, radius)
    }

    pub fn find_path_congested_for(
        &self,
        spatial: &crate::spatial::SpatialGrid,
        start: Vec3,
        goal: Vec3,
        radius: f32,
    ) -> Option<Vec<Vec3>> {
        self.find_path_inner_for(Some(spatial), start, goal, radius)
    }

    fn find_path_inner_for(
        &self,
        spatial: Option<&crate::spatial::SpatialGrid>,
        start: Vec3,
        goal: Vec3,
        radius: f32,
    ) -> Option<Vec<Vec3>> {
        if !start.is_finite() || !goal.is_finite() {
            return None;
        }
        // Explicit body fit: unreachable-for-this-hull is None, never a
        // scout-clear path the body cannot execute.
        if !self.has_clearance_for(start, radius) || !self.has_clearance_for(goal, radius) {
            return None;
        }
        // Small hulls keep the exact legacy behaviour (density + budget).
        if Self::clearance_for(radius) <= UNIT_CLEARANCE + 1e-6 {
            return self.find_path_inner(spatial, start, goal);
        }
        let start_cell = self.cell(start)?;
        let goal_cell = self.cell(goal)?;
        if !self.walkable[start_cell] || !self.walkable[goal_cell] {
            return None;
        }
        // Start/goal cells are entered from fitted endpoints even when their
        // centers are tight; every other cell must fit the hull center.
        let center_fits = |index: usize| {
            index == start_cell
                || index == goal_cell
                || self.has_clearance_for(self.cell_center(index), radius)
        };
        if !center_fits(start_cell) || !center_fits(goal_cell) {
            // Endpoints fit but their cells are sealed for this hull (e.g.
            // start inside a 0.8-only pocket): no honest route exists.
            // Fall through to search anyway? No — return None explicitly to
            // avoid a stuck order. Callers repair via clear_point_for.
            // (We still allow the cells themselves; only intermediate cells
            // are filtered, so this branch is unreachable. Kept for clarity.)
        }
        PATH_SCRATCH.with(|slot| {
            let mut scratch = slot.borrow_mut();
            scratch.prepare(self.walkable.len());
            let center = |index: usize| self.cell_center(index).xz();
            let center3 = |index: usize| self.cell_center(index);
            let segment = |a: usize, b: usize| center(a).distance(center(b));
            let heuristic = |index: usize| center(index).distance(center(goal_cell));
            // Body-aware LOS: grid walkability (cheap reject) + exact swept hull
            // (accept). Both deterministic over grid + endpoints.
            let los_for = |a: usize, b: usize| {
                self.has_line_of_sight(center(a), center(b))
                    && self.segment_clear_for(center3(a), center3(b), radius)
            };
            scratch.set_node(start_cell, 0.0, start_cell);
            scratch.open.push(Reverse((
                FOrd(heuristic(start_cell)),
                FOrd(0.0),
                start_cell,
            )));
            while let Some(Reverse((_, queued, current))) = scratch.open.pop() {
                if queued.0 != scratch.cost[current] {
                    continue;
                }
                if current == goal_cell {
                    let mut cells = vec![current];
                    let mut next = current;
                    while next != start_cell {
                        next = scratch.parent[next];
                        // Corrupt parent chain guard (should be unreachable).
                        if next == usize::MAX {
                            return None;
                        }
                        cells.push(next);
                    }
                    cells.reverse();
                    let mut path = Vec::new();
                    for (index, cell) in cells.iter().enumerate() {
                        if index > 0 && index + 1 < cells.len() {
                            let incoming = *cell as isize - cells[index - 1] as isize;
                            let outgoing = cells[index + 1] as isize - *cell as isize;
                            if incoming == outgoing {
                                continue;
                            }
                        }
                        path.push(self.cell_center(*cell));
                    }
                    if path
                        .last()
                        .is_none_or(|last| last.distance_squared(goal) > 0.0001)
                    {
                        path.push(goal);
                    }
                    self.anchor_path_ends_for(start, goal, &mut path, radius);
                    // Final honesty check: every leg must execute under
                    // constrain_motion for this hull. Otherwise report
                    // unreachable instead of issuing a stuck order.
                    let mut prev = start;
                    for point in &path {
                        if !self.segment_clear_for(prev, *point, radius) {
                            return None;
                        }
                        prev = *point;
                    }
                    // Trailing goal leg already checked above when path ends with
                    // goal; when path is empty (start==goal cell) the loop covers
                    // start->goal via the single pushed goal.
                    return Some(path);
                }
                let x = (current % self.width) as isize;
                let z = (current / self.width) as isize;
                for (dx, dz) in [
                    (-1, 0),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (-1, 1),
                    (1, -1),
                    (1, 1),
                ] {
                    let nx = x + dx;
                    let nz = z + dz;
                    if nx < 0 || nz < 0 || nx >= self.width as isize || nz >= self.width as isize {
                        continue;
                    }
                    let next = nz as usize * self.width + nx as usize;
                    if !self.walkable[next] {
                        continue;
                    }
                    if next != goal_cell && !self.has_clearance_for(self.cell_center(next), radius)
                    {
                        continue;
                    }
                    let diagonal = dx != 0 && dz != 0;
                    if diagonal
                        && (!self.walkable[z as usize * self.width + nx as usize]
                            || !self.walkable[nz as usize * self.width + x as usize])
                    {
                        continue;
                    }
                    // Diagonal corners must also fit the hull, not just be
                    // walkable at 0.8.
                    if diagonal {
                        let side_a = z as usize * self.width + nx as usize;
                        let side_b = nz as usize * self.width + x as usize;
                        if side_a != start_cell
                            && side_a != goal_cell
                            && !self.has_clearance_for(self.cell_center(side_a), radius)
                        {
                            continue;
                        }
                        if side_b != start_cell
                            && side_b != goal_cell
                            && !self.has_clearance_for(self.cell_center(side_b), radius)
                        {
                            continue;
                        }
                    }
                    let via = scratch.parent[current];
                    let (next_parent, next_cost) = if los_for(via, next) {
                        (via, scratch.cost[via] + segment(via, next))
                    } else {
                        (current, scratch.cost[current] + segment(current, next))
                    };
                    let next_cost = next_cost
                        + spatial.map_or(0.0, |grid| {
                            CONGESTION_WEIGHT * grid.bucket_count(center(next)) as f32
                        });
                    if next_cost < scratch.cost[next] {
                        scratch.set_node(next, next_cost, next_parent);
                        scratch.open.push(Reverse((
                            FOrd(next_cost + heuristic(next)),
                            FOrd(next_cost),
                            next,
                        )));
                    }
                }
            }
            None
        })
    }

    fn find_path_inner(
        &self,
        spatial: Option<&crate::spatial::SpatialGrid>,
        start: Vec3,
        goal: Vec3,
    ) -> Option<Vec<Vec3>> {
        if !self.has_clearance(start) || !self.has_clearance(goal) {
            return None;
        }
        let start_cell = self.cell(start)?;
        let goal_cell = self.cell(goal)?;
        if !self.walkable[start_cell] || !self.walkable[goal_cell] {
            return None;
        }
        // Theta*: any-angle search over the same grid. From each expansion
        // the path shortcuts through the parent cell whenever line of
        // sight holds, so open-field routes come out straight instead of
        // staircased. Costs are exact world distances; the heap orders by
        // total order so repeated runs agree bit-for-bit.
        PATH_SCRATCH.with(|slot| {
            let mut scratch = slot.borrow_mut();
            scratch.prepare(self.walkable.len());
            let center = |index: usize| self.cell_center(index).xz();
            let segment = |a: usize, b: usize| center(a).distance(center(b));
            let heuristic = |index: usize| center(index).distance(center(goal_cell));
            scratch.set_node(start_cell, 0.0, start_cell);
            scratch.open.push(Reverse((
                FOrd(heuristic(start_cell)),
                FOrd(0.0),
                start_cell,
            )));
            while let Some(Reverse((_, queued, current))) = scratch.open.pop() {
                if queued.0 != scratch.cost[current] {
                    continue;
                }
                if current == goal_cell {
                    let mut cells = vec![current];
                    let mut next = current;
                    while next != start_cell {
                        next = scratch.parent[next];
                        cells.push(next);
                    }
                    cells.reverse();
                    let mut path = Vec::new();
                    for (index, cell) in cells.iter().enumerate() {
                        if index > 0 && index + 1 < cells.len() {
                            let incoming = *cell as isize - cells[index - 1] as isize;
                            let outgoing = cells[index + 1] as isize - *cell as isize;
                            if incoming == outgoing {
                                continue;
                            }
                        }
                        path.push(self.cell_center(*cell));
                    }
                    if path
                        .last()
                        .is_none_or(|last| last.distance_squared(goal) > 0.0001)
                    {
                        path.push(goal);
                    }
                    self.anchor_path_ends(start, goal, &mut path);
                    return Some(path);
                }
                let x = (current % self.width) as isize;
                let z = (current / self.width) as isize;
                for (dx, dz) in [
                    (-1, 0),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (-1, 1),
                    (1, -1),
                    (1, 1),
                ] {
                    let nx = x + dx;
                    let nz = z + dz;
                    if nx < 0 || nz < 0 || nx >= self.width as isize || nz >= self.width as isize {
                        continue;
                    }
                    let next = nz as usize * self.width + nx as usize;
                    if !self.walkable[next] {
                        continue;
                    }
                    let diagonal = dx != 0 && dz != 0;
                    if diagonal
                        && (!self.walkable[z as usize * self.width + nx as usize]
                            || !self.walkable[nz as usize * self.width + x as usize])
                    {
                        continue;
                    }
                    let via = scratch.parent[current];
                    let (next_parent, next_cost) =
                        if self.has_line_of_sight(center(via), center(next)) {
                            (via, scratch.cost[via] + segment(via, next))
                        } else {
                            (current, scratch.cost[current] + segment(current, next))
                        };
                    let next_cost = next_cost
                        + spatial.map_or(0.0, |grid| {
                            CONGESTION_WEIGHT * grid.bucket_count(center(next)) as f32
                        });
                    if next_cost < scratch.cost[next] {
                        scratch.set_node(next, next_cost, next_parent);
                        scratch.open.push(Reverse((
                            FOrd(next_cost + heuristic(next)),
                            FOrd(next_cost),
                            next,
                        )));
                    }
                }
            }
            None
        })
    }

    /// Anchor a raw cell-center path to the caller's continuous endpoints.
    /// General for every locomotion kind (tanks and tank-like bipeds share
    /// the same steering): Theta* routes cell centers, but a unit re-routed
    /// mid-march is never exactly on its start-cell center, and the goal is
    /// rarely exactly on its goal-cell center. Steering to those quantized
    /// centers first produces the visible "micro passo indietro" on lateral
    /// re-clicks (plus an overshoot-and-return kink at arrival), with the
    /// hull flipping the wrong way for a few frames.
    /// Trims at most the quantization slop at both ends, and only when the
    /// direct shortcut keeps full static clearance: the leading center goes
    /// when `start -> path[1]` holds line of sight, the trailing center
    /// when the approach `-> goal` does. Anything farther than one cell is
    /// real routing (crowd detours, maze turns) and is always preserved, so
    /// congestion avoidance and obstacle clearance are untouched.
    /// Deterministic: pure over grid + endpoints, no frame state.
    fn anchor_path_ends(&self, start: Vec3, goal: Vec3, path: &mut Vec<Vec3>) {
        self.anchor_path_ends_for(start, goal, path, 0.5);
    }

    /// Body-aware endpoint anchoring: shortcuts must keep hull clearance
    /// ([`Self::segment_clear_for`]), not just the scout margin. Small hulls
    /// keep the exact legacy behaviour; larger hulls trim only when the hull
    /// fits the shortcut.
    fn anchor_path_ends_for(&self, start: Vec3, goal: Vec3, path: &mut Vec<Vec3>, radius: f32) {
        if Self::clearance_for(radius) <= UNIT_CLEARANCE + 1e-6 {
            while path.len() > 1
                && path[0].xz().distance(start.xz()) <= CELL_SIZE
                && self.has_line_of_sight(start.xz(), path[1].xz())
                && self.segment_clear(start, path[1])
            {
                path.remove(0);
            }
            while path.len() >= 2 && path[path.len() - 1] == goal {
                let center = path[path.len() - 2];
                if center.xz().distance(goal.xz()) > CELL_SIZE {
                    break;
                }
                let anchor = if path.len() >= 3 {
                    path[path.len() - 3]
                } else {
                    start
                };
                if self.has_line_of_sight(anchor.xz(), goal.xz())
                    && self.segment_clear(anchor, goal)
                {
                    path.remove(path.len() - 2);
                } else {
                    break;
                }
            }
            return;
        }
        while path.len() > 1
            && path[0].xz().distance(start.xz()) <= CELL_SIZE
            && self.has_line_of_sight(start.xz(), path[1].xz())
            && self.segment_clear_for(start, path[1], radius)
        {
            path.remove(0);
        }
        while path.len() >= 2 && path[path.len() - 1] == goal {
            let center = path[path.len() - 2];
            if center.xz().distance(goal.xz()) > CELL_SIZE {
                break;
            }
            let anchor = if path.len() >= 3 {
                path[path.len() - 3]
            } else {
                start
            };
            if self.has_line_of_sight(anchor.xz(), goal.xz())
                && self.segment_clear_for(anchor, goal, radius)
            {
                path.remove(path.len() - 2);
            } else {
                break;
            }
        }
    }

    /// Line of sight between two ground points as an exact grid traversal
    /// (Amanatides & Woo) over walkability: O(cells crossed) with O(1)
    /// lookups, instead of rect scans per sample. Conservative by one design
    /// choice: margin-band cells count as blocked even where point clearance
    /// alone would pass, so shortcuts keep extra distance and stay valid.
    pub fn has_line_of_sight(&self, from: Vec2, to: Vec2) -> bool {
        let width = self.width as isize;
        let mut x = ((from.x + self.half_size) / self.cell_size).floor() as isize;
        let mut z = ((from.y + self.half_size) / self.cell_size).floor() as isize;
        let end_x = ((to.x + self.half_size) / self.cell_size).floor() as isize;
        let end_z = ((to.y + self.half_size) / self.cell_size).floor() as isize;
        let step_x = (to.x > from.x) as isize - (to.x < from.x) as isize;
        let step_z = (to.y > from.y) as isize - (to.y < from.y) as isize;
        let mut t_max_x = if step_x == 0 {
            f32::INFINITY
        } else {
            let edge = if step_x > 0 {
                (x + 1) as f32 * self.cell_size - self.half_size
            } else {
                x as f32 * self.cell_size - self.half_size
            };
            (edge - from.x) / (to.x - from.x)
        };
        let mut t_max_z = if step_z == 0 {
            f32::INFINITY
        } else {
            let edge = if step_z > 0 {
                (z + 1) as f32 * self.cell_size - self.half_size
            } else {
                z as f32 * self.cell_size - self.half_size
            };
            (edge - from.y) / (to.y - from.y)
        };
        let t_delta_x = if step_x == 0 {
            f32::INFINITY
        } else {
            self.cell_size / (to.x - from.x).abs()
        };
        let t_delta_z = if step_z == 0 {
            f32::INFINITY
        } else {
            self.cell_size / (to.y - from.y).abs()
        };
        loop {
            if x < 0 || z < 0 || x >= width || z >= width {
                return false;
            }
            if !self.walkable[z as usize * self.width + x as usize] {
                return false;
            }
            if x == end_x && z == end_z {
                return true;
            }
            if t_max_x < t_max_z {
                x += step_x;
                t_max_x += t_delta_x;
            } else {
                z += step_z;
                t_max_z += t_delta_z;
            }
        }
    }
}
