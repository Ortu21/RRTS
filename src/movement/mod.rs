use crate::{
    combat::{AttackTarget, Chasing, HoldFire},
    navigation::{PlanPaths, Route},
    orders::UnitOrder,
    units::UnitKind,
};
use bevy::prelude::*;

pub struct MovementPlugin;
impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, move_units.in_set(MovementSystems).after(PlanPaths));
    }
}

/// Ordering anchor so avoidance and combat chase run after route movement.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MovementSystems;
#[derive(Component)]
pub struct Movement {
    pub speed: f32,
}
#[derive(Component)]
pub struct MoveTarget(pub Vec3);

/// Arrival radius: route/beeline arrivals snap exactly, so anything inside
/// counts as arrived and completes the order instead of re-issuing it.
/// Without this, completed move orders resurrect every frame and churn the
/// budgeted path planner forever.
pub const ARRIVE_RADIUS: f32 = 0.5;

/// A plain move order: march to the destination, firing at enemies on the
/// way without ever stopping or chasing. Replaces any other intent and
/// drops temporary combat state.
pub fn queue_move(entity: &mut EntityCommands, destination: Vec3) {
    entity
        .insert((UnitOrder::Move { destination }, MoveTarget(destination)))
        .remove::<(Route, AttackTarget, Chasing, HoldFire)>();
}

#[allow(clippy::type_complexity)]
fn move_units(
    mut commands: Commands,
    time: Res<Time>,
    mut units: Query<
        (
            Entity,
            &mut Transform,
            &Movement,
            &MoveTarget,
            &UnitKind,
            Option<&mut Route>,
        ),
        (Without<Chasing>, Without<HoldFire>),
    >,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, movement, target, kind, route) in &mut units {
        let turn_rate = crate::units::archetype(*kind).hull_turn;
        match route {
            Some(mut route) => {
                let mut remaining = movement.speed * dt;
                while route.next < route.points.len() {
                    let destination = route.points[route.next].with_y(transform.translation.y);
                    let offset = destination - transform.translation;
                    let distance = offset.length();
                    if distance <= f32::EPSILON {
                        route.next += 1;
                        continue;
                    }
                    face_toward(&mut transform, offset, turn_rate, dt);
                    if distance <= remaining {
                        transform.translation = destination;
                        remaining -= distance;
                        route.next += 1;
                    } else {
                        transform.translation += offset.normalize_or_zero() * remaining;
                        break;
                    }
                }
                if route.next == route.points.len() {
                    commands.entity(entity).remove::<(MoveTarget, Route)>();
                }
            }
            None => {
                // No planned route yet (planner backlog) or a transiently
                // unplannable start: steer directly so units keep closing in
                // instead of stranding. The planner upgrades them to a real
                // route when budget allows; arrival snaps exactly.
                let step = movement.speed * dt;
                let goal = target.0.with_y(transform.translation.y);
                let offset = goal - transform.translation;
                let distance = offset.length();
                if distance <= step {
                    transform.translation = goal;
                    commands.entity(entity).remove::<MoveTarget>();
                } else if distance > f32::EPSILON {
                    face_toward(&mut transform, offset, turn_rate, dt);
                    transform.translation += offset / distance * step;
                }
            }
        }
    }
}

/// Yaw (radians, Y-up) facing `direction` on the XZ plane with -Z forward.
/// Pure helper shared by body facing and turret aim.
pub fn yaw_toward(direction: Vec3) -> f32 {
    (-direction.x).atan2(-direction.z)
}

/// Yaw (radians) wrapped to [-PI, PI] for stable turret-relative angles.
pub fn wrap_angle(angle: f32) -> f32 {
    let two_pi = std::f32::consts::TAU;
    ((angle + std::f32::consts::PI).rem_euclid(two_pi)) - std::f32::consts::PI
}

/// Step `current` yaw toward `target` by at most `max_step` radians, taking
/// the short way around. Snaps exactly when close. Pure and deterministic.
pub fn rotate_toward(current: f32, target: f32, max_step: f32) -> f32 {
    let delta = wrap_angle(target - current);
    if delta.abs() <= max_step {
        target
    } else {
        current + delta.signum() * max_step
    }
}

