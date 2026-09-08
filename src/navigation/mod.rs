use crate::{formation::formation_slots, movement::MoveTarget};
use bevy::{
    prelude::*,
    tasks::{ComputeTaskPool, ParallelSlice},
};
use std::{
    cell::RefCell,
    cmp::{Ordering, Reverse},
    collections::{BinaryHeap, HashSet},
    time::Instant,
};

type OpenNode = Reverse<(FOrd, FOrd, usize)>;

#[derive(Default)]
struct PathScratch {
    cost: Vec<f32>,
    parent: Vec<usize>,
    open: BinaryHeap<OpenNode>,
    touched: Vec<usize>,
}

impl PathScratch {
    fn prepare(&mut self, len: usize) {
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

    fn set_node(&mut self, index: usize, cost: f32, parent: usize) {
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

pub const HALF_SIZE: f32 = 300.0;
pub const CELL_SIZE: f32 = 2.5;
pub const UNIT_CLEARANCE: f32 = 0.8;
pub const PATHS_PER_FRAME: usize = 128;
/// Below this many pending requests the planner stays serial: spawning
/// tasks costs more than the search itself on tiny batches.
pub const SERIAL_PATH_THRESHOLD: usize = 16;
/// Extra route cost per crowded body in the destination cell: routes spread
/// around live crowds instead of piling through them. Tunable; validated by
/// keeping zero path failures on the stress workloads.
pub const CONGESTION_WEIGHT: f32 = 0.5;

/// Fixed map seed: obstacle layout is identical every run, on every
/// platform, so benchmark checksums stay comparable.
pub const MAP_SEED: u64 = 20260907;
/// Target obstacle count for the default map. Scales with area to keep
/// gameplay density constant (44 on 400m -> 100 on 600m).
pub const MAP_OBSTACLES: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Obstacle {
    pub center: Vec2,
    pub half_size: Vec2,
}

/// Deterministic splitmix64: no external RNG dependency, identical output
/// on every platform for a given seed.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * ((self.next() >> 11) as f32 / ((1u64 << 53) as f32))
    }
}

fn rects_overlap(a: Obstacle, b: Obstacle, lane: f32) -> bool {
    (a.center.x - b.center.x).abs() < a.half_size.x + b.half_size.x + lane
        && (a.center.y - b.center.y).abs() < a.half_size.y + b.half_size.y + lane
}

/// Cell walkability for a candidate obstacle set, shared by grid building
/// and connectivity checks so both agree exactly. Blocks every cell touched
/// by an obstacle expanded by the unit radius, so routes through the
/// remaining cells keep clearance even at diagonal turns.
fn build_walkable(
    obstacles: &[Obstacle],
    half_size: f32,
    cell_size: f32,
    width: usize,
) -> Vec<bool> {
    (0..width * width)
        .map(|index| cell_walkable(obstacles, half_size, cell_size, width, index))
        .collect()
}

/// Single-cell version of [`build_walkable`]: pure over the obstacle set, so
/// placement probes can recompute just the neighborhood of one extra
/// obstacle instead of rebuilding the whole grid.
fn cell_walkable(
    obstacles: &[Obstacle],
    half_size: f32,
    cell_size: f32,
    width: usize,
    index: usize,
) -> bool {
    let margin = UNIT_CLEARANCE + cell_size * 0.5;
    let center = Vec3::new(
        (index % width) as f32 * cell_size - half_size + cell_size * 0.5,
        0.0,
        (index / width) as f32 * cell_size - half_size + cell_size * 0.5,
    )
    .xz();
    obstacles.iter().all(|obstacle| {
        let delta = (center - obstacle.center).abs();
        delta.x > obstacle.half_size.x + margin || delta.y > obstacle.half_size.y + margin
    })
}

/// Key gameplay points (fractions of half size) that must stay walkable and
/// mutually reachable: team spawns, attack targets, map arteries. Adding a
/// rect that breaks any of them discards the rect, so generation always
/// terminates with a connected map.
fn key_points(half_size: f32) -> [Vec2; 8] {
    let (a, b, c) = (half_size * 0.55, half_size * 0.35, half_size - 10.0);
    let d = half_size - 40.0;
    [
        Vec2::new(-a, 0.0),
        Vec2::new(a, 0.0),
        Vec2::new(-b, 0.0),
        Vec2::new(b, 0.0),
        Vec2::new(0.0, -c),
        Vec2::new(0.0, c),
        // Playground corner spawns (blu SW, rosso NE): protetti come gli altri.
        Vec2::new(-d, -d),
        Vec2::new(d, d),
    ]
}

fn key_points_connected(walkable: &[bool], width: usize, half_size: f32, cell_size: f32) -> bool {
    let cell_of = |point: Vec2| {
        let x = ((point.x + half_size) / cell_size) as isize;
        let z = ((point.y + half_size) / cell_size) as isize;
        if x < 0 || z < 0 || x >= width as isize || z >= width as isize {
            None
        } else {
            Some(z as usize * width + x as usize)
        }
    };
    let starts: Option<Vec<usize>> = key_points(half_size)
        .iter()
        .map(|point| cell_of(*point).filter(|cell| walkable[*cell]))
        .collect();
    let Some(starts) = starts else {
        return false;
    };
    let mut seen = vec![false; walkable.len()];
    let mut open = vec![starts[0]];
    seen[starts[0]] = true;
    while let Some(current) = open.pop() {
        let (x, z) = (current % width, current / width);
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let (nx, nz) = (x as isize + dx, z as isize + dz);
            if nx < 0 || nz < 0 || nx >= width as isize || nz >= width as isize {
                continue;
            }
            let next = nz as usize * width + nx as usize;
            if walkable[next] && !seen[next] {
                seen[next] = true;
                open.push(next);
            }
        }
    }
    starts.iter().all(|cell| seen[*cell])
}

