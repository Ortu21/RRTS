use crate::{formation::formation_slots, movement::MoveTarget};
use bevy::prelude::*;
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashSet},
    time::Instant,
};

pub const HALF_SIZE: f32 = 200.0;
pub const CELL_SIZE: f32 = 2.5;
pub const UNIT_CLEARANCE: f32 = 0.8;
pub const PATHS_PER_FRAME: usize = 32;

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

    /// Push a spawn point out of obstacle margins (plus unit clearance) and
    /// back inside the map. Grid-fill layouts cannot dodge random rects, so
    /// skirmish slots are repaired here instead of failing pathfinding.
    /// Converges: obstacle lanes guarantee free space within a few passes.
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
        for _ in 0..4 {
            let mut moved = false;
            for obstacle in &self.obstacles {
                let delta = (point.xz() - obstacle.center).abs();
                let push_x = obstacle.half_size.x + UNIT_CLEARANCE - delta.x;
                let push_z = obstacle.half_size.y + UNIT_CLEARANCE - delta.y;
                if push_x > 0.0 && push_z > 0.0 {
                    if push_x < push_z {
                        let sign = if point.x >= obstacle.center.x {
                            1.0
                        } else {
                            -1.0
                        };
                        point.x += push_x * sign;
                    } else {
                        let sign = if point.z >= obstacle.center.y {
                            1.0
                        } else {
                            -1.0
                        };
                        point.z += push_z * sign;
                    }
                    moved = true;
                }
            }
            if !moved {
                break;
            }
            point.x = point.x.clamp(
                -self.half_size + UNIT_CLEARANCE,
                self.half_size - UNIT_CLEARANCE,
            );
            point.z = point.z.clamp(
                -self.half_size + UNIT_CLEARANCE,
                self.half_size - UNIT_CLEARANCE,
            );
        }
        point
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
                let cell = self
                    .cell(ideal)
                    .filter(|index| self.walkable[*index] && !reserved.contains(index))
                    .or_else(|| {
                        (0..self.walkable.len())
                            .filter(|index| self.walkable[*index] && !reserved.contains(index))
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

    pub fn find_path(&self, start: Vec3, goal: Vec3) -> Option<Vec<Vec3>> {
        if !self.has_clearance(start) || !self.has_clearance(goal) {
            return None;
        }
        let start_cell = self.cell(start)?;
        let goal_cell = self.cell(goal)?;
        if !self.walkable[start_cell] || !self.walkable[goal_cell] {
            return None;
        }
        let mut cost = vec![u32::MAX; self.walkable.len()];
        let mut parent = vec![usize::MAX; self.walkable.len()];
        let mut open = BinaryHeap::new();
        let heuristic = |index: usize| {
            let dx = (index % self.width).abs_diff(goal_cell % self.width) as u32;
            let dz = (index / self.width).abs_diff(goal_cell / self.width) as u32;
            10 * dx.max(dz) + 4 * dx.min(dz)
        };
        cost[start_cell] = 0;
        open.push(Reverse((heuristic(start_cell), 0, start_cell)));
        while let Some(Reverse((_, queued_cost, current))) = open.pop() {
            if queued_cost != cost[current] {
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
                let next_cost = cost[current] + if diagonal { 14 } else { 10 };
                if next_cost < cost[next] {
                    cost[next] = next_cost;
                    parent[next] = current;
                    open.push(Reverse((next_cost + heuristic(next), next_cost, next)));
                }
            }
        }
        None
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
    mut stats: ResMut<NavigationStats>,
    pending: Query<(Entity, &Transform, &MoveTarget), Without<Route>>,
) {
    let start = Instant::now();
    stats.last_planned = 0;
    for (entity, transform, target) in pending.iter().take(PATHS_PER_FRAME) {
        stats.last_planned += 1;
        if let Some(points) = grid.find_path(transform.translation.with_y(0.0), target.0) {
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
}
