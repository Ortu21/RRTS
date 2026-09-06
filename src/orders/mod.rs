use bevy::{prelude::*, window::PrimaryWindow};

pub mod lines;

use crate::{
    camera::RtsCamera,
    combat::{AttackTarget, Chasing, HoldFire},
    movement::{MoveTarget, queue_move},
    navigation::{NavGrid, Route},
    picking::{ground_position, ray_box_distance},
    selection::{Selected, SelectionSystems},
    units::{PLAYER_TEAM, Team, UNIT_HALF_SIZE, Unit},
};

pub struct OrderPlugin;

impl Plugin for OrderPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(FormationSettings { spacing: 2.5 })
            .init_resource::<PendingOrder>()
            .add_plugins(lines::LinesPlugin)
            .add_systems(
                PostUpdate,
                (
                    issue_pending_attack_move,
                    issue_pending_patrol,
                    issue_pending_guard,
                )
                    .before(SelectionSystems),
            )
            .add_systems(
                PostUpdate,
                (
                    issue_move_order,
                    enter_attack_move_targeting,
                    enter_patrol_targeting,
                    enter_guard_targeting,
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

/// BAR-style order targeting: some orders need a ground point picked AFTER
/// pressing their key. While armed, left click issues the order instead of
/// changing selection; ESC or right click disarms. One variant per future
/// targeting order (Patrol, Guard, ...) fits here without touching the
/// selection or combat pipelines.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum PendingOrder {
    #[default]
    None,
    AttackMove,
    Patrol,
    Guard,
}

/// Cap waypoint count per patrol loop: bounds memory and keeps loops readable.
pub const MAX_PATROL_POINTS: usize = 16;

/// Current unit intent. A single enum avoids contradictory flag sets like
/// `is_moving` + `is_attacking`. Capabilities (Weapon, Health, ...) never
/// imply intent: only the order decides whether combat is allowed.
#[derive(Component, Debug, Clone, PartialEq)]
pub enum UnitOrder {
    Idle,
    Move { destination: Vec3 },
    Attack { target: Entity },
    AttackMove { destination: Vec3 },
    HoldPosition,
    Patrol { points: Vec<Vec3>, next: usize },
    Guard { target: Entity },
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
            | UnitOrder::Patrol { .. }
            | UnitOrder::Guard { .. }
    )
}

/// Behaviour rule: only orders that close distance may steer toward their
/// target. Holders acquire and fire in place instead of chasing.
pub fn allows_chase(order: &UnitOrder) -> bool {
    matches!(
        order,
        UnitOrder::Attack { .. }
            | UnitOrder::AttackMove { .. }
            | UnitOrder::Patrol { .. }
            | UnitOrder::Guard { .. }
    )
}

/// Display colour per order: Move green, Attack red, Hold blue, Idle dim
/// yellow, Patrol violet, Guard bright yellow. Shared by order lines,
/// destination markers and route flashes.
pub fn order_color(order: &UnitOrder) -> Color {
    match order {
        UnitOrder::Move { .. } => Color::srgb(0.3, 1.0, 0.4),
        UnitOrder::AttackMove { .. } | UnitOrder::Attack { .. } => Color::srgb(1.0, 0.35, 0.15),
        UnitOrder::HoldPosition => Color::srgb(0.35, 0.6, 1.0),
        UnitOrder::Idle => Color::srgb(0.7, 0.7, 0.25),
        UnitOrder::Patrol { .. } => Color::srgb(0.75, 0.4, 1.0),
        UnitOrder::Guard { .. } => Color::srgb(1.0, 0.85, 0.2),
    }
}

