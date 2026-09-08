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
#[derive(Component)]
pub struct Factory {
    pub queue: VecDeque<Job>,
    pub rally: Option<Vec3>,
    pub blocked: bool,
    pub tier: u8,
    /// Ticks to skip costly exit probes while blocked. Decremented on each
    /// ready tick; a rally change bypasses it so re-targeting restarts
    /// production immediately.
    pub retry_in: u8,
    /// Rally used at the last failed probe. `None` until the first block.
    pub retry_rally: Option<Vec3>,
}
impl Default for Factory {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
            rally: None,
            blocked: false,
            tier: 1,
            retry_in: 0,
            retry_rally: None,
        }
    }
}
impl Factory {
    pub fn enqueue(&mut self, kind: UnitKind) -> bool {
        // Il Comandante è unico per team: mai in coda factory.
        if kind.is_commander() {
            return false;
        }
        // Tier gate: le unità T2 escono solo dal LabT2.
        if units::archetype(kind).tier > self.tier {
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
            self.retry_in = 0;
        }
    }
}
pub struct ProductionPlugin;
impl Plugin for ProductionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, release_products.after(EconomyTick));
    }
}

/// Ticks to wait between exit probes for a blocked factory. At the 20 Hz
/// economy tick this is ~1 s: a surrounded factory retries cheaply instead
/// of burning up to 12 `find_path` searches every tick, while a freed exit
/// still restarts production within a second. A rally change bypasses the
/// wait so re-targeting restarts immediately.
pub const BLOCKED_RETRY_TICKS: u8 = 20;

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
            // Body-aware: the exiting hull must be able to execute the route,
            // not just the scout margin.
            let outside = point + delta.normalize() * 4.0;
            let goal = rally.unwrap_or(outside);
            if grid.find_path_for(point, goal, radius).is_some() {
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
    let mut sorted: Vec<_> = factories.iter_mut().collect();
    if sorted.is_empty() {
        return;
    }
    sorted.sort_by_key(|row| row.0.to_bits());
    // Fast path: skip the O(units) occupancy scan unless at least one live
    // factory actually needs an exit probe this tick (ready product + either
    // unblocked, rally-changed, or cooldown expired).
    let needs_probe = sorted.iter().any(|(_, _, _, _, health, factory)| {
        if health.current <= 0.0 {
            return false;
        }
        if !factory
            .queue
            .front()
            .is_some_and(|job| job.project.complete())
        {
            return false;
        }
        if !factory.blocked {
            return true;
        }
        factory.rally != factory.retry_rally || factory.retry_in == 0
    });
    if !needs_probe {
        // Still tick down blocked cooldowns so freed exits restart without
        // ever paying for occupancy or path searches while fully stalled.
        for (_, _, _, _, health, mut factory) in sorted {
            if health.current <= 0.0 {
                continue;
            }
            if !factory
                .queue
                .front()
                .is_some_and(|job| job.project.complete())
            {
                continue;
            }
            if factory.blocked && factory.rally == factory.retry_rally && factory.retry_in > 0 {
                factory.retry_in -= 1;
            }
        }
        return;
    }
    let mut occupied: Option<Vec<(Vec3, f32)>> = None;
    for (_, team, transform, kind, health, mut factory) in sorted {
        if health.current <= 0.0 {
            continue;
        }
        let Some(job) = factory.queue.front().filter(|job| job.project.complete()) else {
            continue;
        };
        // Stagger costly probes while blocked: skip the up-to-12 find_path
        // search until the cooldown expires, unless the rally changed (fresh
        // target may be reachable even when the old one was not).
        if factory.blocked && factory.rally == factory.retry_rally && factory.retry_in > 0 {
            factory.retry_in -= 1;
            continue;
        }
        let unit_kind = job.kind;
        let radius = units::archetype(unit_kind).radius;
        let occupied = occupied
            .get_or_insert_with(|| units.iter().map(|(t, r)| (t.translation, r.0)).collect());
        let Some(position) = free_exit(
            &grid,
            transform.translation,
            kind.stats().half,
            radius,
            occupied,
            factory.rally,
        ) else {
            factory.blocked = true;
            factory.retry_in = BLOCKED_RETRY_TICKS;
            factory.retry_rally = factory.rally;
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
        factory.retry_in = 0;
        factory.retry_rally = None;
    }
}
