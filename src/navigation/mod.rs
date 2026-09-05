use crate::{formation::formation_slots, movement::MoveTarget};
use bevy::prelude::*;
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashSet},
    time::Instant,
};

pub const HALF_SIZE: f32 = 100.0;
pub const CELL_SIZE: f32 = 2.5;
pub const UNIT_CLEARANCE: f32 = 0.8;
pub const PATHS_PER_FRAME: usize = 32;

#[derive(Clone, Copy)]
pub struct Obstacle {
    pub center: Vec2,
    pub half_size: Vec2,
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
            vec![
                Obstacle {
                    center: Vec2::new(0.0, -57.5),
                    half_size: Vec2::new(3.5, 32.5),
                },
                Obstacle {
                    center: Vec2::ZERO,
                    half_size: Vec2::new(3.5, 15.0),
                },
                Obstacle {
                    center: Vec2::new(0.0, 57.5),
                    half_size: Vec2::new(3.5, 32.5),
                },
            ],
        )
    }
}

impl NavGrid {
    pub fn new(half_size: f32, cell_size: f32, obstacles: Vec<Obstacle>) -> Self {
        let width = (half_size * 2.0 / cell_size).round() as usize;
        let mut grid = Self {
            obstacles,
            half_size,
            cell_size,
            width,
            walkable: vec![true; width * width],
        };
        // Block every cell touched by an obstacle expanded by the unit radius.
        // Routes through the remaining cells have clearance even at diagonal turns.
        let margin = UNIT_CLEARANCE + cell_size * 0.5;
        for index in 0..grid.walkable.len() {
            let center = grid.cell_center(index).xz();
            grid.walkable[index] = grid.obstacles.iter().all(|obstacle| {
                let delta = (center - obstacle.center).abs();
                delta.x > obstacle.half_size.x + margin || delta.y > obstacle.half_size.y + margin
            });
        }
        grid
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
        let start = Vec3::new(-55.0, 0.0, 0.0);
        let goal = Vec3::new(55.0, 0.0, 0.0);
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
    fn invalid_and_unreachable_targets_are_rejected() {
        let grid = NavGrid::default();
        assert!(
            grid.find_path(Vec3::new(-55.0, 0.0, 0.0), Vec3::ZERO)
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
        for center in [Vec3::ZERO, Vec3::new(99.0, 0.0, 99.0)] {
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