#[allow(clippy::too_many_arguments)]
fn issue_move_order(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<RtsCamera>>,
    selected: Query<(Entity, &Unit, &UnitOrder), With<Selected>>,
    enemies: Query<(Entity, &GlobalTransform, &Team), With<Unit>>,
    settings: Res<FormationSettings>,
    grid: Res<NavGrid>,
    mut pending: ResMut<PendingOrder>,
    mut queues: Query<&mut UnitOrderQueue>,
) {
    if !window.focused || selected.is_empty() {
        return;
    }
    // Right click while targeting only disarms; a second click moves.
    if *pending != PendingOrder::None {
        if mouse.just_pressed(MouseButton::Right) {
            *pending = PendingOrder::None;
        }
        return;
    }
    if !mouse.just_pressed(MouseButton::Right) {
        return;
    }
    let (camera, transform) = *camera;
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    // Enemy under cursor: direct attack with focus fire (Shift queues it
    // behind the live order). Otherwise fall through to the ground move.
    if let Ok(ray) = camera.viewport_to_world(transform, cursor) {
        let candidates: Vec<_> = enemies
            .iter()
            .filter(|(_, _, team)| team.0 != PLAYER_TEAM.0)
            .map(|(entity, transform, _)| (entity, transform.translation()))
            .collect();
        if let Some(target) = pick_enemy_target(&ray, &candidates) {
            let additive = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
            for (entity, _, order) in &selected {
                if additive && is_busy(order, &mut queues, entity) {
                    enqueue_order(
                        &mut commands,
                        &mut queues,
                        entity,
                        UnitOrder::Attack { target },
                    );
                } else {
                    queue_attack(&mut commands.entity(entity), target);
                }
            }
            return;
        }
        // Friendly unit under cursor: guard it (right-click), Shift queues.
        // Self-guard is ignored; if nobody was issued (e.g. right-click on
        // the single selected unit itself), fall through to the ground move
        // so the click still marches instead of being swallowed.
        let wards: Vec<_> = enemies
            .iter()
            .filter(|(_, _, team)| team.0 == PLAYER_TEAM.0)
            .map(|(entity, transform, _)| (entity, transform.translation()))
            .collect();
        if let Some(ward) = pick_enemy_target(&ray, &wards) {
            let additive = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
            let mut issued = false;
            for (entity, _, order) in &selected {
                if entity == ward {
                    continue;
                }
                issued = true;
                if additive && is_busy(order, &mut queues, entity) {
                    enqueue_order(
                        &mut commands,
                        &mut queues,
                        entity,
                        UnitOrder::Guard { target: ward },
                    );
                } else {
                    queue_guard(&mut commands.entity(entity), ward);
                }
            }
            if issued {
                return;
            }
        }
    }
    let Some(center) = ground_position(camera, transform, cursor) else {
        return;
    };
    let mut units: Vec<_> = selected.iter().collect();
    units.sort_unstable_by_key(|(_, unit, _)| unit.0);
    let Some(slots) = grid.formation(units.len(), center, settings.spacing) else {
        return;
    };
    // Shift queues behind the live order; plain click replaces everything.
    let additive = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    for (slot, (entity, _, order)) in slots.into_iter().zip(units) {
        let destination = UnitOrder::Move { destination: slot };
        if additive && is_busy(order, &mut queues, entity) {
            enqueue_order(&mut commands, &mut queues, entity, destination);
        } else {
            queue_move(&mut commands.entity(entity), slot);
        }
    }
}

/// G arms attack-move targeting (toggle): the next left click issues the
/// order at the clicked point instead of changing selection.
fn enter_attack_move_targeting(
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    selected: Query<Entity, With<Selected>>,
    mut pending: ResMut<PendingOrder>,
) {
    if !window.focused || !keys.just_pressed(KeyCode::KeyG) {
        return;
    }
    if *pending == PendingOrder::AttackMove {
        *pending = PendingOrder::None;
    } else if !selected.is_empty() {
        *pending = PendingOrder::AttackMove;
    }
}

/// P arms patrol targeting (toggle): each left click appends a loop
/// waypoint to the selected units' patrol instead of changing selection.
/// Targeting stays armed across clicks so multi-point loops need no
/// re-press; ESC, right click or another order key disarms.
fn enter_patrol_targeting(
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    selected: Query<Entity, With<Selected>>,
    mut pending: ResMut<PendingOrder>,
) {
    if !window.focused || !keys.just_pressed(KeyCode::KeyP) {
        return;
    }
    if *pending == PendingOrder::Patrol {
        *pending = PendingOrder::None;
    } else if !selected.is_empty() {
        *pending = PendingOrder::Patrol;
    }
}

/// T arms guard targeting (toggle): left-click a friendly unit to follow
/// and defend it. ESC or right click disarms.
fn enter_guard_targeting(
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    selected: Query<Entity, With<Selected>>,
    mut pending: ResMut<PendingOrder>,
) {
    if !window.focused || !keys.just_pressed(KeyCode::KeyT) {
        return;
    }
    if *pending == PendingOrder::Guard {
        *pending = PendingOrder::None;
    } else if !selected.is_empty() {
        *pending = PendingOrder::Guard;
    }
}

