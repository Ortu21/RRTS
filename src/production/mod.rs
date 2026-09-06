//! Industrial queues are independent of mobile unit orders.
use crate::{
    combat::Health,
    economy::{EconomyTick, Project, balance::*},
    movement::queue_move,
    navigation::NavGrid,
    structures::{Building, Construction},
    units::{self, CollisionRadius, Team, Unit, UnitKind},
};
use bevy::prelude::*;
use std::collections::VecDeque;

#[derive(Clone)]
pub struct Job {
    pub kind: UnitKind,
    pub project: Project,
}
#[derive(Component, Default)]
pub struct Factory {
    pub queue: VecDeque<Job>,
    pub rally: Option<Vec3>,
    pub blocked: bool,
}
impl Factory {
    pub fn enqueue(&mut self, kind: UnitKind) -> bool {
        // Il Comandante è unico per team: mai in coda factory.
        if kind.is_commander() {
            return false;
        }
        if self.queue.len() >= MAX_QUEUE {
            return false;
        }
        self.queue.push_back(Job {
            kind,
            project: Project::new(unit_cost(kind)),
        });
        true
    }
    pub fn cancel(&mut self, index: usize) {
        self.queue.remove(index);
        if index == 0 {
            self.blocked = false;
        }
    }
}
pub struct ProductionPlugin;
impl Plugin for ProductionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, release_products.after(EconomyTick));
    }
}

/// Deterministic doors around the perimeter. Never search beyond the door
/// apron: a surrounded factory must retain its product, not teleport it.
pub fn free_exit(
    grid: &NavGrid,
    center: Vec3,
    half: Vec2,
    radius: f32,
    occupied: &[(Vec3, f32)],
    rally: Option<Vec3>,
) -> Option<Vec3> {
    let ring = half + Vec2::splat(3.0);
    for side in 0..4 {
        for offset in [0.0, -2.5, 2.5] {
            let delta = match side {
                0 => Vec3::new(offset, 0.0, ring.y),
                1 => Vec3::new(ring.x, 0.0, offset),
                2 => Vec3::new(offset, 0.0, -ring.y),
                _ => Vec3::new(-ring.x, 0.0, offset),
            };
            let point = (center + delta).with_y(0.0);
            // Body-aware door check: the exiting hull must fit, not just the
            // default scout margin.
            if !grid.is_walkable(point) || !grid.has_clearance_for(point, radius) {
                continue;
            }
            if occupied
                .iter()
                .any(|(p, r)| p.xz().distance(point.xz()) < r + radius + 0.3)
            {
                continue;
            }
            // An outward apron must also be reachable: don't spawn in a tiny
            // sealed pocket. A set rally additionally requires a valid route.
            let outside = point + delta.normalize() * 4.0;
            let goal = rally.unwrap_or(outside);
            if grid.find_path(point, goal).is_some() {
                return Some(point);
            }
        }
    }
    None
}
#[allow(clippy::type_complexity)]
fn release_products(
    mut commands: Commands,
    grid: Res<NavGrid>,
    mut ids: ResMut<units::UnitIds>,
    mut factories: Query<
        (
            Entity,
            &Team,
            &Transform,
            &BuildingKind,
            &Health,
            &mut Factory,
        ),
        (With<Building>, Without<Construction>),
    >,
    units: Query<(&Transform, &CollisionRadius), With<Unit>>,
) {
    let mut occupied: Vec<_> = units.iter().map(|(t, r)| (t.translation, r.0)).collect();
    let mut sorted: Vec<_> = factories.iter_mut().collect();
    sorted.sort_by_key(|row| row.0.to_bits());
    for (_, team, transform, kind, health, mut factory) in sorted {
        if health.current <= 0.0 {
            continue;
        }
        let Some(job) = factory.queue.front().filter(|job| job.project.complete()) else {
            continue;
        };
        let unit_kind = job.kind;
        let radius = units::archetype(unit_kind).radius;
        let Some(position) = free_exit(
            &grid,
            transform.translation,
            kind.stats().half,
            radius,
            &occupied,
            factory.rally,
        ) else {
            factory.blocked = true;
            continue;
        };
        let id = ids.allocate();
        let entity = units::spawn_combat_unit(&mut commands, id, *team, unit_kind, position);
        if let Some(rally) = factory.rally {
            queue_move(&mut commands.entity(entity), rally);
        }
        occupied.push((position, radius));
        factory.queue.pop_front();
        factory.blocked = false;
    }
}
