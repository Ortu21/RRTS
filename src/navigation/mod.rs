//! Navigazione: griglia 240x240 su 600x600m, Theta* any-angle + LOS.
//!
//! Split per dominio (re-export invariato):
//! - `grid` — occupancy, ostacoli, clearance, formazioni.
//! - `theta` — Theta* + LOS + scratch arena.
//! - `congestion` — costi live da spatial.
//! - `budget` — scheduling, stat, plugin.
//!
//! Vedi `ARCHITECTURE.md`.

pub mod budget;
pub mod congestion;
pub mod grid;
pub mod theta;

pub use budget::{
    NavigationPlugin, NavigationStats, PATHS_PER_FRAME, PlanPaths, Route, SERIAL_PATH_THRESHOLD,
};
pub use congestion::{CONGESTION_WEIGHT, congestion_cost};
pub use grid::{
    CELL_SIZE, HALF_SIZE, MAP_OBSTACLES, MAP_SEED, NavGrid, Obstacle, UNIT_CLEARANCE,
    generate_obstacles,
};
#[cfg(test)]
pub(crate) use theta::{FOrd, PathScratch};

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;
    use std::cmp::Reverse;
    use std::collections::HashSet;

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