/// Left click while guard-targeting guards the friendly unit under the
/// cursor. Self-guard is ignored; targeting stays armed on a miss so the
/// player can retry, and disarms after issuing.
#[allow(clippy::too_many_arguments)]
fn issue_pending_guard(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<RtsCamera>>,
    selected: Query<Entity, With<Selected>>,
    friendlies: Query<(Entity, &GlobalTransform, &Team), With<Unit>>,
    mut pending: ResMut<PendingOrder>,
) {
    if *pending != PendingOrder::Guard {
        return;
    }
    if !window.focused || keys.just_pressed(KeyCode::Escape) {
        *pending = PendingOrder::None;
        return;
    }
    if !mouse.just_released(MouseButton::Left) || selected.is_empty() {
        return;
    }
    let (camera, transform) = *camera;
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(transform, cursor) else {
        return;
    };
    let wards: Vec<_> = friendlies
        .iter()
        .filter(|(_, _, team)| team.0 == PLAYER_TEAM.0)
        .map(|(entity, transform, _)| (entity, transform.translation()))
        .collect();
    let Some(ward) = pick_enemy_target(&ray, &wards) else {
        return;
    };
    for entity in &selected {
        if entity != ward {
            queue_guard(&mut commands.entity(entity), ward);
        }
    }
    *pending = PendingOrder::None;
}

/// Left click while patrol-targeting appends the clicked point to every
/// selected unit's own patrol loop (capped). Units without a patrol start
/// one; targeting stays armed for the next point.
#[allow(clippy::too_many_arguments)]
fn issue_pending_patrol(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<RtsCamera>>,
    selected: Query<Entity, With<Selected>>,
    orders: Query<&UnitOrder>,
    mut pending: ResMut<PendingOrder>,
) {
    if *pending != PendingOrder::Patrol {
        return;
    }
    if !window.focused || keys.just_pressed(KeyCode::Escape) {
        *pending = PendingOrder::None;
        return;
    }
    if !mouse.just_released(MouseButton::Left) || selected.is_empty() {
        return;
    }
    let (camera, transform) = *camera;
    let Some(point) = window
        .cursor_position()
        .and_then(|cursor| ground_position(camera, transform, cursor))
    else {
        return;
    };
    for entity in &selected {
        let mut entity_commands = commands.entity(entity);
        match orders.get(entity) {
            Ok(UnitOrder::Patrol { points, next }) => {
                let mut points = points.clone();
                if points.len() < MAX_PATROL_POINTS {
                    points.push(point);
                }
                let leg = points[*next % points.len()];
                // Editing the live loop replaces intent: drop any queue
                // behind the never-completing patrol (dead weight in HUD).
                entity_commands
                    .insert((
                        UnitOrder::Patrol {
                            points,
                            next: *next,
                        },
                        MoveTarget(leg),
                    ))
                    .remove::<UnitOrderQueue>();
            }
            _ => {
                queue_patrol(&mut entity_commands, vec![point]);
            }
        }
    }
}

/// Fresh patrol loop through `points`: head for the first leg immediately.
/// Empty input stops the unit instead of panicking.
pub fn queue_patrol(entity: &mut EntityCommands, points: Vec<Vec3>) {
    let Some(leg) = points.first().copied() else {
        queue_stop(entity);
        return;
    };
    entity
        .insert((UnitOrder::Patrol { points, next: 0 }, MoveTarget(leg)))
        .remove::<(Route, AttackTarget, Chasing, HoldFire, UnitOrderQueue)>();
}

