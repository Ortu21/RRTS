//! Navigazione — budget: scheduling plan_paths, stat, plugin.
//!
//! `PATHS_PER_FRAME=128`, seriale sotto 16. Risultati in ordine input,
//! quindi bit-identici al loop seriale.

use super::grid::NavGrid;
use crate::movement::MoveTarget;
use bevy::{
    prelude::*,
    tasks::{ComputeTaskPool, ParallelSlice},
};
use std::time::Instant;

/// Below this many pending requests the planner stays serial: spawning
/// tasks costs more than the search itself on tiny batches.
pub const SERIAL_PATH_THRESHOLD: usize = 16;
pub const PATHS_PER_FRAME: usize = 128;

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
