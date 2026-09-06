use crate::{formation::formation_slots, movement::MoveTarget};
use bevy::prelude::*;
use std::{
    cmp::{Ordering, Reverse},
    collections::{BinaryHeap, HashSet},
    time::Instant,
};

pub const HALF_SIZE: f32 = 200.0;
pub const CELL_SIZE: f32 = 2.5;
pub const UNIT_CLEARANCE: f32 = 0.8;
pub const PATHS_PER_FRAME: usize = 32;
/// Extra route cost per crowded body in the destination cell: routes spread
/// around live crowds instead of piling through them. Tunable; validated by
/// keeping zero path failures on the stress workloads.
pub const CONGESTION_WEIGHT: f32 = 0.5;

/// Fixed map seed: obstacle layout is identical every run, on every
/// platform, so benchmark checksums stay comparable.
pub const MAP_SEED: u64 = 20260907;
/// Target obstacle count for the default map.
pub const MAP_OBSTACLES: usize = 44;

#[derive(Clone, Copy, Debug)]
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
    let margin = UNIT_CLEARANCE + cell_size * 0.5;
    (0..width * width)
        .map(|index| {
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
        })
        .collect()
}

/// Key gameplay points (fractions of half size) that must stay walkable and
/// mutually reachable: team spawns, attack targets, map arteries. Adding a
/// rect that breaks any of them discards the rect, so generation always
/// terminates with a connected map.
fn key_points(half_size: f32) -> [Vec2; 6] {
    let (a, b, c) = (half_size * 0.55, half_size * 0.35, half_size - 10.0);
    [
        Vec2::new(-a, 0.0),
        Vec2::new(a, 0.0),
        Vec2::new(-b, 0.0),
        Vec2::new(b, 0.0),
        Vec2::new(0.0, -c),
        Vec2::new(0.0, c),
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

#[derive(Resource)]
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
        let width = (half_size * 2.0 / cell_size).round() as usize;
        let walkable = build_walkable(&obstacles, half_size, cell_size, width);
        Self {
            obstacles,
            half_size,
            cell_size,
            width,
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
                delta.x >= obstacle.half_size.x + margin
                    || delta.y >= obstacle.half_size.y + margin
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
        point.x = point.x.clamp(-self.half_size + margin, self.half_size - margin);
        point.z = point.z.clamp(-self.half_size + margin, self.half_size - margin);
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
                    if let Some(found) = self.clear_cell_for(cx + dx, cz + dz, point.y, radius)
                    {
                        return found;
                    }
                }
            }
            for dz in -ring + 1..=ring - 1 {
                for dx in [-ring, ring] {
                    if let Some(found) = self.clear_cell_for(cx + dx, cz + dz, point.y, radius)
                    {
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

    pub fn formation(&self, count: usize, center: Vec3, spacing: f32) -> Option<Vec<Vec3>> {
        let mut reserved = HashSet::with_capacity(count);
        formation_slots(count, center, spacing)
            .into_iter()
            .map(|ideal| {
                // Slots are cell centers, so clearance of the center is exact:
                // every returned slot is walkable, unique and clear.
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
            .collect()
    }

    fn slot_free(&self, index: usize, reserved: &HashSet<usize>) -> bool {
        self.walkable[index]
            && !reserved.contains(&index)
            && self.has_clearance(self.cell_center(index))
    }

    pub fn find_path(&self, start: Vec3, goal: Vec3) -> Option<Vec<Vec3>> {
        self.find_path_inner(None, start, goal)
    }

    /// Theta* with live congestion costs: entering a crowded cell costs
    /// extra, so fresh routes flow around battles instead of through them.
    /// Only new plans see congestion (no mid-route replanning, no churn);
    /// density comes from the pre-movement spatial index, so results stay
    /// deterministic across repeats.
    pub fn find_path_congested(
        &self,
        spatial: &crate::spatial::SpatialGrid,
        start: Vec3,
        goal: Vec3,
    ) -> Option<Vec<Vec3>> {
        self.find_path_inner(Some(spatial), start, goal)
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
        let mut cost = vec![f32::INFINITY; self.walkable.len()];
        let mut parent = vec![usize::MAX; self.walkable.len()];
        let mut open = BinaryHeap::new();
        let center = |index: usize| self.cell_center(index).xz();
        let segment = |a: usize, b: usize| center(a).distance(center(b));
        let heuristic = |index: usize| center(index).distance(center(goal_cell));
        cost[start_cell] = 0.0;
        parent[start_cell] = start_cell;
        open.push(Reverse((
            FOrd(heuristic(start_cell)),
            FOrd(0.0),
            start_cell,
        )));
        while let Some(Reverse((_, queued, current))) = open.pop() {
            if queued.0 != cost[current] {
                continue;
            }
            if current == goal_cell {
                let mut cells = vec![current];
                let mut next = current;
                while next != start_cell {
                    next = parent[next];
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
                let via = parent[current];
                let (next_parent, next_cost) = if self.has_line_of_sight(center(via), center(next))
                {
                    (via, cost[via] + segment(via, next))
                } else {
                    (current, cost[current] + segment(current, next))
                };
                let next_cost = next_cost
                    + spatial.map_or(0.0, |grid| {
                        CONGESTION_WEIGHT * grid.bucket_count(center(next)) as f32
                    });
                if next_cost < cost[next] {
                    cost[next] = next_cost;
                    parent[next] = next_parent;
                    open.push(Reverse((
                        FOrd(next_cost + heuristic(next)),
                        FOrd(next_cost),
                        next,
                    )));
                }
            }
        }
        None
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

fn plan_paths(
    mut commands: Commands,
    grid: Res<NavGrid>,
    spatial: Option<Res<crate::spatial::SpatialGrid>>,
    mut stats: ResMut<NavigationStats>,
    pending: Query<(Entity, &Transform, &MoveTarget), Without<Route>>,
) {
    let start = Instant::now();
    stats.last_planned = 0;
    for (entity, transform, target) in pending.iter().take(PATHS_PER_FRAME) {
        stats.last_planned += 1;
        // Congested planning where the spatial index exists (combat
        // scenes); plain movement benchmarks never load it and keep the
        // exact legacy behaviour through the same code path.
        let points = match spatial.as_deref() {
            Some(index) => {
                grid.find_path_congested(index, transform.translation.with_y(0.0), target.0)
            }
            None => grid.find_path(transform.translation.with_y(0.0), target.0),
        };
        if let Some(points) = points {
            commands.entity(entity).insert(Route { points, next: 0 });
            stats.planned += 1;
        } else if grid.has_clearance(target.0)
            && (!grid.has_clearance(transform.translation.with_y(0.0))
                || !grid.is_walkable(transform.translation.with_y(0.0)))
        {
            // Transient start inside an obstacle margin or on an unwalkable
            // cell edge (crowd shove) with a valid goal: keep the order
            // pending and let the movement beeline fallback walk the unit
            // out; a later planning frame succeeds. Every other failure
            // stops the unit and counts, as before.
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
        for center in [Vec3::ZERO, Vec3::new(195.0, 0.0, 195.0)] {
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
}