/// Left click while targeting issues the armed order in formation around
/// the clicked point, then disarms. Runs before selection so the click
/// never both issues and reselects.
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
fn issue_pending_attack_move(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<RtsCamera>>,
    selected: Query<(Entity, &Unit, &UnitOrder), With<Selected>>,
    settings: Res<FormationSettings>,
    grid: Res<NavGrid>,
    mut pending: ResMut<PendingOrder>,
    mut queues: Query<&mut UnitOrderQueue>,
) {
    if *pending == PendingOrder::None {
        return;
    }
    if !window.focused || keys.just_pressed(KeyCode::Escape) {
        *pending = PendingOrder::None;
        return;
    }
    if !mouse.just_released(MouseButton::Left) || selected.is_empty() {
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
    units.sort_unstable_by_key(|(_, unit, _)| unit.0);
    let Some(slots) = grid.formation(units.len(), center, settings.spacing) else {
        return;
    };
    // Shift queues behind the live order (G then Shift+click attack-moves
    // next); plain click replaces everything.
    let additive = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    for (slot, (entity, _, order)) in slots.into_iter().zip(units) {
        let destination = UnitOrder::AttackMove { destination: slot };
        if additive && is_busy(order, &mut queues, entity) {
            enqueue_order(&mut commands, &mut queues, entity, destination);
        } else {
            queue_attack_move(&mut commands.entity(entity), slot);
        }
    }
    *pending = PendingOrder::None;
}

/// Demo shortcut: H holds selected units in place, S stops them outright.
/// Either concrete order disarms pending targeting.
fn issue_hold_stop_keys(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    selected: Query<Entity, With<Selected>>,
    mut pending: ResMut<PendingOrder>,
) {
    if !window.focused || selected.is_empty() {
        return;
    }
    if keys.just_pressed(KeyCode::KeyH) {
        for entity in &selected {
            queue_hold(&mut commands.entity(entity));
        }
        *pending = PendingOrder::None;
    } else if keys.just_pressed(KeyCode::KeyS) {
        for entity in &selected {
            queue_stop(&mut commands.entity(entity));
        }
        *pending = PendingOrder::None;
    }
}

/// Per-unit order queue. The head is always the live `UnitOrder`; issuing
/// without Shift replaces it and drops the queue, issuing with Shift
/// appends. When an order completes, the next entry pops automatically.
/// Future orders (Patrol, Guard, ...) ride the same queue with no new
/// components: they only need completion and input hooks.
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct UnitOrderQueue(pub Vec<UnitOrder>);

/// Apply an order's full intent state: order component plus movement and
/// combat resets. Shared by fresh issues and queue pops so both paths
/// establish exactly the same state.
fn apply_order(entity: &mut EntityCommands, order: UnitOrder) {
    match order {
        UnitOrder::Move { destination } | UnitOrder::AttackMove { destination } => {
            entity
                .insert((order, MoveTarget(destination)))
                .remove::<(Route, AttackTarget, Chasing, HoldFire)>();
        }
        UnitOrder::Patrol { .. } => {
            let leg = match &order {
                UnitOrder::Patrol { points, next } if !points.is_empty() => {
                    Some(points[*next % points.len()])
                }
                _ => None,
            };
            match leg {
                Some(destination) => {
                    entity.insert((order, MoveTarget(destination))).remove::<(
                        Route,
                        AttackTarget,
                        Chasing,
                        HoldFire,
                    )>();
                }
                None => {
                    entity.insert(UnitOrder::Idle).remove::<(
                        MoveTarget,
                        Route,
                        AttackTarget,
                        Chasing,
                        HoldFire,
                    )>();
                }
            }
        }
        UnitOrder::Attack { .. } | UnitOrder::HoldPosition | UnitOrder::Idle => {
            entity
                .insert(order)
                .remove::<(MoveTarget, Route, AttackTarget, Chasing, HoldFire)>();
        }
        UnitOrder::Guard { .. } => {
            // Follow target is dynamic: resolve_behaviour seeds and refreshes
            // the MoveTarget from the ward's live position each frame, so
            // the budgeted planner routes around obstacles automatically.
            entity
                .insert(order)
                .remove::<(MoveTarget, Route, AttackTarget, Chasing, HoldFire)>();
        }
    }
}

/// Pop the next queued order on completion; Idle (fully stopped) when the
/// queue is empty. Called wherever an order can complete: arrivals in
/// `resolve_behaviour`, destroyed explicit targets in validation.
pub fn complete_order(
    commands: &mut Commands,
    entity: Entity,
    queues: &mut Query<&mut UnitOrderQueue>,
) {
    let next = queues
        .get_mut(entity)
        .ok()
        .and_then(|mut queue| (!queue.0.is_empty()).then(|| queue.0.remove(0)));
    match next {
        // Fresh intent: full state setup like a new issue.
        Some(order) => apply_order(&mut commands.entity(entity), order),
        // No queue: plain Idle, keeping any live lock so an arrived unit
        // defends itself without a re-acquisition gap.
        None => {
            commands.entity(entity).insert(UnitOrder::Idle);
        }
    }
}

/// Append an order behind the live one, creating the queue on demand.
/// Callers decide replace-vs-append (Shift key); busy means live order
/// other than Idle or a non-empty queue.
pub fn enqueue_order(
    commands: &mut Commands,
    queues: &mut Query<&mut UnitOrderQueue>,
    entity: Entity,
    order: UnitOrder,
) {
    match queues.get_mut(entity) {
        Ok(mut queue) => queue.0.push(order),
        Err(_) => {
            commands.entity(entity).insert(UnitOrderQueue(vec![order]));
        }
    }
}

pub fn is_busy(order: &UnitOrder, queues: &mut Query<&mut UnitOrderQueue>, entity: Entity) -> bool {
    !matches!(order, UnitOrder::Idle)
        || queues
            .get_mut(entity)
            .map(|queue| !queue.0.is_empty())
            .unwrap_or(false)
}

/// Nearest enemy unit under a pick ray, if any. Pure over snapshots so the
/// focus-fire rule is unit-testable without a window or camera.
pub fn pick_enemy_target(ray: &Ray3d, enemies: &[(Entity, Vec3)]) -> Option<Entity> {
    enemies
        .iter()
        .filter_map(|(entity, position)| {
            ray_box_distance(ray, *position, UNIT_HALF_SIZE).map(|distance| (*entity, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(entity, _)| entity)
}

/// Explicit attack foundation: chase the given target and fire.
/// When the target dies the order completes instead of roaming.
pub fn queue_attack(entity: &mut EntityCommands, target: Entity) {
    apply_order(entity, UnitOrder::Attack { target });
    entity.remove::<UnitOrderQueue>();
}

/// Attack-move foundation: the strategic destination is stored in the order
/// itself, so engaging a temporary target never loses it.
pub fn queue_attack_move(entity: &mut EntityCommands, destination: Vec3) {
    apply_order(entity, UnitOrder::AttackMove { destination });
    entity.remove::<UnitOrderQueue>();
}

/// Guard a friendly unit: follow it at close range and engage enemies near the ward.
/// Acquire within the guard's AcquisitionRange of both itself and the ward;
/// release beyond 1.5x that radius from either unit.
/// Resume following when the engagement ends.
pub fn queue_guard(entity: &mut EntityCommands, target: Entity) {
    apply_order(entity, UnitOrder::Guard { target });
    entity.remove::<UnitOrderQueue>();
}

/// Hold in place: keep any temporary target cleared and never take a route.
/// The unit acquires enemies and fires from its position without chasing.
pub fn queue_hold(entity: &mut EntityCommands) {
    apply_order(entity, UnitOrder::HoldPosition);
    entity.remove::<UnitOrderQueue>();
}

/// Stop: clear intent and any temporary combat state.
pub fn queue_stop(entity: &mut EntityCommands) {
    apply_order(entity, UnitOrder::Idle);
    entity.remove::<UnitOrderQueue>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_order_starts_disarmed() {
        assert_eq!(PendingOrder::default(), PendingOrder::None);
    }

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
            UnitOrder::Patrol {
                points: vec![Vec3::ZERO],
                next: 0,
            },
            UnitOrder::Guard {
                target: Entity::from_bits(9),
            },
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
        assert!(allows_chase(&UnitOrder::Patrol {
            points: vec![Vec3::ZERO],
            next: 0,
        }));
        assert!(allows_chase(&UnitOrder::Guard {
            target: Entity::from_bits(9)
        }));
        assert!(!allows_chase(&UnitOrder::HoldPosition));
        assert!(!allows_chase(&UnitOrder::Move {
            destination: Vec3::ZERO
        }));
        assert!(!allows_chase(&UnitOrder::Idle));
    }

    #[test]
    fn pick_enemy_returns_nearest_hit_and_ignores_misses() {
        use bevy::math::{Dir3, Ray3d};
        let ray = Ray3d::new(Vec3::new(0.0, 0.0, 5.0), Dir3::NEG_Z);
        let far = Entity::from_bits(1);
        let near = Entity::from_bits(2);
        let off = Entity::from_bits(3);
        let enemies = vec![
            (far, Vec3::new(0.0, 0.0, -10.0)),
            (near, Vec3::ZERO),
            (off, Vec3::new(50.0, 0.0, 0.0)),
        ];
        assert_eq!(pick_enemy_target(&ray, &enemies), Some(near));
        assert_eq!(pick_enemy_target(&ray, &[]), None);
    }

    #[test]
    fn patrol_cap_bounds_loop_memory() {
        const { assert!(MAX_PATROL_POINTS >= 2) }
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