/// Random-but-deterministic obstacle layout: varied rects with guaranteed
/// lanes between them, capped coverage, and enforced connectivity. Adding
/// rects one by one and keeping only connectivity-preserving ones makes
/// termination certain (the empty set is always connected).
pub fn generate_obstacles(seed: u64, half_size: f32, target_count: usize) -> Vec<Obstacle> {
    let mut rng = SplitMix64(seed);
    let mut obstacles = Vec::new();
    let mut attempts = 0;
    while obstacles.len() < target_count && attempts < target_count * 40 {
        attempts += 1;
        let half = Vec2::new(rng.range(4.0, 16.0), rng.range(4.0, 26.0));
        let bound = half_size - 24.0 - half.max_element();
        if bound <= 0.0 {
            continue;
        }
        let candidate = Obstacle {
            center: Vec2::new(rng.range(-bound, bound), rng.range(-bound, bound)),
            half_size: half,
        };
        if obstacles
            .iter()
            .any(|other| rects_overlap(candidate, *other, 7.0))
        {
            continue;
        }
        let mut trial = obstacles.clone();
        trial.push(candidate);
        let width = (half_size * 2.0 / CELL_SIZE).round() as usize;
        let walkable = build_walkable(&trial, half_size, CELL_SIZE, width);
        let blocked = walkable.iter().filter(|cell| !**cell).count();
        if blocked as f32 / walkable.len() as f32 > 0.15 {
            continue;
        }
        if !key_points_connected(&walkable, width, half_size, CELL_SIZE) {
            continue;
        }
        obstacles = trial;
    }
    obstacles
}

#[derive(Resource, Clone)]
pub struct NavGrid {
    pub obstacles: Vec<Obstacle>,
    half_size: f32,
    cell_size: f32,
    width: usize,
    walkable: Vec<bool>,
}

impl Default for NavGrid {
    fn default() -> Self {
        Self::new(
            HALF_SIZE,
            CELL_SIZE,
            generate_obstacles(MAP_SEED, HALF_SIZE, MAP_OBSTACLES),
        )
    }
}

impl NavGrid {
    /// Called only when a building footprint is added/removed, never per frame.
    pub fn replace_dynamic(&mut self, previous_count: usize, dynamic: &[Obstacle]) {
        self.obstacles
            .truncate(self.obstacles.len() - previous_count);
        self.obstacles.extend_from_slice(dynamic);
        self.walkable = build_walkable(&self.obstacles, self.half_size, self.cell_size, self.width);
    }
    /// Exact swept XZ clearance against expanded obstacle AABBs.
    pub fn segment_clear(&self, from: Vec3, to: Vec3) -> bool {
        if !self.has_clearance(from) || !self.has_clearance(to) {
            return false;
        }
        let delta = to.xz() - from.xz();
        self.obstacles.iter().all(|o| {
            let lo = o.center - o.half_size - Vec2::splat(UNIT_CLEARANCE);
            let hi = o.center + o.half_size + Vec2::splat(UNIT_CLEARANCE);
            let mut enter: f32 = 0.0;
            let mut leave: f32 = 1.0;
            for axis in 0..2 {
                if delta[axis].abs() < 1e-8 {
                    if from.xz()[axis] <= lo[axis] || from.xz()[axis] >= hi[axis] {
                        return true;
                    }
                } else {
                    let a = (lo[axis] - from.xz()[axis]) / delta[axis];
                    let b = (hi[axis] - from.xz()[axis]) / delta[axis];
                    enter = enter.max(a.min(b));
                    leave = leave.min(a.max(b));
                }
            }
            enter >= leave
        })
    }

    pub fn new(half_size: f32, cell_size: f32, obstacles: Vec<Obstacle>) -> Self {
        let width = (half_size * 2.0 / CELL_SIZE).round() as usize;
        let walkable = build_walkable(&obstacles, half_size, CELL_SIZE, width);
        Self {
            obstacles,
            half_size,
            cell_size,
            width,
            walkable,
        }
    }

    /// Throwaway clone with one extra obstacle, recomputing walkability only
    /// in its neighborhood (identical result to a full rebuild, see test).
    /// Placement validation uses it to path against the grid *as it will
    /// look once the building exists* — validating on the live grid gives
    /// false positives when the new footprint seals its own approach.
    pub fn cloned_with_obstacle(&self, obstacle: Obstacle) -> Self {
        let mut obstacles = self.obstacles.clone();
        obstacles.push(obstacle);
        let mut walkable = self.walkable.clone();
        let margin = obstacle.half_size
            + Vec2::splat(UNIT_CLEARANCE + self.cell_size * 0.5 + self.cell_size);
        let lo = (obstacle.center - margin + Vec2::splat(self.half_size)) / self.cell_size;
        let hi = (obstacle.center + margin + Vec2::splat(self.half_size)) / self.cell_size;
        let width = self.width as isize;
        for row in (lo.y.floor() as isize).max(0)..=(hi.y.ceil() as isize).min(width - 1) {
            for col in (lo.x.floor() as isize).max(0)..=(hi.x.ceil() as isize).min(width - 1) {
                let index = row as usize * self.width + col as usize;
                walkable[index] = cell_walkable(
                    &obstacles,
                    self.half_size,
                    self.cell_size,
                    self.width,
                    index,
                );
            }
        }
        Self {
            obstacles,
            half_size: self.half_size,
            cell_size: self.cell_size,
            width: self.width,
            walkable,
        }
    }

