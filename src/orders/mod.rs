use bevy::{prelude::*, window::PrimaryWindow};

pub mod lines;

use crate::{
    camera::RtsCamera,
    combat::{AttackTarget, Chasing, HoldFire},
    movement::{MoveTarget, queue_move},
    navigation::{NavGrid, Route},
    picking::ground_position,
    scenario::Scenario,
    selection::{Selected, SelectionSystems},
    units::{Team, Unit},
};

pub struct OrderPlugin;

impl Plugin for OrderPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(FormationSettings { spacing: 2.5 })
            .add_plugins(lines::LinesPlugin)
            .add_systems(
                PostUpdate,
                (
                    issue_move_order,
                    issue_attack_move_key,
                    issue_hold_stop_keys,
                )
                    .after(SelectionSystems),
            );
    }
}

#[derive(Resource)]
pub struct FormationSettings {
    pub spacing: f32,
}

/// Current unit intent. A single enum avoids contradictory flag sets like
/// `is_moving` + `is_attacking`. Capabilities (Weapon, Health, ...) never
/// imply intent: only the order decides whether combat is allowed.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub enum UnitOrder {
    Idle,
    Move { destination: Vec3 },
    Attack { target: Entity },
    AttackMove { destination: Vec3 },
    HoldPosition,
}

/// Behaviour rule: which orders may acquire enemies on their own.
/// Beyond-All-Reason style: every order acquires — `Move` fires on the
/// march without chasing, `Idle` defends itself in place. Only units
/// without nearby enemies (or without weapons) stay quiet.
pub fn allows_auto_targeting(order: &UnitOrder) -> bool {
    matches!(
        order,
        UnitOrder::AttackMove { .. }
            | UnitOrder::Move { .. }
            | UnitOrder::HoldPosition
            | UnitOrder::Idle
    )
}

/// Behaviour rule: only orders that close distance may steer toward their
/// target. Holders acquire and fire in place instead of chasing.
pub fn allows_chase(order: &UnitOrder) -> bool {
    matches!(
        order,
        UnitOrder::Attack { .. } | UnitOrder::AttackMove { .. }
    )
}

/// Display colour per order: Move green, Attack red, Hold blue, Idle dim
/// yellow. Shared by order lines, destination markers and route flashes.
pub fn order_color(order: &UnitOrder) -> Color {
    match order {
        UnitOrder::Move { .. } => Color::srgb(0.3, 1.0, 0.4),
        UnitOrder::AttackMove { .. } | UnitOrder::Attack { .. } => Color::srgb(1.0, 0.35, 0.15),
        UnitOrder::HoldPosition => Color::srgb(0.35, 0.6, 1.0),
        UnitOrder::Idle => Color::srgb(0.7, 0.7, 0.25),
    }
}

fn issue_move_order(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<RtsCamera>>,
    selected: Query<(Entity, &Unit), With<Selected>>,
    settings: Res<FormationSettings>,
    grid: Res<NavGrid>,
) {
    if !window.focused || !mouse.just_pressed(MouseButton::Right) || selected.is_empty() {
        return;
    }
    let (camera, transform) = *camera;
    let Some(center) = window
        .cursor_position()
        .and_then(|cursor| ground_position(camera, transform, cursor))
    else {
        return;
    };
    let mut units: Vec<_> = selected.iter().collect();
    units.sort_unstable_by_key(|(_, unit)| unit.0);
    let Some(slots) = grid.formation(units.len(), center, settings.spacing) else {
        return;
    };
    for (slot, (entity, _)) in slots.into_iter().zip(units) {
        queue_move(&mut commands.entity(entity), slot);
    }
}

/// Demo shortcut: selected units attack-move toward the enemy side.
/// Right click keeps issuing plain `Move` orders.
fn issue_attack_move_key(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    scenario: Res<Scenario>,
    selected: Query<(Entity, &Team), With<Selected>>,
) {
    if !window.focused || !keys.just_pressed(KeyCode::KeyG) || selected.is_empty() {
        return;
    }
    for (entity, team) in &selected {
        queue_attack_move(
            &mut commands.entity(entity),
            scenario.attack_target(team.0 as usize),
        );
    }
}

/// Demo shortcut: H holds selected units in place, S stops them outright.
fn issue_hold_stop_keys(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    selected: Query<Entity, With<Selected>>,
) {
    if !window.focused || selected.is_empty() {
        return;
    }
    if keys.just_pressed(KeyCode::KeyH) {
        for entity in &selected {
            queue_hold(&mut commands.entity(entity));
        }
    } else if keys.just_pressed(KeyCode::KeyS) {
        for entity in &selected {
            queue_stop(&mut commands.entity(entity));
        }
    }
}

/// Explicit attack foundation: chase the given target and fire.
/// When the target dies the order completes instead of roaming.
/// Reserved for future direct-attack input; attack-move is the v0.0.3 demo.
#[allow(dead_code)]
pub fn queue_attack(entity: &mut EntityCommands, target: Entity) {
    entity
        .insert(UnitOrder::Attack { target })
        .remove::<(Route, Chasing, HoldFire)>();
}

/// Attack-move foundation: the strategic destination is stored in the order
/// itself, so engaging a temporary target never loses it.
pub fn queue_attack_move(entity: &mut EntityCommands, destination: Vec3) {
    entity
        .insert((
            UnitOrder::AttackMove { destination },
            MoveTarget(destination),
        ))
        .remove::<(AttackTarget, Route, Chasing, HoldFire)>();
}

/// Hold in place: keep any temporary target cleared and never take a route.
/// The unit acquires enemies and fires from its position without chasing.
pub fn queue_hold(entity: &mut EntityCommands) {
    entity
        .insert(UnitOrder::HoldPosition)
        .remove::<(MoveTarget, Route, AttackTarget, Chasing, HoldFire)>();
}

/// Stop: clear intent and any temporary combat state.
pub fn queue_stop(entity: &mut EntityCommands) {
    entity
        .insert(UnitOrder::Idle)
        .remove::<(MoveTarget, Route, AttackTarget, Chasing, HoldFire)>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_targeting_matrix() {
        assert!(!allows_auto_targeting(&UnitOrder::Attack {
            target: Entity::from_bits(9)
        }));
        for order in [
            UnitOrder::AttackMove {
                destination: Vec3::ZERO,
            },
            UnitOrder::Move {
                destination: Vec3::ZERO,
            },
            UnitOrder::HoldPosition,
            UnitOrder::Idle,
        ] {
            assert!(allows_auto_targeting(&order), "{order:?} must acquire");
        }
    }

    #[test]
    fn only_chase_orders_close_distance() {
        assert!(allows_chase(&UnitOrder::Attack {
            target: Entity::from_bits(9)
        }));
        assert!(allows_chase(&UnitOrder::AttackMove {
            destination: Vec3::ZERO
        }));
        assert!(!allows_chase(&UnitOrder::HoldPosition));
        assert!(!allows_chase(&UnitOrder::Move {
            destination: Vec3::ZERO
        }));
        assert!(!allows_chase(&UnitOrder::Idle));
    }

    #[test]
    fn orders_form_a_single_contradiction_free_intent() {
        // One enum variant at a time: a unit can never be both
        // "moving" and "attack-moving" the way separate flags would allow.
        let order = UnitOrder::Move {
            destination: Vec3::X,
        };
        assert_ne!(
            order,
            UnitOrder::AttackMove {
                destination: Vec3::X
            }
        );
        // Marching units acquire, but never chase (see allows_chase).
        assert!(allows_auto_targeting(&order));
        assert!(!allows_chase(&order));
    }
}