/// Turn a unit body toward its current step direction at `turn_rate` rad/s.
/// Idle units keep their last facing; rotation never feeds back into the
/// simulation. Translation is untouched: facing is smooth, arrival exact.
pub fn face_toward(transform: &mut Transform, offset: Vec3, turn_rate: f32, dt: f32) {
    if offset.xz().length_squared() <= f32::EPSILON {
        return;
    }
    let current = transform.rotation.to_euler(EulerRot::YXZ).0;
    transform.rotation =
        Quat::from_rotation_y(rotate_toward(current, yaw_toward(offset), turn_rate * dt));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        navigation::{
            CELL_SIZE, HALF_SIZE, NavGrid, NavigationPlugin, NavigationStats, PATHS_PER_FRAME,
        },
        units::UnitKind,
    };
    use bevy::time::TimeUpdateStrategy;
    use std::f32::consts::{FRAC_PI_2, PI};
    use std::time::Duration;

    #[test]
    fn yaw_faces_travel_direction_with_minus_z_forward() {
        assert!(yaw_toward(Vec3::new(0.0, 0.0, -1.0)).abs() < 0.000001);
        assert!((yaw_toward(Vec3::X) + FRAC_PI_2).abs() < 0.000001);
        assert!((yaw_toward(Vec3::Z).abs() - PI).abs() < 0.000001);
        assert_eq!(wrap_angle(0.0), 0.0);
        assert!((wrap_angle(3.0 * PI).abs() - PI).abs() < 0.0001);
    }

    #[test]
    fn rotate_toward_takes_short_steps_and_snaps() {
        assert_eq!(rotate_toward(0.0, 1.0, 0.1), 0.1);
        assert_eq!(rotate_toward(0.0, 0.05, 0.1), 0.05);
        // Short way around the wrap: +179° to -179° snaps across the seam.
        assert_eq!(rotate_toward(PI - 0.01, -PI + 0.01, 0.1), -PI + 0.01);
        // Full sweep converges in bounded steps, frame-rate independent.
        let mut yaw = 0.0;
        for _ in 0..100 {
            yaw = rotate_toward(yaw, PI, 0.1);
        }
        assert!((yaw.abs() - PI).abs() < 0.0001);
    }

    fn simulation(dt: f64) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                dt,
            )))
            .add_plugins((NavigationPlugin, MovementPlugin))
            // Hermetic open field: movement logic must not depend on the
            // generated map layout.
            .insert_resource(NavGrid::new(HALF_SIZE, CELL_SIZE, Vec::new()));
        app.update();
        app
    }
    #[test]
    fn arrival_is_independent_of_frame_rate_and_retarget_replaces_route() {
        for dt in [1.0 / 30.0, 1.0 / 60.0, 1.0 / 144.0] {
            let mut app = simulation(dt);
            let entity = app
                .world_mut()
                .spawn((
                    Transform::from_xyz(-55.0, 0.8, 0.0),
                    Movement { speed: 7.0 },
                    MoveTarget(Vec3::new(55.0, 0.0, 0.0)),
                    UnitKind::Tank,
                ))
                .id();
            for _ in 0..(4.0 / dt) as usize {
                app.update();
            }
            app.world_mut()
                .entity_mut(entity)
                .insert(MoveTarget(Vec3::new(-45.0, 0.0, -20.0)))
                .remove::<Route>();
            for _ in 0..(30.0 / dt) as usize {
                app.update();
            }
            let entity = app.world().entity(entity);
            assert!(entity.get::<MoveTarget>().is_none());
            assert!(
                entity
                    .get::<Transform>()
                    .unwrap()
                    .translation
                    .distance(Vec3::new(-45.0, 0.8, -20.0))
                    < 0.001
            );
        }
    }
    #[test]
    fn planning_is_bounded_and_failures_stop_units() {
        let mut app = simulation(1.0 / 60.0);
        // Off-map goals can never plan: failures stop units and count.
        let unreachable = Vec3::new(HALF_SIZE + 50.0, 0.0, 0.0);
        for _ in 0..PATHS_PER_FRAME + 1 {
            app.world_mut().spawn((
                Transform::from_xyz(-55.0, 0.8, 0.0),
                Movement { speed: 7.0 },
                MoveTarget(unreachable),
                UnitKind::Tank,
            ));
        }
        app.update();
        assert_eq!(
            app.world().resource::<NavigationStats>().failed,
            PATHS_PER_FRAME
        );
        app.update();
        assert_eq!(
            app.world().resource::<NavigationStats>().failed,
            PATHS_PER_FRAME + 1
        );
        assert_eq!(
            app.world_mut()
                .query::<&MoveTarget>()
                .iter(app.world())
                .count(),
            0
        );
    }
}