    /// Body-aware clearance margin for a unit radius. Default grid routing
    /// stays at UNIT_CLEARANCE (cheap, comparable); spawn points and builder
    /// destinations use this so large hulls never spawn intersecting rock.
    /// +0.3 keeps a visual gap without choking the map like a full diameter.
    pub fn clearance_for(radius: f32) -> f32 {
        radius + 0.3
    }

    pub fn has_clearance_for(&self, point: Vec3, radius: f32) -> bool {
        let margin = Self::clearance_for(radius);
        point.is_finite()
            && point.x.abs() <= self.half_size - margin
            && point.z.abs() <= self.half_size - margin
            && self.obstacles.iter().all(|obstacle| {
                let delta = (point.xz() - obstacle.center).abs();
                delta.x >= obstacle.half_size.x + margin || delta.y >= obstacle.half_size.y + margin
            })
    }

    /// Exact swept XZ clearance for a body radius (buildings use the default
    /// 0.8 margin via segment_clear; units check their own hull here).
    pub fn segment_clear_for(&self, from: Vec3, to: Vec3, radius: f32) -> bool {
        let margin = Self::clearance_for(radius);
        if !self.has_clearance_for(from, radius) || !self.has_clearance_for(to, radius) {
            return false;
        }
        let delta = to.xz() - from.xz();
        self.obstacles.iter().all(|o| {
            let lo = o.center - o.half_size - Vec2::splat(margin);
            let hi = o.center + o.half_size + Vec2::splat(margin);
            let mut enter: f32 = 0.0;
            let mut leave: f32 = 1.0;
            for axis in 0..2 {
                if delta[axis].abs() < 1e-8 {
                    if from.xz()[axis] <= lo[axis] || from.xz()[axis] >= hi[axis] {
                        return true;
                    }
                } else {
                    let a = (lo[axis] - from.xz()[axis]) / delta[axis];
                    let b = (hi[axis] - from.xz()[axis]) / delta[axis];
                    enter = enter.max(a.min(b));
                    leave = leave.min(a.max(b));
                }
            }
            enter >= leave
        })
    }

    /// Repair a spawn point for a body radius. Same deterministic spiral as
    /// clear_point but the goal cell must fit the hull, not just the default
    /// scout margin. Falls back to the clamped point if nothing fits in range.
    pub fn clear_point_for(&self, point: Vec3, radius: f32) -> Vec3 {
        let margin = Self::clearance_for(radius);
        let mut point = point;
        point.x = point
            .x
            .clamp(-self.half_size + margin, self.half_size - margin);
        point.z = point
            .z
            .clamp(-self.half_size + margin, self.half_size - margin);
        if self.is_walkable(point) && self.has_clearance_for(point, radius) {
            return point;
        }
        let (cx, cz) = (
            ((point.x + self.half_size) / self.cell_size) as isize,
            ((point.z + self.half_size) / self.cell_size) as isize,
        );
        for ring in 1..=16 {
            for dx in -ring..=ring {
                for dz in [-ring, ring] {
                    if let Some(found) = self.clear_cell_for(cx + dx, cz + dz, point.y, radius) {
                        return found;
                    }
                }
            }
            for dz in -ring + 1..=ring - 1 {
                for dx in [-ring, ring] {
                    if let Some(found) = self.clear_cell_for(cx + dx, cz + dz, point.y, radius) {
                        return found;
                    }
                }
            }
        }
        point
    }

    fn clear_cell_for(&self, x: isize, z: isize, y: f32, radius: f32) -> Option<Vec3> {
        if x < 0 || z < 0 || x >= self.width as isize || z >= self.width as isize {
            return None;
        }
        let index = z as usize * self.width + x as usize;
        if !self.walkable[index] {
            return None;
        }
        let center = self.cell_center(index).with_y(y);
        self.has_clearance_for(center, radius).then_some(center)
    }

    /// Repair a spawn point into walkable, clear ground. Grid-fill layouts
    /// cannot dodge random rects, so slots are fixed here instead of failing
    /// pathfinding. Fast path returns untouched clear points; otherwise a
    /// deterministic spiral over the precomputed walkability finds the
    /// nearest walkable cell with clearance. Obstacle lanes guarantee one
    /// exists within a few rings.
    /// Legacy scout-margin wrapper; gameplay uses [`Self::clear_point_for`].
    /// Kept for tests and comparable baselines.
    #[allow(dead_code)]
    pub fn clear_point(&self, point: Vec3) -> Vec3 {
        let mut point = point;
        point.x = point.x.clamp(
            -self.half_size + UNIT_CLEARANCE,
            self.half_size - UNIT_CLEARANCE,
        );
        point.z = point.z.clamp(
            -self.half_size + UNIT_CLEARANCE,
            self.half_size - UNIT_CLEARANCE,
        );
        if self.is_walkable(point) && self.has_clearance(point) {
            return point;
        }
        let (cx, cz) = (
            ((point.x + self.half_size) / self.cell_size) as isize,
            ((point.z + self.half_size) / self.cell_size) as isize,
        );
        for radius in 1..=12 {
            for dx in -radius..=radius {
                for dz in [-radius, radius] {
                    if let Some(found) = self.clear_cell(cx + dx, cz + dz, point.y) {
                        return found;
                    }
                }
            }
            for dz in -radius + 1..=radius - 1 {
                for dx in [-radius, radius] {
                    if let Some(found) = self.clear_cell(cx + dx, cz + dz, point.y) {
                        return found;
                    }
                }
            }
        }
        point
    }

