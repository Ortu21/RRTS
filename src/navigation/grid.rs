//! Navigazione — griglia: occupancy, ostacoli, clearance scafo, formazioni.
//!
//! Owner di `NavGrid` storage + walkability. Theta* vive in `theta`,
//! congestione in `congestion`, budget/scheduling in `budget`.

use crate::formation::formation_slots;
use bevy::prelude::*;
use std::collections::HashSet;

pub const HALF_SIZE: f32 = 300.0;
pub const CELL_SIZE: f32 = 2.5;
pub const UNIT_CLEARANCE: f32 = 0.8;

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
    pub(crate) half_size: f32,
    pub(crate) cell_size: f32,
    pub(crate) width: usize,
    pub(crate) walkable: Vec<bool>,
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

    pub(crate) fn cell(&self, point: Vec3) -> Option<usize> {
        if !point.is_finite() || point.x.abs() >= self.half_size || point.z.abs() >= self.half_size
        {
            return None;
        }
        let x = ((point.x + self.half_size) / self.cell_size) as usize;
        let z = ((point.z + self.half_size) / self.cell_size) as usize;
        Some(z * self.width + x)
    }

    pub(crate) fn cell_center(&self, index: usize) -> Vec3 {
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
}
