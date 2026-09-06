use crate::{
    combat::AttackTarget,
    navigation::{PlanPaths, Route},
    orders::UnitOrder,
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

/// A plain move order: go to the destination and ignore enemies.
/// Replaces any attack-move intent and drops temporary combat targets.
pub fn queue_move(entity: &mut EntityCommands, destination: Vec3) {
    entity
        .insert((UnitOrder::Move { destination }, MoveTarget(destination)))
        .remove::<(Route, crate::combat::AttackTarget)>();
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
            Option<&mut Route>,
        ),
        Without<AttackTarget>,
    >,
) {
    for (entity, mut transform, movement, target, route) in &mut units {
        match route {
            Some(mut route) => {
                let mut remaining = movement.speed * time.delta_secs();
                while route.next < route.points.len() {
                    let destination = route.points[route.next].with_y(transform.translation.y);
                    let offset = destination - transform.translation;
                    let distance = offset.length();
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
                let step = movement.speed * time.delta_secs();
                let goal = target.0.with_y(transform.translation.y);
                let offset = goal - transform.translation;
                let distance = offset.length();
                if distance <= step {
                    transform.translation = goal;
                    commands.entity(entity).remove::<MoveTarget>();
                } else if distance > f32::EPSILON {
                    transform.translation += offset / distance * step;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::navigation::{NavigationPlugin, NavigationStats, PATHS_PER_FRAME};
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    fn simulation(dt: f64) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                dt,
            )))
            .add_plugins((NavigationPlugin, MovementPlugin));
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
        for _ in 0..PATHS_PER_FRAME + 1 {
            app.world_mut().spawn((
                Transform::from_xyz(-55.0, 0.8, 0.0),
                Movement { speed: 7.0 },
                MoveTarget(Vec3::ZERO),
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