    #[allow(dead_code)]
    fn clear_cell(&self, x: isize, z: isize, y: f32) -> Option<Vec3> {
        if x < 0 || z < 0 || x >= self.width as isize || z >= self.width as isize {
            return None;
        }
        let index = z as usize * self.width + x as usize;
        if !self.walkable[index] {
            return None;
        }
        let center = self.cell_center(index).with_y(y);
        self.has_clearance(center).then_some(center)
    }

    fn cell(&self, point: Vec3) -> Option<usize> {
        if !point.is_finite() || point.x.abs() >= self.half_size || point.z.abs() >= self.half_size
        {
            return None;
        }
        let x = ((point.x + self.half_size) / self.cell_size) as usize;
        let z = ((point.z + self.half_size) / self.cell_size) as usize;
        Some(z * self.width + x)
    }

    fn cell_center(&self, index: usize) -> Vec3 {
        Vec3::new(
            (index % self.width) as f32 * self.cell_size - self.half_size + self.cell_size * 0.5,
            0.0,
            (index / self.width) as f32 * self.cell_size - self.half_size + self.cell_size * 0.5,
        )
    }

    pub fn is_walkable(&self, point: Vec3) -> bool {
        self.cell(point).is_some_and(|index| self.walkable[index])
    }

    pub fn has_clearance(&self, point: Vec3) -> bool {
        point.is_finite()
            && point.x.abs() <= self.half_size - UNIT_CLEARANCE
            && point.z.abs() <= self.half_size - UNIT_CLEARANCE
            && self.obstacles.iter().all(|obstacle| {
                let delta = (point.xz() - obstacle.center).abs();
                delta.x >= obstacle.half_size.x + UNIT_CLEARANCE
                    || delta.y >= obstacle.half_size.y + UNIT_CLEARANCE
            })
    }

    /// Legacy scout-margin formation; gameplay uses [`Self::formation_for`]
    /// with the largest hull in the group. Kept for tests and baselines.
    #[allow(dead_code)]
    pub fn formation(&self, count: usize, center: Vec3, spacing: f32) -> Option<Vec<Vec3>> {
        self.formation_for(count, center, spacing, 0.5)
    }

    /// Body-aware formation: every slot fits `radius` (via
    /// [`Self::clearance_for`]), not just the default scout margin. Small
    /// hulls (`clearance_for(radius) <= UNIT_CLEARANCE`) delegate to the
    /// legacy fast path so their density is preserved bit-for-bit; larger
    /// hulls (Commander 1.7, T2 1.0-1.05) get unique walkable slots with
    /// full hull clearance. Returns `None` when the map cannot fit `count`
    /// bodies of this size.
    pub fn formation_for(
        &self,
        count: usize,
        center: Vec3,
        spacing: f32,
        radius: f32,
    ) -> Option<Vec<Vec3>> {
        if Self::clearance_for(radius) <= UNIT_CLEARANCE + 1e-6 {
            let mut reserved = HashSet::with_capacity(count);
            return formation_slots(count, center, spacing)
                .into_iter()
                .map(|ideal| {
                    let cell = self
                        .cell(ideal)
                        .filter(|index| self.slot_free(*index, &reserved))
                        .or_else(|| {
                            (0..self.walkable.len())
                                .filter(|index| self.slot_free(*index, &reserved))
                                .min_by(|a, b| {
                                    self.cell_center(*a)
                                        .distance_squared(ideal)
                                        .total_cmp(&self.cell_center(*b).distance_squared(ideal))
                                        .then(a.cmp(b))
                                })
                        })?;
                    reserved.insert(cell);
                    Some(self.cell_center(cell))
                })
                .collect();
        }
        let mut reserved = HashSet::with_capacity(count);
        formation_slots(count, center, spacing)
            .into_iter()
            .map(|ideal| {
                let cell = self
                    .cell(ideal)
                    .filter(|index| self.slot_free_for(*index, &reserved, radius))
                    .or_else(|| {
                        (0..self.walkable.len())
                            .filter(|index| self.slot_free_for(*index, &reserved, radius))
                            .min_by(|a, b| {
                                self.cell_center(*a)
                                    .distance_squared(ideal)
                                    .total_cmp(&self.cell_center(*b).distance_squared(ideal))
                                    .then(a.cmp(b))
                            })
                    })?;
                reserved.insert(cell);
                Some(self.cell_center(cell))
            })
            .collect()
    }

    fn slot_free(&self, index: usize, reserved: &HashSet<usize>) -> bool {
        self.walkable[index]
            && !reserved.contains(&index)
            && self.has_clearance(self.cell_center(index))
    }

    fn slot_free_for(&self, index: usize, reserved: &HashSet<usize>, radius: f32) -> bool {
        self.walkable[index]
            && !reserved.contains(&index)
            && self.has_clearance_for(self.cell_center(index), radius)
    }

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

/// Total order over finite path costs for the Theta* heap. Costs never go
/// NaN (sums of finite distances), so `total_cmp` is a valid ordering and
/// repeated runs agree exactly.
#[derive(Clone, Copy, PartialEq)]
struct FOrd(f32);

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

#[derive(Component)]
pub struct Route {
    pub points: Vec<Vec3>,
    pub next: usize,
}

#[derive(Resource, Default)]
pub struct NavigationStats {
    pub planned: usize,
    pub failed: usize,
    pub last_planned: usize,
    pub last_ms: f64,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlanPaths;

pub struct NavigationPlugin;
impl Plugin for NavigationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NavGrid>()
            .init_resource::<NavigationStats>()
            .add_systems(Update, plan_paths.in_set(PlanPaths));
    }
}

#[allow(clippy::type_complexity)]
fn plan_paths(
    mut commands: Commands,
    grid: Res<NavGrid>,
    spatial: Option<Res<crate::spatial::SpatialGrid>>,
    mut stats: ResMut<NavigationStats>,
    pending: Query<
        (
            Entity,
            &Transform,
            &MoveTarget,
            Option<&crate::units::UnitKind>,
            Option<&crate::units::CollisionRadius>,
        ),
        Without<Route>,
    >,
) {
    let start = Instant::now();
    stats.last_planned = 0;
    // Collect up to budget preserving query order. `par_splat_map` below
    // returns results in input order, so the sequential apply stays
    // bit-identical to the old serial loop for the same world state.
    // Radius travels with the job so planning, collision and destinations
    // agree on the same hull: UnitKind wins (archetype table), then the
    // live CollisionRadius, then the legacy scout default.
    let jobs: Vec<(Entity, Vec3, Vec3, f32)> = pending
        .iter()
        .take(PATHS_PER_FRAME)
        .map(|(entity, transform, target, kind, body)| {
            let radius = kind
                .map(|k| crate::units::archetype(*k).radius)
                .or(body.map(|b| b.0))
                .unwrap_or(0.5);
            (entity, transform.translation.with_y(0.0), target.0, radius)
        })
        .collect();
    if jobs.is_empty() {
        stats.last_ms = start.elapsed().as_secs_f64() * 1000.0;
        return;
    }
    stats.last_planned = jobs.len();
    // Pure compute: `find_path_inner_for` only reads `NavGrid` (+ read-only
    // congestion index), so batch parallelism is embarrassingly parallel.
    // Small batches stay serial to avoid task-spawn overhead.
    let computed: Vec<Option<Vec<Vec3>>> = if jobs.len() < SERIAL_PATH_THRESHOLD {
        jobs.iter()
            .map(|(_, from, goal, radius)| match spatial.as_deref() {
                Some(index) => grid.find_path_congested_for(index, *from, *goal, *radius),
                None => grid.find_path_for(*from, *goal, *radius),
            })
            .collect()
    } else {
        let pool = ComputeTaskPool::get();
        let grid_ref: &NavGrid = &grid;
        let spatial_ref: Option<&crate::spatial::SpatialGrid> = spatial.as_deref();
        jobs.par_splat_map(pool, None, |_, chunk| {
            chunk
                .iter()
                .map(|(_, from, goal, radius)| match spatial_ref {
                    Some(index) => grid_ref.find_path_congested_for(index, *from, *goal, *radius),
                    None => grid_ref.find_path_for(*from, *goal, *radius),
                })
                .collect::<Vec<_>>()
        })
        .into_iter()
        .flatten()
        .collect()
    };
    for ((entity, from, goal, radius), points) in jobs.into_iter().zip(computed) {
        // Congested planning where the spatial index exists (combat
        // scenes); plain movement benchmarks never load it and keep the
        // exact legacy behaviour through the same code path.
        if let Some(points) = points {
            commands.entity(entity).insert(Route { points, next: 0 });
            stats.planned += 1;
        } else if grid.has_clearance_for(goal, radius)
            && (!grid.has_clearance_for(from, radius) || !grid.is_walkable(from))
        {
            // Transient start inside an obstacle margin or on an unwalkable
            // cell edge (crowd shove) with a body-valid goal: keep the order
            // pending and let the movement beeline fallback walk the unit
            // out; a later planning frame succeeds. Every other failure
            // (including goals that never fit this hull) stops the unit and
            // counts, so no order stays blocked forever.
        } else {
            commands.entity(entity).remove::<MoveTarget>();
            stats.failed += 1;
        }
    }
    stats.last_ms = start.elapsed().as_secs_f64() * 1000.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_scratch_reuses_full_grid_buffers_and_resets_touched_cells() {
        let mut scratch = PathScratch::default();
        scratch.prepare(57_600);
        let cost_ptr = scratch.cost.as_ptr();
        let parent_ptr = scratch.parent.as_ptr();
        scratch.set_node(42, 12.0, 7);
        scratch.open.push(Reverse((FOrd(1.0), FOrd(1.0), 42)));

        scratch.prepare(57_600);
        assert_eq!(scratch.cost.as_ptr(), cost_ptr);
        assert_eq!(scratch.parent.as_ptr(), parent_ptr);
        assert!(scratch.cost[42].is_infinite());
        assert_eq!(scratch.parent[42], usize::MAX);
        assert!(scratch.open.is_empty());
        assert!(scratch.touched.is_empty());
    }
    #[test]
    fn cloned_probe_matches_full_rebuild() {
        let grid = NavGrid::default();
        // Big building-like obstacle plus a rock-sized one, on and off map.
        for extra in [
            Obstacle {
                center: Vec2::new(20.0, -30.0),
                half_size: Vec2::new(6.0, 5.0),
            },
            Obstacle {
                center: Vec2::new(-150.0, 120.0),
                half_size: Vec2::new(1.0, 1.0),
            },
        ] {
            let probe = grid.cloned_with_obstacle(extra);
            let mut full = grid.obstacles.clone();
            full.push(extra);
            let rebuilt = NavGrid::new(grid.half_size, grid.cell_size, full);
            assert_eq!(probe.walkable, rebuilt.walkable);
            assert_eq!(probe.obstacles, rebuilt.obstacles);
        }
    }
    #[test]
    fn paths_are_deterministic_and_clear_obstacles() {
        let grid = NavGrid::default();
        // Guaranteed-connected key points (see key_points).
        let start = Vec3::new(-HALF_SIZE * 0.55, 0.0, 0.0);
        let goal = Vec3::new(HALF_SIZE * 0.55, 0.0, 0.0);
        let path = grid.find_path(start, goal).unwrap();
        assert_eq!(Some(path.clone()), grid.find_path(start, goal));
        let mut previous = start;
        for point in path {
            for step in 0..=100 {
                assert!(grid.has_clearance(previous.lerp(point, step as f32 / 100.0)));
            }
            previous = point;
        }
        assert_eq!(previous, goal);
    }
    #[test]
    fn generated_layout_is_deterministic_connected_and_bounded() {
        let first = generate_obstacles(MAP_SEED, HALF_SIZE, MAP_OBSTACLES);
        let second = generate_obstacles(MAP_SEED, HALF_SIZE, MAP_OBSTACLES);
        assert!(!first.is_empty());
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(&second) {
            assert_eq!((a.center, a.half_size), (b.center, b.half_size));
        }
        assert_ne!(
            generate_obstacles(MAP_SEED + 1, HALF_SIZE, MAP_OBSTACLES)[0].center,
            first[0].center
        );
        for obstacle in &first {
            assert!(obstacle.center.x.abs() + obstacle.half_size.x <= HALF_SIZE);
            assert!(obstacle.center.y.abs() + obstacle.half_size.y <= HALF_SIZE);
        }
        // Key points stay walkable and mutually reachable.
        let grid = NavGrid::default();
        for x in [-0.55, -0.35, 0.35, 0.55] {
            let point = Vec3::new(HALF_SIZE * x, 0.0, 0.0);
            assert!(grid.has_clearance(point));
            assert!(
                grid.find_path(point, Vec3::new(HALF_SIZE * 0.55, 0.0, 0.0))
                    .is_some()
            );
        }
    }
    #[test]
    fn open_field_routes_come_out_straight() {
        let grid = NavGrid::new(HALF_SIZE, CELL_SIZE, Vec::new());
        let start = Vec3::new(-50.0, 0.0, -20.0);
        let goal = Vec3::new(50.0, 0.0, 30.0);
        let path = grid.find_path(start, goal).unwrap();
        // Any-angle search: start cell, maybe one turning cell, goal.
        assert!(path.len() <= 3, "staircased path: {path:?}");
        // Within one half-cell diagonal per endpoint of the direct line:
        // the only slack is cell-center quantization, never a detour.
        let direct = start.distance(goal);
        let full: Vec<Vec3> = std::iter::once(start)
            .chain(path.iter().copied())
            .chain(std::iter::once(goal))
            .collect();
        let walked: f32 = full.windows(2).map(|leg| leg[0].distance(leg[1])).sum();
        assert!(walked <= direct + CELL_SIZE * std::f32::consts::SQRT_2 + 0.01);
    }
    #[test]
    fn retarget_from_off_center_goes_direct_without_backtrack() {
        use crate::movement::flat_distance;
        let grid = NavGrid::new(HALF_SIZE, CELL_SIZE, Vec::new());
        // Mid-cell start (a unit re-routed while moving is never exactly on
        // a cell center) with a lateral goal and clear line of sight.
        let start = Vec3::new(0.6, 0.0, 0.3);
        let goal = Vec3::new(10.0, 0.0, 20.0);
        assert!(grid.has_line_of_sight(start.xz(), goal.xz()));
        let path = grid.find_path(start, goal).unwrap();
        // No leading waypoint behind the unit: the first leg must shorten
        // the distance to the goal instead of stepping back to the
        // start-cell center (the visible "micro passo indietro" on lateral
        // re-clicks, with the hull flipping the wrong way first).
        assert!(
            flat_distance(start, path[0]) <= flat_distance(start, goal),
            "first waypoint farther than goal: {path:?}"
        );
        let to_first = path[0] - start;
        let to_goal = goal - start;
        assert!(
            to_first.xz().dot(to_goal.xz()) > 0.0,
            "first leg points away from goal: {path:?}"
        );
        // Open field with LOS: the quantization artifact must be trimmed so
        // the route is a single direct leg.
        assert_eq!(path.len(), 1, "stale start-center waypoint: {path:?}");
        assert_eq!(path[0], goal);
    }
    #[test]
    fn line_of_sight_respects_clearance_margins() {
        let grid = NavGrid::new(
            20.0,
            CELL_SIZE,
            vec![Obstacle {
                center: Vec2::ZERO,
                half_size: Vec2::new(2.0, 2.0),
            }],
        );
        assert!(grid.has_line_of_sight(Vec2::new(-8.0, 8.0), Vec2::new(8.0, 8.0)));
        assert!(!grid.has_line_of_sight(Vec2::new(-8.0, 0.0), Vec2::new(8.0, 0.0)));
        // Grazing the margin counts as blocked: routes keep full clearance.
        assert!(!grid.has_line_of_sight(Vec2::new(-8.0, 2.5), Vec2::new(8.0, 2.5)));
    }

    #[test]
    fn congestion_keeps_routes_valid_and_deterministic() {
        use crate::spatial::SpatialGrid;
        let grid = NavGrid::new(HALF_SIZE, CELL_SIZE, Vec::new());
        let start = Vec3::new(-50.0, 0.0, 0.0);
        let goal = Vec3::new(50.0, 0.0, 0.0);
        // Empty index: congested planning degrades exactly to plain Theta*.
        let empty = SpatialGrid::new(8.0);
        assert_eq!(
            grid.find_path(start, goal),
            grid.find_path_congested(&empty, start, goal)
        );
        // A dense crowd block on the straight line pushes the route around
        // it while staying valid and deterministic.
        let mut crowded = SpatialGrid::new(8.0);
        for index in 0..60 {
            crowded.insert(
                Entity::from_bits(index as u64 + 1),
                Vec3::new(-5.0 + index as f32 * 0.2, 0.0, (index % 2) as f32),
                0.55,
            );
        }
        let bent = grid.find_path_congested(&crowded, start, goal).unwrap();
        assert_eq!(
            Some(bent.clone()),
            grid.find_path_congested(&crowded, start, goal)
        );
        let mut previous = start;
        for point in &bent {
            assert!(grid.has_clearance(*point));
            previous = *point;
        }
        assert_eq!(previous, goal);
    }
    #[test]
    fn invalid_and_unreachable_targets_are_rejected() {
        let grid = NavGrid::default();
        // The first obstacle's core has no clearance by construction.
        let inside = grid.obstacles.first().unwrap().center;
        assert!(
            grid.find_path(
                Vec3::new(-HALF_SIZE * 0.55, 0.0, 0.0),
                Vec3::new(inside.x, 0.0, inside.y)
            )
            .is_none()
        );
        assert!(grid.find_path(Vec3::splat(f32::NAN), Vec3::ZERO).is_none());
        let sealed = NavGrid::new(
            10.0,
            2.5,
            vec![Obstacle {
                center: Vec2::ZERO,
                half_size: Vec2::new(1.0, 10.0),
            }],
        );
        assert!(
            sealed
                .find_path(Vec3::new(-6.0, 0.0, 0.0), Vec3::new(6.0, 0.0, 0.0))
                .is_none()
        );
    }
    #[test]
    fn benchmark_spawn_slots_repair_into_clearance() {
        use crate::formation::formation_slots;
        let grid = NavGrid::default();
        // Mirror units::spawn_units for both benchmark armies.
        for center in [Vec3::new(-110.0, 0.0, 0.0), Vec3::new(110.0, 0.0, 0.0)] {
            let slots: Vec<_> = formation_slots(1000, center, 2.5)
                .into_iter()
                .map(|slot| grid.clear_point(slot))
                .collect();
            assert_eq!(slots.len(), 1000);
            for slot in &slots {
                assert!(
                    grid.is_walkable(*slot) && grid.has_clearance(*slot),
                    "spawn slot without clearance: {slot:?}"
                );
            }
        }
    }
    #[test]
    fn formations_near_edges_and_obstacles_have_unique_free_slots() {
        let grid = NavGrid::default();
        let edge = HALF_SIZE - 5.0;
        for center in [Vec3::ZERO, Vec3::new(edge, 0.0, edge)] {
            let slots = grid.formation(1000, center, 2.5).unwrap();
            assert_eq!(slots.len(), 1000);
            let cells: HashSet<_> = slots.iter().map(|slot| grid.cell(*slot).unwrap()).collect();
            assert_eq!(cells.len(), 1000);
            assert!(
                slots
                    .iter()
                    .all(|slot| grid.is_walkable(*slot) && grid.has_clearance(*slot))
            );
        }
    }
    #[test]
    fn body_aware_clearance_fits_hull_not_just_scout_margin() {
        let grid = NavGrid::new(
            20.0,
            2.5,
            vec![Obstacle {
                center: Vec2::ZERO,
                half_size: Vec2::splat(2.0),
            }],
        );
        // 3.5m from the wall: fine for a scout (margin 0.8), too tight for
        // the commander hull (margin 1.7). Exact predicate, no grid rounding.
        let tight = Vec3::new(3.5, 0.0, 0.0);
        assert!(grid.has_clearance(tight));
        assert!(grid.has_clearance_for(tight, 0.45));
        assert!(!grid.has_clearance_for(tight, 1.4));
        // Swept check agrees along a wall-hugging lane that stays outside
        // the scout margin: scout slips past, commander does not fit.
        let across = Vec3::new(8.0, 0.0, 3.5);
        assert!(grid.segment_clear(tight, across));
        assert!(!grid.segment_clear_for(tight, across, 1.4));
        // Repair near the map edge, where the hull margin (not the baked
        // walkability at 0.8) is the binding constraint.
        let open = NavGrid::new(20.0, 2.5, vec![]);
        let edge = Vec3::new(19.0, 0.0, 0.0);
        assert_eq!(open.clear_point_for(edge, 0.45), edge);
        let repaired = open.clear_point_for(edge, 1.4);
        assert!(open.has_clearance_for(repaired, 1.4));
        assert!(repaired.x <= 20.0 - 1.7);
    }
    #[test]
    fn body_aware_planning_rejects_unexecutable_commander_goals() {
        // Issue #4 repro: empty map, planner at 0.8 accepts x=299 but
        // constrain_motion (margin 1.7) rejects it for the Commander.
        let open = NavGrid::new(HALF_SIZE, CELL_SIZE, Vec::new());
        let start = Vec3::new(280.0, 0.0, 0.0);
        let edge_goal = Vec3::new(299.0, 0.0, 0.0);
        // Legacy scout path still succeeds (preserves small clearance).
        assert!(open.find_path(start, edge_goal).is_some());
        assert!(open.find_path_for(start, edge_goal, 0.45).is_some());
        // Commander hull cannot fit the rim: explicit None, never a stuck order.
        assert!(open.find_path_for(start, edge_goal, 1.4).is_none());
        assert!(!open.has_clearance_for(edge_goal, 1.4));
        // A goal that fits the hull still routes and executes.
        let fit_goal = Vec3::new(290.0, 0.0, 0.0);
        let path = open.find_path_for(start, fit_goal, 1.4).unwrap();
        let mut prev = start;
        for point in &path {
            assert!(open.segment_clear_for(prev, *point, 1.4));
            prev = *point;
        }
        // Deterministic across repeats.
        assert_eq!(Some(path.clone()), open.find_path_for(start, fit_goal, 1.4));
    }
    #[test]
    fn body_aware_planning_respects_narrow_passages() {
        // Exact swept clearance is the contract constrain_motion enforces:
        // a wall-hugging lane that is scout-clear must still reject the
        // commander hull. Grid routing stays conservative (margin-band cells
        // count as blocked), so path assertions use open field where both
        // hulls are walkable; the tight lane is covered by exact segment
        // checks (see body_aware_clearance test).
        let grid = NavGrid::new(
            20.0,
            2.5,
            vec![Obstacle {
                center: Vec2::ZERO,
                half_size: Vec2::splat(2.0),
            }],
        );
        let tight = Vec3::new(3.5, 0.0, 0.0);
        let across = Vec3::new(8.0, 0.0, 3.5);
        assert!(grid.segment_clear(tight, across));
        assert!(!grid.segment_clear_for(tight, across, 1.4));
        // Wide open field still routes for both hulls, deterministically,
        // and every commander leg executes under constrain_motion.
        let open = NavGrid::new(20.0, 2.5, vec![]);
        let start = Vec3::new(-8.0, 0.0, 0.0);
        let goal = Vec3::new(8.0, 0.0, 0.0);
        let scout = open.find_path_for(start, goal, 0.45).unwrap();
        let cmd = open.find_path_for(start, goal, 1.4).unwrap();
        assert_eq!(Some(scout.clone()), open.find_path_for(start, goal, 0.45));
        assert_eq!(Some(cmd.clone()), open.find_path_for(start, goal, 1.4));
        let mut prev = start;
        for point in &cmd {
            assert!(open.segment_clear_for(prev, *point, 1.4));
            prev = *point;
        }
    }
    #[test]
    fn body_aware_formations_fit_mixed_hulls() {
        let grid = NavGrid::default();
        // Small hulls keep legacy density.
        let small = grid.formation_for(100, Vec3::ZERO, 2.5, 0.45).unwrap();
        assert_eq!(small.len(), 100);
        for slot in &small {
            assert!(grid.has_clearance_for(*slot, 0.45));
        }
        // Commander formation: fewer, but every slot fits the 1.7 margin and
        // stays unique.
        let big = grid.formation_for(20, Vec3::ZERO, 2.5, 1.4).unwrap();
        assert_eq!(big.len(), 20);
        let cells: HashSet<_> = big.iter().map(|s| grid.cell(*s).unwrap()).collect();
        assert_eq!(cells.len(), 20);
        for slot in &big {
            assert!(grid.is_walkable(*slot));
            assert!(grid.has_clearance_for(*slot, 1.4));
        }
    }
    #[test]
    fn parallel_batch_matches_serial_and_preserves_order() {
        use bevy::tasks::{ParallelSlice, TaskPool};
        // Batch above SERIAL_PATH_THRESHOLD so production takes the parallel
        // branch; open field keeps every search successful and comparable.
        let grid = NavGrid::new(HALF_SIZE, CELL_SIZE, Vec::new());
        let jobs: Vec<(u32, Vec3, Vec3)> = (0..40)
            .map(|i| {
                let f = i as f32;
                (
                    i,
                    Vec3::new(-90.0 + f, 0.0, -60.0 + (f * 1.7) % 120.0),
                    Vec3::new(90.0 - f * 0.5, 0.0, 60.0 - (f * 2.3) % 120.0),
                )
            })
            .collect();
        let serial: Vec<Option<Vec<Vec3>>> = jobs
            .iter()
            .map(|(_, from, goal)| grid.find_path(*from, *goal))
            .collect();
        assert!(serial.iter().all(|p| p.is_some()));
        let pool = TaskPool::new();
        let parallel: Vec<Option<Vec<Vec3>>> = jobs
            .par_splat_map(&pool, None, |_, chunk| {
                chunk
                    .iter()
                    .map(|(_, from, goal)| grid.find_path(*from, *goal))
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(serial, parallel);
        // Order preserved: input i maps to output i.
        assert_eq!(parallel.len(), jobs.len());
    }
    #[test]
    fn planning_budget_covers_parallel_batch_in_one_frame() {
        use crate::movement::MoveTarget;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(NavGrid::new(HALF_SIZE, CELL_SIZE, Vec::new()))
            .add_plugins(NavigationPlugin);
        app.update();
        for i in 0..40 {
            let f = i as f32;
            app.world_mut().spawn((
                Transform::from_xyz(-90.0 + f, 0.0, -60.0 + f),
                MoveTarget(Vec3::new(90.0 - f * 0.5, 0.0, 60.0 - f)),
            ));
        }
        app.update();
        let stats = app.world().resource::<NavigationStats>();
        // Budget is 128: all 40 plan in a single frame, exercising the
        // parallel branch (>= SERIAL_PATH_THRESHOLD).
        assert_eq!(stats.last_planned, 40);
        assert_eq!(stats.planned, 40);
        assert_eq!(stats.failed, 0);
        let routed = app.world_mut().query::<&Route>().iter(app.world()).count();
        assert_eq!(routed, 40);
    }
}
