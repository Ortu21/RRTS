use bevy::prelude::*;

use crate::{
    fog::{VisibilityMap, can_target},
    movement::{ARRIVE_RADIUS, MoveTarget, Movement, MovementSystems, face_toward},
    navigation::Route,
    orders::{UnitOrder, UnitOrderQueue, allows_auto_targeting, allows_chase, complete_order},
    spatial::SpatialGrid,
    units::{Builder, CollisionRadius, Team, Unit, UnitKind},
};

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CombatClock>()
            .configure_sets(
                Update,
                (ValidateTargets, AcquireTargets, ResolveBehaviour)
                    .chain()
                    .before(crate::navigation::PlanPaths),
            )
            .configure_sets(
                Update,
                (ChaseTargets, WeaponSystems, ProjectileSystems, DeathSystems)
                    .chain()
                    .after(MovementSystems),
            )
            .add_systems(Startup, setup_projectile_assets)
            .add_systems(
                Update,
                (
                    (validate_guards, validate_targets)
                        .chain()
                        .in_set(ValidateTargets),
                    acquire_targets.in_set(AcquireTargets),
                    resolve_behaviour.in_set(ResolveBehaviour),
                    chase_targets.in_set(ChaseTargets),
                    (
                        traverse_turrets,
                        traverse_secondary,
                        tick_cooldowns,
                        fire_weapons,
                        fire_secondary,
                    )
                        .chain()
                        .in_set(WeaponSystems),
                    move_projectiles.in_set(ProjectileSystems),
                    process_deaths.in_set(DeathSystems),
                ),
            );
    }
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ValidateTargets;
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct AcquireTargets;
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ResolveBehaviour;
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChaseTargets;
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct WeaponSystems;
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProjectileSystems;
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DeathSystems;

/// Extra leash margin before an automatic chaser drops a distant target.
/// Hysteresis avoids acquire/drop flicker at the acquisition boundary.
const LEASH_MULTIPLIER: f32 = 1.5;
const PROJECTILE_HIT_RADIUS: f32 = 0.7;
/// Holders never close distance, so a lock outside weapon reach is useless:
/// dropping it frees the slot for closer acquisitions instead of holding a
/// target the unit can never shoot.
const HOLD_MARGIN: f32 = 1.25;
/// Follow distance: guards hold inside this radius of their ward instead
/// of stacking onto it, and resume following outside of it.
pub const GUARD_RADIUS: f32 = 6.0;
/// Chase replan threshold: when an explicit Attack target drifts this far
/// from the synced MoveTarget, refresh it and drop the stale Route so the
/// budgeted planner routes to the target's live position (obstacle-aware
/// pursuit instead of a straight-line chase through walls).
pub const CHASE_REPLAN_DISTANCE: f32 = 4.0;
/// Hysteresis for target re-evaluation (squared distances): switch locks
/// only when the candidate is clearly closer, to avoid thrash between
/// equidistant enemies.
const RETARGET_HYSTERESIS_SQ: f32 = 0.36;

/// Decides whether a locked target should be replaced by a closer candidate.
/// Without re-evaluation a unit chases its first lock forever, running past
/// nearer enemies: packed battles decay into tail-chasing with almost no
/// time spent inside weapon range. A candidate inside weapon range while the
/// current lock sits outside always wins, so units prefer shooting.
pub fn should_retarget(current_sq: f32, candidate_sq: f32, weapon_range_sq: f32) -> bool {
    if candidate_sq >= current_sq {
        return false;
    }
    candidate_sq <= current_sq * RETARGET_HYSTERESIS_SQ
        || (candidate_sq <= weapon_range_sq && current_sq > weapon_range_sq)
}
/// Full acquisition scans are staggered across frames: each unit scans when
/// `(tick + entity index) % STRIDE == 0`. At 20k units a global scan every
/// frame would cost millions of spatial lookups per tick; a ≤5-frame delay
/// is imperceptible and keeps the per-tick cost O(n) with a small constant.
pub const ACQUIRE_STRIDE: u64 = 5;

/// Simulation tick counter driving staggered acquisition.
#[derive(Resource, Default)]
pub struct CombatClock {
    pub tick: u64,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

pub fn apply_damage_to(current: f32, damage: f32) -> f32 {
    (current - damage).max(0.0)
}

pub fn is_dead(health: &Health) -> bool {
    health.current <= 0.0
}

#[derive(Component, Debug, Clone, Copy)]
pub struct Weapon {
    pub range: f32,
    pub cooldown: f32,
    pub damage: f32,
    pub projectile_speed: f32,
}

#[derive(Component, Debug, Clone, Copy, Default)]
pub struct WeaponState {
    pub remaining: f32,
}

/// Secondary weapon (Commander missiles, prova). Shares the same
/// `AttackTarget` lock as the primary (mitra): no separate acquisition pass.
/// Own range/cooldown/yaw so the two guns feel different and can be tuned
/// independently. Upgrade hook: fields are components, future levels just
/// swap values.
#[derive(Component, Debug, Clone, Copy)]
pub struct SecondaryWeapon {
    pub range: f32,
    pub cooldown: f32,
    pub damage: f32,
    pub projectile_speed: f32,
}

#[derive(Component, Debug, Clone, Copy, Default)]
pub struct SecondaryWeaponState {
    pub remaining: f32,
}

/// Turret world yaw for the secondary gun, owned by the simulation like
/// `TurretYaw`. Visual barrel mirrors it; fire is gated on alignment.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct SecondaryTurretYaw(pub f32);

/// Deterministic cooldown phase in [0, 1): spreads first volleys over time
/// so damage arrives smoothly instead of synchronized waves. Pure function
/// of the numeric unit id, so repeated runs stay deterministic.
pub fn desync_phase(id: u32) -> f32 {
    ((id as u64).wrapping_mul(0x9E3779B97F4A7C15) % 1000) as f32 / 1000.0
}

/// Acquisition range is deliberately separate from weapon range.
#[derive(Component, Debug, Clone, Copy)]
pub struct AcquisitionRange(pub f32);

/// Temporary combat target. Never replaces the unit's `UnitOrder`:
/// when it is removed, the behaviour resumes the standing order.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttackTarget(pub Entity);

/// Turret world yaw, owned by the simulation. The visual barrel mirrors it;
/// fire is gated on it (see aim tolerance), so traverse rate is real DPS
/// handling rather than decoration.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct TurretYaw(pub f32);

/// Locomotion arbitration markers, resolved once per frame by
/// `resolve_behaviour` so executors never duplicate order logic:
/// - `Chasing`: steer directly at the target (out of range, chase order).
/// - `HoldFire`: stand and shoot (in range with a stop-to-fight order).
/// - neither: follow routes / beeline / hold naturally, firing whenever a
///   valid target is in weapon range.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chasing;
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoldFire;

#[derive(Component, Debug, Clone, Copy)]
pub struct Projectile {
    pub target: Entity,
    pub speed: f32,
    pub damage: f32,
}

pub fn in_weapon_range(from: Vec3, to: Vec3, range: f32) -> bool {
    from.distance_squared(to) <= range * range
}

fn target_position(grid: &SpatialGrid, target: Entity) -> Option<Vec3> {
    grid.position(target)
}

/// Guard ward validation, separate from enemy-lock validation: the ward
/// lives in the order (not the `AttackTarget` slot, which keeps enemy
/// locks), so this must visit every guard with or without a lock.
/// `validate_targets` only sees units carrying `AttackTarget` — a guard
/// holding near its ward has none, and ward death would go unnoticed.
/// A dead, despawned or hostile ward completes the order instead of
/// following a ghost. Health/team queries (not the spatial grid) are
/// authoritative, so completion never depends on index timing.
fn validate_guards(
    mut commands: Commands,
    health: Query<&Health>,
    teams: Query<&Team>,
    units: Query<(Entity, &UnitOrder), With<Unit>>,
    mut queues: Query<&mut UnitOrderQueue>,
) {
    for (entity, order) in &units {
        let UnitOrder::Guard { target: ward } = order else {
            continue;
        };
        let ward_valid = health
            .get(*ward)
            .ok()
            .filter(|health| !is_dead(health))
            .is_some()
            && teams
                .get(*ward)
                .ok()
                .zip(teams.get(entity).ok())
                .is_some_and(|(ward_team, own_team)| !own_team.is_enemy(*ward_team));
        if !ward_valid {
            commands.entity(entity).remove::<AttackTarget>();
            complete_order(&mut commands, entity, &mut queues);
        }
    }
}

/// Targeting is a service, not a behaviour: it only answers requests from
/// orders that allow automatic acquisition, and only returns live enemies.
#[allow(clippy::too_many_arguments)]
fn validate_targets(
    mut commands: Commands,
    grid: Res<SpatialGrid>,
    health: Query<&Health>,
    teams: Query<&Team>,
    ranges: Query<&AcquisitionRange>,
    weapons: Query<&Weapon>,
    secondary: Query<&SecondaryWeapon>,
    map: Option<Res<VisibilityMap>>,
    units: Query<(Entity, &Transform, &UnitOrder, &AttackTarget), With<Unit>>,
    mut queues: Query<&mut UnitOrderQueue>,
) {
    for (entity, transform, order, target) in &units {
        // Fog: locks need team visibility. Headless harnesses without fog
        // data stay open (see can_target); in game every team registers on
        // the first fog tick.
        let seen = target_position(&grid, target.0).is_some_and(|p| {
            teams
                .get(entity)
                .ok()
                .is_some_and(|own| can_target(map.as_deref(), own.0, p))
        });
        let valid = seen
            && match target_position(&grid, target.0)
                .and_then(|_| health.get(target.0).ok())
                .filter(|health| !is_dead(health))
                .and(teams.get(target.0).ok())
            {
                Some(target_team) => {
                    let own_team = teams.get(entity).ok();
                    match (own_team, order) {
                        (
                            Some(own),
                            UnitOrder::AttackMove { .. }
                            | UnitOrder::Move { .. }
                            | UnitOrder::Patrol { .. }
                            | UnitOrder::Guard { .. }
                            | UnitOrder::Build { .. },
                        ) if own.is_enemy(*target_team) => {
                            // Leash: drop targets left far behind (marching past)
                            // or chased far outside acquisition.
                            match (ranges.get(entity).ok(), target_position(&grid, target.0)) {
                                (Some(range), Some(target_position)) => {
                                    let leash = range.0 * LEASH_MULTIPLIER;
                                    transform.translation.distance_squared(target_position)
                                        <= leash * leash
                                        && match order {
                                            UnitOrder::Guard { target: ward } => {
                                                grid.position(*ward).is_some_and(|ward_position| {
                                                    ward_position.distance_squared(target_position)
                                                        <= leash * leash
                                                })
                                            }
                                            _ => true,
                                        }
                                }
                                _ => true,
                            }
                        }
                        (Some(own), UnitOrder::Attack { target: ordered })
                            if own.is_enemy(*target_team) && target.0 == *ordered =>
                        {
                            true
                        }
                        (Some(own), UnitOrder::HoldPosition | UnitOrder::Idle)
                            if own.is_enemy(*target_team) =>
                        {
                            // Static defenders never close distance: keep the lock
                            // while ANY gun reaches (plus margin). Dual-gun units
                            // hold missile locks beyond mitra range.
                            match (weapons.get(entity).ok(), target_position(&grid, target.0)) {
                                (Some(weapon), Some(target_position)) => {
                                    let reach = secondary
                                        .get(entity)
                                        .ok()
                                        .map_or(weapon.range, |s| weapon.range.max(s.range));
                                    transform.translation.distance(target_position)
                                        <= reach * HOLD_MARGIN
                                }
                                _ => true,
                            }
                        }
                        _ => false,
                    }
                }
                None => false,
            };
        if !valid {
            commands.entity(entity).remove::<AttackTarget>();
            if matches!(order, UnitOrder::Attack { .. }) {
                // Explicit attack order completes instead of roaming.
                complete_order(&mut commands, entity, &mut queues);
            }
        }
    }
}

/// Targeting service: automatic acquisition for targetless units plus
/// periodic lock re-evaluation for engaged ones, in a single pass. One
/// query iteration instead of two; each unit scans at most once per
/// stagger tick either way, so the scan budget is unchanged.
///
/// Without re-evaluation a unit chases its first lock forever, running
/// past nearer enemies: packed battles decay into tail-chasing with almost
/// no time spent inside weapon range.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn acquire_targets(
    mut commands: Commands,
    grid: Res<SpatialGrid>,
    mut clock: ResMut<CombatClock>,
    weapons: Query<&Weapon>,
    map: Option<Res<VisibilityMap>>,
    units: Query<
        (
            Entity,
            &Transform,
            &Team,
            &UnitOrder,
            &AcquisitionRange,
            Option<&AttackTarget>,
        ),
        (With<Unit>, With<Weapon>),
    >,
    candidates: Query<(&Team, &Health)>,
    mut queues: Query<&mut UnitOrderQueue>,
) {
    clock.tick += 1;
    for (entity, transform, team, order, range, target) in &units {
        if let UnitOrder::Attack { target } = order {
            // Explicit order, not automatic acquisition — but still gated on
            // team visibility: no firing at what nobody sees.
            let valid = candidates
                .get(*target)
                .is_ok_and(|(target_team, health)| team.is_enemy(*target_team) && !is_dead(health))
                && grid
                    .position(*target)
                    .is_some_and(|p| can_target(map.as_deref(), team.0, p));
            if valid {
                commands.entity(entity).insert(AttackTarget(*target));
            } else {
                complete_order(&mut commands, entity, &mut queues);
            }
            continue;
        }
        if !allows_auto_targeting(order) {
            continue;
        }
        if !(clock.tick + entity.to_bits()).is_multiple_of(ACQUIRE_STRIDE) {
            continue;
        }
        // Guard hysteresis is centered on the ward: new locks (including
        // retargets) must be inside its acquisition radius; validation lets
        // existing locks persist out to 1.5x. Filtering during the scan also
        // finds eligible threats hidden behind a closer out-of-bounds enemy.
        let ward_position = if let UnitOrder::Guard { target: ward } = order {
            let Some(position) = grid.position(*ward) else {
                continue;
            };
            Some(position)
        } else {
            None
        };
        let near_ward = |position: Vec3| {
            ward_position.is_none_or(|ward| ward.distance_squared(position) <= range.0 * range.0)
        };
        // Fog: only team-visible candidates may be (re)acquired. Headless
        // harnesses without fog data stay open via can_target.
        let in_sight = |position: Vec3| can_target(map.as_deref(), team.0, position);
        match target {
            None => {
                if let Some((target, _)) = nearest_enemy(
                    &grid,
                    &candidates,
                    team,
                    transform.translation,
                    range.0,
                    entity,
                    |_, position| near_ward(position) && in_sight(position),
                ) {
                    commands.entity(entity).insert(AttackTarget(target));
                }
            }
            Some(target) => {
                // Retarget every auto-acquiring order except transient Idle:
                // marchers walking past enemies need fresh locks as much as
                // chasers. Explicit `Attack` never reaches here (handled
                // above); invalid locks remain owned by validation.
                if !matches!(
                    order,
                    UnitOrder::AttackMove { .. }
                        | UnitOrder::Move { .. }
                        | UnitOrder::HoldPosition
                        | UnitOrder::Patrol { .. }
                        | UnitOrder::Guard { .. }
                        | UnitOrder::Build { .. }
                ) {
                    continue;
                }
                let Some(current_position) = target_position(&grid, target.0) else {
                    continue;
                };
                let current_sq = transform.translation.distance_squared(current_position);
                let weapon_range_sq = weapons
                    .get(entity)
                    .map(|weapon| weapon.range.powi(2))
                    .unwrap_or(f32::MAX);
                // Only strictly nearer candidates can win (see
                // `should_retarget`), so shrink the scan to the current lock
                // distance: wide scans happen only for stale far locks,
                // settled melee costs almost nothing.
                let radius = range.0.min(current_sq.sqrt() + 0.01);
                if let Some((candidate, _)) = nearest_enemy(
                    &grid,
                    &candidates,
                    team,
                    transform.translation,
                    radius,
                    entity,
                    |candidate, position| {
                        candidate != target.0 && near_ward(position) && in_sight(position)
                    },
                )
                .filter(|(_, candidate_sq)| {
                    should_retarget(current_sq, *candidate_sq, weapon_range_sq)
                }) {
                    commands.entity(entity).insert(AttackTarget(candidate));
                }
            }
        }
    }
}

/// Nearest live enemy within `range` of `position`, excluding `ignore` and
/// candidates rejected by `eligible`. Ties break deterministically.
fn nearest_enemy(
    grid: &SpatialGrid,
    candidates: &Query<(&Team, &Health)>,
    team: &Team,
    position: Vec3,
    range: f32,
    ignore: Entity,
    eligible: impl Fn(Entity, Vec3) -> bool,
) -> Option<(Entity, f32)> {
    let mut best: Option<(Entity, f32)> = None;
    grid.for_each_nearby(position, range, |entry| {
        let candidate = entry.entity;
        if candidate == ignore || !eligible(candidate, entry.position) {
            return;
        }
        let Ok((candidate_team, health)) = candidates.get(candidate) else {
            return;
        };
        if !team.is_enemy(*candidate_team) || is_dead(health) {
            return;
        }
        let Some(candidate_position) = grid.position(candidate) else {
            return;
        };
        let distance_squared = candidate_position.distance_squared(position);
        let replace = match best {
            None => true,
            Some((best_entity, best_distance)) => {
                distance_squared < best_distance
                    || (distance_squared == best_distance
                        && candidate.to_bits() < best_entity.to_bits())
            }
        };
        if replace {
            best = Some((candidate, distance_squared));
        }
    });
    best
}

/// Resolves order intent into locomotion arbitration markers plus movement
/// state. Attack-move keeps its strategic destination in the order while
/// engaging temporary targets. Engaged units deliberately keep their
/// `MoveTarget`/`Route`: route following pauses for them (see `move_units`),
/// so when the engagement ends the previous route resumes instantly without
/// replanning. Dropping routes on every engagement would churn the budgeted
/// path planner and strand units without routes for hundreds of ticks.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn resolve_behaviour(
    mut commands: Commands,
    grid: Res<SpatialGrid>,
    nav: Option<Res<crate::navigation::NavGrid>>,
    gameplay: Option<Res<crate::structures::Placement>>,
    weapons: Query<&Weapon>,
    units: Query<
        (
            Entity,
            &Transform,
            &UnitOrder,
            Option<&AttackTarget>,
            Option<&MoveTarget>,
            Has<Route>,
            Has<Chasing>,
            Has<HoldFire>,
            Option<&Builder>,
            Option<&CollisionRadius>,
        ),
        With<Unit>,
    >,
    sites: Query<
        (
            Entity,
            &Transform,
            &crate::economy::balance::BuildingKind,
            Has<crate::structures::Construction>,
        ),
        With<crate::structures::Building>,
    >,
    mut queues: Query<&mut UnitOrderQueue>,
) {
    for (
        entity,
        transform,
        order,
        target,
        move_target,
        has_route,
        chasing,
        holding,
        builder,
        body,
    ) in &units
    {
        // Stale markers strand units (executors filter on them), so clear
        // them the moment the lock is gone, for every order uniformly.
        if target.is_none() && (chasing || holding) {
            commands.entity(entity).remove::<(Chasing, HoldFire)>();
        }
        match (order, target) {
            (UnitOrder::Idle, None) => {}
            (UnitOrder::Move { destination }, None) => {
                if move_target.is_none() && !has_route {
                    if crate::movement::flat_distance(transform.translation, *destination)
                        <= ARRIVE_RADIUS
                    {
                        complete_order(&mut commands, entity, &mut queues);
                    } else {
                        commands.entity(entity).insert(MoveTarget(*destination));
                    }
                } else if move_target.is_none_or(|current| current.0 != *destination) {
                    commands
                        .entity(entity)
                        .insert(MoveTarget(*destination))
                        .remove::<Route>();
                }
            }
            (UnitOrder::AttackMove { destination }, None) => {
                if move_target.is_none() && !has_route {
                    // Destination reached: the order completes.
                    complete_order(&mut commands, entity, &mut queues);
                } else if move_target.is_none_or(|current| current.0 != *destination) {
                    commands
                        .entity(entity)
                        .insert(MoveTarget(*destination))
                        .remove::<Route>();
                }
            }
            (UnitOrder::Patrol { points, next }, None) => {
                if points.is_empty() {
                    commands.entity(entity).insert(UnitOrder::Idle);
                } else {
                    let leg = points[*next % points.len()];
                    if move_target.is_none() && !has_route {
                        if crate::movement::flat_distance(transform.translation, leg)
                            <= ARRIVE_RADIUS
                        {
                            // Waypoint reached: advance the loop, never complete.
                            let advanced = (*next + 1) % points.len();
                            commands.entity(entity).insert((
                                UnitOrder::Patrol {
                                    points: points.clone(),
                                    next: advanced,
                                },
                                MoveTarget(points[advanced]),
                            ));
                            commands.entity(entity).remove::<Route>();
                        } else {
                            commands.entity(entity).insert(MoveTarget(leg));
                        }
                    } else if move_target.is_none_or(|current| current.0 != leg) {
                        commands
                            .entity(entity)
                            .insert(MoveTarget(leg))
                            .remove::<Route>();
                    }
                }
            }
            // Guard follow without an enemy lock: stay near the ward via the
            // budgeted planner (obstacle-aware), hold inside GUARD_RADIUS.
            // Never completes on its own; ward death is handled by validation.
            (UnitOrder::Guard { target: ward }, None) => {
                if chasing || holding {
                    commands.entity(entity).remove::<(Chasing, HoldFire)>();
                }
                let Some(ward_position) = target_position(&grid, *ward) else {
                    continue;
                };
                if crate::movement::flat_distance(transform.translation, ward_position)
                    <= GUARD_RADIUS
                {
                    if move_target.is_some() || has_route {
                        commands.entity(entity).remove::<(MoveTarget, Route)>();
                    }
                } else if move_target.is_none_or(|current| {
                    crate::movement::flat_distance(current.0, ward_position) > CHASE_REPLAN_DISTANCE
                }) {
                    commands
                        .entity(entity)
                        .insert(MoveTarget(ward_position))
                        .remove::<Route>();
                }
            }
            // Explicit construction task: hold inside the builder's own
            // radius of the site (power flows there, see economy); march to
            // a footprint-edge stand-off while out of range. Completes when
            // the site finishes or vanishes (cancelled/destroyed). Builders
            // never chase: they hold the site and fire from there.
            (UnitOrder::Build { site }, None) => {
                if chasing || holding {
                    commands.entity(entity).remove::<(Chasing, HoldFire)>();
                }
                let live = sites
                    .get(*site)
                    .ok()
                    .filter(|(_, _, _, under_construction)| *under_construction);
                let Some((_, site_transform, site_kind, _)) = live else {
                    complete_order(&mut commands, entity, &mut queues);
                    continue;
                };
                let radius = builder.map_or(0.0, |b| b.radius);
                if crate::movement::flat_distance(transform.translation, site_transform.translation)
                    <= radius
                {
                    if move_target.is_some() || has_route {
                        commands.entity(entity).remove::<(MoveTarget, Route)>();
                    }
                } else {
                    let body_radius = body.map_or(0.5, |r| r.0);
                    let approach = nav.as_ref().map_or(site_transform.translation, |nav| {
                        crate::structures::site_approach(
                            nav,
                            site_transform.translation,
                            transform.translation,
                            site_kind.stats().half,
                            body_radius,
                        )
                    });
                    if move_target.is_none_or(|current| {
                        crate::movement::flat_distance(current.0, approach) > CHASE_REPLAN_DISTANCE
                    }) {
                        commands
                            .entity(entity)
                            .insert(MoveTarget(approach))
                            .remove::<Route>();
                    }
                }
            }
            // Marching, holding or building with a lock: no markers, the unit
            // follows its route (or holds) and fires whenever the target is
            // in range.
            (
                UnitOrder::Move { .. }
                | UnitOrder::HoldPosition
                | UnitOrder::Idle
                | UnitOrder::Build { .. },
                Some(_),
            ) => {
                if chasing || holding {
                    commands.entity(entity).remove::<(Chasing, HoldFire)>();
                }
            }
            // Stop-to-fight orders: hold position and shoot inside weapon
            // range, chase outside of it. Markers are mutually exclusive.
            (order, Some(target)) if allows_chase(order) => {
                let in_range = weapons
                    .get(entity)
                    .ok()
                    .zip(target_position(&grid, target.0))
                    .is_some_and(|(weapon, target_position)| {
                        in_weapon_range(transform.translation, target_position, weapon.range)
                    });
                if in_range {
                    if chasing {
                        commands.entity(entity).remove::<Chasing>();
                    }
                    if !holding {
                        commands.entity(entity).insert(HoldFire);
                    }
                } else {
                    if holding {
                        commands.entity(entity).remove::<HoldFire>();
                    }
                    if !chasing {
                        commands.entity(entity).insert(Chasing);
                    }
                }
                // Keep pursuit obstacle-aware: explicit Attack chases sync a
                // MoveTarget to the target's live position (dropping stale
                // routes past the threshold) so the planner keeps a fresh
                // route; guards keep theirs pointed at the ward to resume
                // the follow the moment the engagement ends.
                let anchor = if gameplay.is_some() && !in_range {
                    target_position(&grid, target.0)
                        .map(|p| nav.as_ref().map_or(p, |nav| nav.clear_point(p.with_y(0.0))))
                } else {
                    match order {
                        UnitOrder::Attack { .. } => target_position(&grid, target.0),
                        UnitOrder::Guard { target: ward } => target_position(&grid, *ward),
                        _ => None,
                    }
                };
                if let Some(anchor) = anchor {
                    // `is_none_or` covers the missing-target case: a fresh
                    // chase without any MoveTarget always (re)seeds it.
                    if move_target
                        .is_none_or(|current| current.0.distance(anchor) > CHASE_REPLAN_DISTANCE)
                    {
                        commands
                            .entity(entity)
                            .insert(MoveTarget(anchor))
                            .remove::<Route>();
                    }
                }
            }
            // Unreachable: the guard above covers every chase order, but the
            // compiler cannot prove it.
            (
                UnitOrder::AttackMove { .. }
                | UnitOrder::Attack { .. }
                | UnitOrder::Patrol { .. }
                | UnitOrder::Guard { .. },
                Some(_),
            ) => {}
            (UnitOrder::HoldPosition, _) => {
                if move_target.is_some() || has_route {
                    commands.entity(entity).remove::<(MoveTarget, Route)>();
                }
            }
            (UnitOrder::Attack { .. }, None) => {
                complete_order(&mut commands, entity, &mut queues);
            }
        }
    }
}

/// Direct kinematic chase toward the temporary target while outside weapon
/// range. No pathfinding here: strategic routes use the nav grid, combat
/// steering is local. Runs after route movement.
#[allow(clippy::type_complexity)]
fn chase_targets(
    time: Res<Time>,
    grid: Res<SpatialGrid>,
    nav: Option<Res<crate::navigation::NavGrid>>,
    gameplay: Option<Res<crate::structures::Placement>>,
    mut units: Query<
        (
            &mut Transform,
            &Movement,
            &Weapon,
            &AttackTarget,
            &UnitKind,
            Option<&mut Route>,
        ),
        (With<Unit>, With<Chasing>),
    >,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (mut transform, movement, weapon, target, kind, route) in &mut units {
        let Some(target_position) = target_position(&grid, target.0) else {
            continue;
        };
        let mut aim = target_position;
        if gameplay.is_some()
            && nav
                .as_ref()
                .is_some_and(|nav| !nav.segment_clear(transform.translation, target_position))
            && let Some(mut route) = route
        {
            while route.next < route.points.len()
                && route.points[route.next]
                    .xz()
                    .distance(transform.translation.xz())
                    < 1.0
            {
                route.next += 1;
            }
            if let Some(point) = route.points.get(route.next) {
                aim = point.with_y(transform.translation.y);
            }
        }
        let offset = aim - transform.translation;
        if in_weapon_range(transform.translation, target_position, weapon.range) {
            continue;
        }
        let distance = offset.length();
        if distance > f32::EPSILON {
            // Smooth pursuit: turn the hull at its rate and scale speed by
            // alignment, so chasers arc into the target instead of spinning
            // in place or snapping. Full speed when aligned, crawl when the
            // target is directly behind.
            let turn_rate = crate::units::archetype(*kind).hull_turn;
            face_toward(&mut transform, offset, turn_rate, dt);
            let forward = transform.rotation * Vec3::NEG_Z;
            let alignment = forward.xz().dot(offset.xz().normalize_or_zero()).max(0.0);
            let step = movement.speed * (0.35 + 0.65 * alignment) * dt;
            crate::movement::steer(&mut transform, offset, step.min(distance));
        }
    }
}

fn tick_cooldowns(
    time: Res<Time>,
    mut states: Query<&mut WeaponState>,
    mut secondary: Query<&mut SecondaryWeaponState>,
) {
    let dt = time.delta_secs();
    for mut state in &mut states {
        state.remaining = (state.remaining - dt).max(0.0);
    }
    for mut state in &mut secondary {
        state.remaining = (state.remaining - dt).max(0.0);
    }
}

/// Traverse turret yaw toward the current lock (or hull-forward when
/// targetless) at the archetype traverse rate. slowly-traversing kinds
/// genuinely aim slower: fire is gated on alignment (see `fire_weapons`).
#[allow(clippy::type_complexity)]
fn traverse_turrets(
    time: Res<Time>,
    grid: Res<SpatialGrid>,
    mut units: Query<
        (&Transform, &UnitKind, &mut TurretYaw, Option<&AttackTarget>),
        (With<Unit>, With<Weapon>),
    >,
) {
    use crate::movement::{rotate_toward, yaw_toward};
    use crate::units::archetype;

    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (transform, kind, mut turret, target) in &mut units {
        let stats = archetype(*kind);
        let body_yaw = transform.rotation.to_euler(EulerRot::YXZ).0;
        let aim = target
            .and_then(|target| grid.position(target.0))
            .filter(|aim| aim.xz().distance_squared(transform.translation.xz()) > f32::EPSILON)
            .map(|aim| yaw_toward(aim - transform.translation))
            .unwrap_or(body_yaw);
        turret.0 = rotate_toward(turret.0, aim, stats.traverse * dt);
    }
}

/// Secondary traverse: same lock, independent yaw rate from
/// `COMMANDER_MISSILES`. Missiles aim slower, so at close range the mitra
/// fires first while missiles still traverse — double gun feeling.
#[allow(clippy::type_complexity)]
fn traverse_secondary(
    time: Res<Time>,
    grid: Res<SpatialGrid>,
    mut units: Query<
        (&Transform, &mut SecondaryTurretYaw, Option<&AttackTarget>),
        (With<Unit>, With<SecondaryWeapon>),
    >,
) {
    use crate::movement::{rotate_toward, yaw_toward};
    use crate::units::archetype::COMMANDER_MISSILES;

    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (transform, mut turret, target) in &mut units {
        let body_yaw = transform.rotation.to_euler(EulerRot::YXZ).0;
        let aim = target
            .and_then(|target| grid.position(target.0))
            .filter(|aim| aim.xz().distance_squared(transform.translation.xz()) > f32::EPSILON)
            .map(|aim| yaw_toward(aim - transform.translation))
            .unwrap_or(body_yaw);
        turret.0 = rotate_toward(turret.0, aim, COMMANDER_MISSILES.traverse * dt);
    }
}

#[derive(Resource)]
struct ProjectileAssets {
    mesh: Handle<Mesh>,
    team_material: [Handle<StandardMaterial>; 2],
}

fn setup_projectile_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(ProjectileAssets {
        mesh: meshes.add(Sphere::new(0.18)),
        team_material: [
            materials.add(StandardMaterial {
                base_color: Color::srgb(0.4, 0.9, 1.0),
                emissive: Color::srgb(0.2, 0.7, 1.0).into(),
                unlit: true,
                ..default()
            }),
            materials.add(StandardMaterial {
                base_color: Color::srgb(1.0, 0.6, 0.2),
                emissive: Color::srgb(1.0, 0.4, 0.1).into(),
                unlit: true,
                ..default()
            }),
        ],
    });
}

#[allow(clippy::type_complexity)]
fn fire_weapons(
    mut commands: Commands,
    grid: Res<SpatialGrid>,
    assets: Option<Res<ProjectileAssets>>,
    health: Query<&Health>,
    mut shooters: Query<
        (
            &Transform,
            &Team,
            &Weapon,
            &mut WeaponState,
            &AttackTarget,
            &TurretYaw,
            &UnitKind,
        ),
        With<Unit>,
    >,
) {
    let Some(assets) = assets else {
        return;
    };
    for (transform, team, weapon, mut state, target, turret, kind) in &mut shooters {
        if state.remaining > 0.0 {
            continue;
        }
        let Some(target_position) = target_position(&grid, target.0) else {
            continue;
        };
        if health.get(target.0).is_ok_and(is_dead)
            || !in_weapon_range(transform.translation, target_position, weapon.range)
        {
            continue;
        }
        // Fire only when the barrel has traversed onto the target: slow
        // turrets genuinely shoot later, fast ones snap-shoot on the move.
        let stats = crate::units::archetype(*kind);
        let aim = crate::movement::yaw_toward(target_position - transform.translation);
        if crate::movement::wrap_angle(aim - turret.0).abs() > stats.aim_tolerance {
            continue;
        }
        state.remaining = weapon.cooldown;
        // Spawn at the muzzle: forward of the traversing barrel.
        let muzzle = Vec3::new(-turret.0.sin(), 0.0, -turret.0.cos()) * crate::units::MUZZLE_REACH;
        commands.spawn((
            Projectile {
                target: target.0,
                speed: weapon.projectile_speed,
                damage: weapon.damage,
            },
            *team,
            Transform::from_translation(transform.translation + muzzle),
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(assets.team_material[team.0 as usize % 2].clone()),
        ));
    }
}

/// Missile fire for the Commander. Same `AttackTarget` as the mitra but own
/// range/cooldown/yaw gate, so the two weapons overlap without syncing.
/// Prova tuning lives in `COMMANDER_MISSILES`.
#[allow(clippy::type_complexity)]
fn fire_secondary(
    mut commands: Commands,
    grid: Res<SpatialGrid>,
    assets: Option<Res<ProjectileAssets>>,
    health: Query<&Health>,
    mut shooters: Query<
        (
            &Transform,
            &Team,
            &SecondaryWeapon,
            &mut SecondaryWeaponState,
            &AttackTarget,
            &SecondaryTurretYaw,
        ),
        With<Unit>,
    >,
) {
    use crate::units::archetype::COMMANDER_MISSILES;

    let Some(assets) = assets else {
        return;
    };
    for (transform, team, weapon, mut state, target, turret) in &mut shooters {
        if state.remaining > 0.0 {
            continue;
        }
        let Some(target_position) = target_position(&grid, target.0) else {
            continue;
        };
        if health.get(target.0).is_ok_and(is_dead)
            || !in_weapon_range(transform.translation, target_position, weapon.range)
        {
            continue;
        }
        let aim = crate::movement::yaw_toward(target_position - transform.translation);
        if crate::movement::wrap_angle(aim - turret.0).abs() > COMMANDER_MISSILES.aim_tolerance {
            continue;
        }
        state.remaining = weapon.cooldown;
        // Missiles launch higher off the hull so the two muzzles read apart.
        let muzzle = Vec3::new(-turret.0.sin(), 0.0, -turret.0.cos()) * 2.2 + Vec3::Y * 1.6;
        commands.spawn((
            Projectile {
                target: target.0,
                speed: weapon.projectile_speed,
                damage: weapon.damage,
            },
            *team,
            Transform::from_translation(transform.translation + muzzle),
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(assets.team_material[team.0 as usize % 2].clone()),
        ));
    }
}

/// Simple homing projectiles: steer at the target's current position,
/// apply damage on impact, despawn quietly if the target is already gone.
fn move_projectiles(
    mut commands: Commands,
    time: Res<Time>,
    grid: Res<SpatialGrid>,
    mut projectiles: Query<(Entity, &mut Transform, &Projectile)>,
    mut health: Query<&mut Health>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, projectile) in &mut projectiles {
        let Some(target_position) = target_position(&grid, projectile.target) else {
            commands.entity(entity).despawn();
            continue;
        };
        let offset = target_position - transform.translation;
        let distance = offset.length();
        let step = projectile.speed * dt;
        if distance <= step.max(PROJECTILE_HIT_RADIUS) {
            if let Ok(mut target_health) = health.get_mut(projectile.target) {
                target_health.current = apply_damage_to(target_health.current, projectile.damage)
                    .clamp(0.0, target_health.max);
            }
            commands.entity(entity).despawn();
        } else if distance > f32::EPSILON {
            transform.translation += offset / distance * step;
        }
    }
}

fn process_deaths(mut commands: Commands, units: Query<(Entity, &Health)>) {
    for (entity, health) in &units {
        if is_dead(health) {
            // Descendants (selection rings) are despawned automatically.
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        movement::MovementPlugin,
        navigation::{CELL_SIZE, HALF_SIZE, NavGrid, NavigationPlugin},
        scenario::Scenario,
        spatial::SpatialPlugin,
        units::{CollisionRadius, UnitPlugin},
    };
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    /// Headless combat harness on a hermetic open field: gameplay logic
    /// must not depend on the generated map layout.
    fn combat_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                1.0 / 60.0,
            )))
            .insert_resource(Scenario::Benchmark { per_team: 1 })
            .add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .add_plugins((
                NavigationPlugin,
                SpatialPlugin,
                UnitPlugin { visuals: false },
                CombatPlugin,
                MovementPlugin,
            ))
            .insert_resource(NavGrid::new(HALF_SIZE, CELL_SIZE, Vec::new()));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    #[allow(clippy::type_complexity)]
    fn combatant(
        id: u32,
        team: u8,
        position: Vec3,
        order: UnitOrder,
        kind: UnitKind,
    ) -> (
        Unit,
        Team,
        crate::movement::Movement,
        UnitOrder,
        MoveTarget,
        Transform,
        UnitKind,
        CollisionRadius,
        Health,
        Weapon,
        WeaponState,
        AcquisitionRange,
        TurretYaw,
    ) {
        let destination = match order {
            UnitOrder::Move { destination } | UnitOrder::AttackMove { destination } => destination,
            _ => position,
        };
        let stats = crate::units::archetype(kind);
        let (kind_component, radius, health, weapon, weapon_state, acquisition, turret) =
            crate::units::arm_bundle(id, kind);
        (
            Unit(id),
            Team(team),
            crate::movement::Movement { speed: stats.speed },
            order,
            MoveTarget(destination),
            Transform::from_translation(position),
            kind_component,
            radius,
            health,
            weapon,
            weapon_state,
            acquisition,
            turret,
        )
    }

    #[test]
    fn damage_accumulates_and_kills_at_zero() {
        assert_eq!(apply_damage_to(100.0, 10.0), 90.0);
        assert_eq!(apply_damage_to(5.0, 10.0), 0.0);
        assert!(is_dead(&Health {
            current: 0.0,
            max: 100.0
        }));
        assert!(!is_dead(&Health {
            current: 100.0,
            max: 100.0
        }));
    }

    #[test]
    fn weapon_range_is_inclusive_and_uses_full_distance() {
        let from = Vec3::ZERO;
        assert!(in_weapon_range(from, Vec3::new(18.0, 0.0, 0.0), 18.0));
        assert!(!in_weapon_range(from, Vec3::new(18.1, 0.0, 0.0), 18.0));
    }

    /// Fog harness: same combat scene plus the visibility plugin, so locks
    /// need team sight. Plain combat_app (no fog data) stays open.
    fn combat_fog_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                1.0 / 60.0,
            )))
            .insert_resource(Scenario::Benchmark { per_team: 1 })
            .add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .add_plugins((
                NavigationPlugin,
                SpatialPlugin,
                UnitPlugin { visuals: false },
                CombatPlugin,
                MovementPlugin,
                crate::fog::FogPlugin { render: false },
            ))
            .insert_resource(NavGrid::new(HALF_SIZE, CELL_SIZE, Vec::new()));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    #[test]
    fn fog_blocks_locks_until_spotted_then_drops_them() {
        // Tank sights (15m) are shorter than guns (18m): holders 25m apart
        // are mutually blind and must not engage.
        let mut app = combat_fog_app();
        let blue = app
            .world_mut()
            .spawn(combatant(
                600,
                0,
                Vec3::new(-30.0, 0.8, 0.0),
                UnitOrder::HoldPosition,
                UnitKind::Tank,
            ))
            .id();
        let red = app
            .world_mut()
            .spawn(combatant(
                601,
                1,
                Vec3::new(-5.0, 0.8, 0.0),
                UnitOrder::HoldPosition,
                UnitKind::Tank,
            ))
            .id();
        // Warmup activates fog (first 0.25s are open), then 1s blind.
        for _ in 0..20 {
            app.update();
        }
        for _ in 0..60 {
            app.update();
        }
        assert!(app.world().get::<AttackTarget>(blue).is_none());
        assert!(app.world().get::<AttackTarget>(red).is_none());
        // Walked into sight: both lock.
        app.world_mut()
            .get_mut::<Transform>(red)
            .unwrap()
            .translation = Vec3::new(-20.0, 0.8, 0.0);
        for _ in 0..60 {
            app.update();
        }
        assert!(app.world().get::<AttackTarget>(blue).is_some());
        assert!(app.world().get::<AttackTarget>(red).is_some());
        // Gone far beyond sight and leash: locks drop.
        app.world_mut()
            .get_mut::<Transform>(red)
            .unwrap()
            .translation = Vec3::new(70.0, 0.8, 0.0);
        for _ in 0..60 {
            app.update();
        }
        assert!(app.world().get::<AttackTarget>(blue).is_none());
        assert!(app.world().get::<AttackTarget>(red).is_none());
    }

    #[test]
    fn spotter_shares_vision_for_long_guns() {
        // Blue tank at 0, red at 20: beyond the tank's own sight (15) but
        // inside its gun (18) and acquisition (30). A forward engineer at 12
        // (sight 22) spots red for the team, so the tank locks a target its
        // own eyes cannot see — without firing out of range.
        let mut app = combat_fog_app();
        let gun = app
            .world_mut()
            .spawn(combatant(
                610,
                0,
                Vec3::new(0.0, 0.8, 0.0),
                UnitOrder::HoldPosition,
                UnitKind::Tank,
            ))
            .id();
        let red = app
            .world_mut()
            .spawn(combatant(
                611,
                1,
                Vec3::new(20.0, 0.8, 0.0),
                UnitOrder::HoldPosition,
                UnitKind::Tank,
            ))
            .id();
        let _spotter = crate::units::spawn_combat_unit(
            &mut app.world_mut().commands(),
            612,
            Team(0),
            UnitKind::Engineer,
            Vec3::new(12.0, 0.0, 0.0),
        );
        app.world_mut().flush();
        for _ in 0..80 {
            app.update();
        }
        assert!(app.world().get::<AttackTarget>(gun).is_some());
        // Lock holds (20m inside 18m gun + margin) but no shot can land yet:
        // red stays healthy until the gun closes in.
        let hp = app.world().get::<Health>(red).unwrap().current;
        assert!((hp - 120.0).abs() < 0.001, "spotted but out of range: {hp}");
    }

    #[test]
    fn explicit_attack_on_unseen_target_completes() {
        let mut app = combat_fog_app();
        let blue = app
            .world_mut()
            .spawn(combatant(
                620,
                0,
                Vec3::new(0.0, 0.8, 0.0),
                UnitOrder::HoldPosition,
                UnitKind::Tank,
            ))
            .id();
        let red = app
            .world_mut()
            .spawn(combatant(
                621,
                1,
                Vec3::new(100.0, 0.8, 0.0),
                UnitOrder::Idle,
                UnitKind::Tank,
            ))
            .id();
        crate::orders::queue_attack(&mut app.world_mut().commands().entity(blue), red);
        app.world_mut().flush();
        for _ in 0..40 {
            app.update();
        }
        assert!(app.world().get::<AttackTarget>(blue).is_none());
        assert!(matches!(
            app.world().get::<UnitOrder>(blue),
            Some(UnitOrder::Idle)
        ));
    }

    #[test]
    fn commander_fires_missiles_beyond_mitra_range() {
        use crate::units::{Commander, secondary_bundle};
        let mut app = combat_app();
        // Commander vs tank a 30m: fuori mitra (20), dentro missili (34) e
        // dentro acquisition (36). Solo i missili devono partire.
        let commander = app
            .world_mut()
            .spawn(combatant(
                500,
                0,
                Vec3::new(0.0, 1.6, 0.0),
                UnitOrder::HoldPosition,
                UnitKind::Commander,
            ))
            .id();
        let target = app
            .world_mut()
            .spawn(combatant(
                501,
                1,
                Vec3::new(30.0, 0.8, 0.0),
                UnitOrder::Idle,
                UnitKind::Tank,
            ))
            .id();
        app.world_mut()
            .entity_mut(commander)
            .insert(secondary_bundle(500, UnitKind::Commander).unwrap())
            .insert((
                Commander,
                crate::units::builder_bundle(UnitKind::Commander).unwrap(),
            ))
            .insert(AttackTarget(target));
        // Cooldown azzerati + turret allineate: isola il gating di range.
        // (Il bundle desynca il primo colpo fino a 2.5s, il test gira 0.5s.)
        app.world_mut()
            .entity_mut(commander)
            .insert(SecondaryWeaponState { remaining: 0.0 })
            .insert(WeaponState { remaining: 99.0 });
        // Aim yaw verso +X: yaw_toward((30,0,0)) ≈ -PI/2? Allinea entrambe.
        app.world_mut()
            .entity_mut(commander)
            .insert(TurretYaw(0.0))
            .insert(SecondaryTurretYaw(0.0));
        // Aim yaw verso +X: yaw_toward((30,0,0)) ≈ -PI/2? Allinea entrambe.
        let aim = crate::movement::yaw_toward(Vec3::X * 30.0);
        app.world_mut()
            .entity_mut(commander)
            .insert(TurretYaw(aim))
            .insert(SecondaryTurretYaw(aim));
        for _ in 0..30 {
            app.update();
        }
        let missiles: Vec<_> = app
            .world_mut()
            .query::<&Projectile>()
            .iter(app.world())
            .map(|p| (p.damage, p.speed))
            .collect();
        // Solo proiettili da 40 danni (missili), mai da 7 (mitra).
        assert!(!missiles.is_empty());
        assert!(missiles.iter().all(|(d, _)| (*d - 40.0).abs() < 0.001));
    }

    #[test]
    fn builder_assists_builder_by_guarding_and_closing_in() {
        use crate::orders::queue_guard;
        // Assist = Guard su builder alleato (right-click): il follower sta
        // nel raggio del sito quando il ward costruisce, e site_power somma
        // entrambi. Qui si prova la marcia di avvicinamento.
        let mut app = combat_app();
        let ward = crate::units::spawn_combat_unit(
            &mut app.world_mut().commands(),
            600,
            Team(0),
            UnitKind::Engineer,
            Vec3::ZERO,
        );
        let follower = crate::units::spawn_combat_unit(
            &mut app.world_mut().commands(),
            601,
            Team(0),
            UnitKind::Engineer,
            Vec3::X * 20.0,
        );
        app.world_mut().flush();
        // Disarmati ma costruibili: niente Weapon, con Builder.
        for e in [ward, follower] {
            assert!(app.world().get::<Weapon>(e).is_none());
            assert!(app.world().get::<crate::units::Builder>(e).is_some());
        }
        queue_guard(&mut app.world_mut().commands().entity(follower), ward);
        app.world_mut().flush();
        for _ in 0..600 {
            app.update();
        }
        let w = app.world().get::<Transform>(ward).unwrap().translation;
        let f = app.world().get::<Transform>(follower).unwrap().translation;
        assert!(
            w.distance(f) <= super::GUARD_RADIUS + 1.0,
            "follower must close in on the ward, dist={}",
            w.distance(f)
        );
        assert!(matches!(
            app.world().get::<UnitOrder>(follower),
            Some(UnitOrder::Guard { .. })
        ));
    }

    #[test]
    fn attack_move_squads_meet_fight_and_move_fires_on_the_march() {
        let mut app = combat_app();

        // Two armed enemies 25 units apart: inside acquisition (30),
        // outside weapon range (18). A third unit on plain Move nearby.
        let blue = app
            .world_mut()
            .spawn(combatant(
                100,
                0,
                Vec3::new(-30.0, 0.8, -40.0),
                UnitOrder::AttackMove {
                    destination: Vec3::new(30.0, 0.8, -40.0),
                },
                UnitKind::Tank,
            ))
            .id();
        let red = app
            .world_mut()
            .spawn(combatant(
                101,
                1,
                Vec3::new(-5.0, 0.8, -40.0),
                UnitOrder::AttackMove {
                    destination: Vec3::new(-60.0, 0.8, -40.0),
                },
                UnitKind::Tank,
            ))
            .id();
        let mover = app
            .world_mut()
            .spawn(combatant(
                102,
                0,
                Vec3::new(-30.0, 0.8, -30.0),
                UnitOrder::Move {
                    destination: Vec3::new(30.0, 0.8, -30.0),
                },
                UnitKind::Tank,
            ))
            .id();

        let mut saw_projectile = false;
        for tick in 0..900 {
            app.update();
            saw_projectile |= app
                .world_mut()
                .query::<&Projectile>()
                .iter(app.world())
                .next()
                .is_some();
            if tick == 60 {
                // Both fighters acquired each other quickly via the grid.
                let world = app.world();
                assert!(world.entity(blue).get::<AttackTarget>().is_some());
                assert!(world.entity(red).get::<AttackTarget>().is_some());
                // The marching unit acquires too, but keeps marching: it
                // fires on the move instead of stopping or chasing.
                assert!(world.entity(mover).get::<AttackTarget>().is_some());
                assert!(matches!(
                    world.entity(mover).get::<UnitOrder>(),
                    Some(UnitOrder::Move { .. })
                ));
            }
        }

        let world = app.world_mut();
        // Combat happened: projectiles flew and health dropped or a unit died.
        assert!(saw_projectile);
        let health_sum: f32 = world
            .query::<&Health>()
            .iter(world)
            .map(|health| health.current)
            .sum();
        assert!(health_sum < 300.0);
        // The mover kept marching (or arrived) and never held a stale state.
        let mover_order = world.entity(mover).get::<UnitOrder>().unwrap();
        assert!(matches!(
            mover_order,
            UnitOrder::Move { .. } | UnitOrder::Idle
        ));
        // Survivors keep a coherent order: attack-move destination preserved
        // or clean idle after arrival/completion, never a stale target.
        for entity in [blue, red] {
            if world.entities().contains(entity) {
                let order = world.entity(entity).get::<UnitOrder>().unwrap();
                assert!(matches!(
                    order,
                    UnitOrder::AttackMove { .. } | UnitOrder::Idle
                ));
            }
        }
    }

    #[test]
    fn dense_melee_keeps_fighting_until_resolved() {
        let mut app = combat_app();

        // 24v24 interleaved on a tight grid: everybody starts inside
        // acquisition range of several enemies.
        for index in 0..48 {
            let team = (index % 2) as u8;
            let column = (index / 2) % 8;
            let row = index / 16;
            let position = Vec3::new(-20.0 + column as f32 * 2.2, 0.8, -40.0 + row as f32 * 2.2);
            app.world_mut().spawn(combatant(
                200 + index,
                team,
                position,
                UnitOrder::AttackMove {
                    destination: Vec3::new(if team == 0 { 30.0 } else { -60.0 }, 0.8, -40.0),
                },
                UnitKind::Tank,
            ));
        }

        let snapshot = |app: &mut App| -> (usize, usize, usize) {
            let world = app.world_mut();
            let alive = world.query::<&Health>().iter(world).count();
            let engaging = world
                .query_filtered::<Entity, (With<Unit>, With<AttackTarget>)>()
                .iter(world)
                .count();
            let projectiles = world
                .query_filtered::<Entity, With<Projectile>>()
                .iter(world)
                .count();
            (alive, engaging, projectiles)
        };
        let mut kills_seen = 0;
        let mut max_projectiles = 0;
        for tick in 0..800 {
            app.update();
            let (_, _, projectiles) = snapshot(&mut app);
            max_projectiles = max_projectiles.max(projectiles);
            if tick % 40 == 0 {
                let (alive, _, _) = snapshot(&mut app);
                kills_seen = kills_seen.max(48 - alive);
            }
        }
        let (alive, engaging, _) = snapshot(&mut app);
        kills_seen = kills_seen.max(48 - alive);
        {
            let world = app.world_mut();
            let mut healths: Vec<f32> = world
                .query::<&Health>()
                .iter(world)
                .map(|health| health.current)
                .collect();
            healths.sort_by(f32::total_cmp);
            println!(
                "health min={:.1} max_projectiles={max_projectiles}",
                healths.first().unwrap_or(&-1.0)
            );
        }
        // Retargeting keeps locks fresh, so a packed melee resolves instead
        // of decaying into tail-chasing with almost no time in weapon range.
        assert!(kills_seen >= 15, "dense melee must resolve with kills");
        if alive >= 12 {
            assert!(
                engaging * 2 >= alive,
                "alive={alive} engaging={engaging}: packed units stopped engaging"
            );
        }
    }

    #[test]
    fn attack_move_stops_to_fight_while_move_fires_on_the_march() {
        let mut app = combat_app();

        // Unarmed stand-ins: valid targets that never fight back, so the
        // measured behaviour belongs to the blue orders under test.
        let attacker = app
            .world_mut()
            .spawn(combatant(
                120,
                0,
                Vec3::new(-30.0, 0.8, -40.0),
                UnitOrder::AttackMove {
                    destination: Vec3::new(60.0, 0.8, -40.0),
                },
                UnitKind::Tank,
            ))
            .id();
        let victim = app
            .world_mut()
            .spawn(combatant(
                121,
                1,
                Vec3::new(-10.0, 0.8, -40.0),
                UnitOrder::Idle,
                UnitKind::Tank,
            ))
            .id();
        let marcher = app
            .world_mut()
            .spawn(combatant(
                122,
                0,
                Vec3::new(-30.0, 0.8, 40.0),
                UnitOrder::Move {
                    destination: Vec3::new(60.0, 0.8, 40.0),
                },
                UnitKind::Tank,
            ))
            .id();
        let bystander = app
            .world_mut()
            .spawn(combatant(
                123,
                1,
                Vec3::new(30.0, 0.8, 30.0),
                UnitOrder::Idle,
                UnitKind::Tank,
            ))
            .id();
        for target in [victim, bystander] {
            app.world_mut()
                .entity_mut(target)
                .remove::<(Weapon, WeaponState)>();
        }

        // Displacement of the attacker while its victim lives: stop-to-fight
        // must hold it near its start while the marcher crosses the map.
        // The marcher's detour around the north wall needs ~1100 ticks.
        // Lanes sit 80 units apart so the pairs never interact.
        let mut held_disp = 0.0_f32;
        let mut march_yaw = 0.0_f32;
        for tick in 0..1100 {
            app.update();
            let world = app.world();
            if world.entities().contains(victim) {
                held_disp = held_disp.max(
                    (world
                        .entity(attacker)
                        .get::<Transform>()
                        .unwrap()
                        .translation
                        .x
                        + 30.0)
                        .abs(),
                );
            }
            // Mid-march the route runs due east: body must face travel.
            if tick == 500 {
                march_yaw = world
                    .entity(marcher)
                    .get::<Transform>()
                    .unwrap()
                    .rotation
                    .to_euler(EulerRot::YXZ)
                    .0;
            }
        }
        let world = app.world();
        assert!(!world.entities().contains(victim), "attacker must kill");
        assert!(
            held_disp < 12.0,
            "attack-move must stop to fight, drifted {held_disp}"
        );
        // Destination far from reached: the order (not just the target)
        // survives the engagement.
        assert!(matches!(
            world.entity(attacker).get::<UnitOrder>(),
            Some(UnitOrder::AttackMove { .. })
        ));
        // The marcher arrives while wounding the bystander in passing.
        let marched = world
            .entity(marcher)
            .get::<Transform>()
            .unwrap()
            .translation;
        assert!(
            marched.distance(Vec3::new(60.0, 0.8, 40.0)) < 1.0,
            "mover must arrive, at {marched:?}"
        );
        assert!(matches!(
            world.entity(marcher).get::<UnitOrder>(),
            Some(UnitOrder::Move { .. }) | Some(UnitOrder::Idle)
        ));
        // Marching body faces travel direction (due east mid-route).
        assert!(
            -march_yaw.sin() > 0.9,
            "body must face east mid-march, yaw={march_yaw}"
        );
        let bystander_hp = world.entity(bystander).get::<Health>().unwrap().current;
        assert!(
            bystander_hp < 100.0,
            "mover must fire on the march, bystander at {bystander_hp}"
        );
        assert!(world.entities().contains(bystander));
    }

    #[test]
    fn queued_orders_pop_on_completion() {
        let mut app = combat_app();
        let unit = app
            .world_mut()
            .spawn(combatant(
                130,
                0,
                Vec3::new(-30.0, 0.8, 60.0),
                UnitOrder::Move {
                    destination: Vec3::new(30.0, 0.8, 60.0),
                },
                UnitKind::Tank,
            ))
            .id();
        app.world_mut()
            .entity_mut(unit)
            .insert(UnitOrderQueue(vec![UnitOrder::HoldPosition]));
        for _ in 0..900 {
            app.update();
        }
        let world = app.world();
        // Arrived AND popped: Hold is live (never plain Idle), the queue
        // drained, the body sits at the march destination.
        assert_eq!(
            world.entity(unit).get::<UnitOrder>(),
            Some(&UnitOrder::HoldPosition)
        );
        assert!(
            world
                .entity(unit)
                .get::<UnitOrderQueue>()
                .map(|queue| queue.0.is_empty())
                .unwrap_or(true)
        );
        let position = world.entity(unit).get::<Transform>().unwrap().translation;
        assert!(position.distance(Vec3::new(30.0, 0.8, 60.0)) < 1.0);
    }

    #[test]
    fn patrol_advances_waypoints_and_loops_forever() {
        let mut app = combat_app();
        let a = Vec3::new(-30.0, 0.8, 60.0);
        let b = Vec3::new(30.0, 0.8, 60.0);
        let unit = app
            .world_mut()
            .spawn(combatant(
                140,
                0,
                a,
                UnitOrder::Patrol {
                    points: vec![a, b],
                    next: 0,
                },
                UnitKind::Tank,
            ))
            .id();
        for _ in 0..200 {
            app.update();
        }
        // Spawned on waypoint A: advanced to leg B without completing.
        assert!(matches!(
            app.world().entity(unit).get::<UnitOrder>(),
            Some(UnitOrder::Patrol { next: 1, .. })
        ));
        for _ in 0..700 {
            app.update();
        }
        // Reached B and looped back to leg A: patrols never complete.
        let world = app.world();
        assert!(matches!(
            world.entity(unit).get::<UnitOrder>(),
            Some(UnitOrder::Patrol { next: 0, .. })
        ));
        assert!(world.entities().contains(unit));
    }

    #[test]
    fn guard_follows_ward_and_completes_when_ward_dies() {
        let mut app = combat_app();
        let ward = app
            .world_mut()
            .spawn(combatant(
                150,
                0,
                Vec3::new(0.0, 0.8, 60.0),
                UnitOrder::Idle,
                UnitKind::Tank,
            ))
            .id();
        let guard = app
            .world_mut()
            .spawn(combatant(
                151,
                0,
                Vec3::new(-30.0, 0.8, 60.0),
                UnitOrder::Guard { target: ward },
                UnitKind::Tank,
            ))
            .id();
        for _ in 0..900 {
            app.update();
        }
        // Closed the 30-unit gap and holds near the ward, order intact.
        let world = app.world();
        let guard_pos = world.entity(guard).get::<Transform>().unwrap().translation;
        let ward_pos = world.entity(ward).get::<Transform>().unwrap().translation;
        assert!(
            guard_pos.distance(ward_pos) <= GUARD_RADIUS + 2.0,
            "guard must close in, at {guard_pos:?} ward at {ward_pos:?}"
        );
        assert!(matches!(
            world.entity(guard).get::<UnitOrder>(),
            Some(UnitOrder::Guard { .. })
        ));
        // Ward destroyed: guard completes to Idle instead of following a ghost.
        app.world_mut()
            .entity_mut(ward)
            .get_mut::<Health>()
            .unwrap()
            .current = 0.0;
        for _ in 0..10 {
            app.update();
        }
        let world = app.world();
        assert_eq!(
            world.entity(guard).get::<UnitOrder>(),
            Some(&UnitOrder::Idle)
        );
    }

    #[test]
    fn guard_leash_is_ward_centered_with_acquisition_and_retarget_hysteresis() {
        // Isolate targeting from damage/motion so every boundary is exact.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<CombatClock>()
            .insert_resource(SpatialGrid::new(10.0))
            .add_systems(
                Update,
                (
                    validate_guards,
                    validate_targets,
                    acquire_targets,
                    resolve_behaviour,
                )
                    .chain(),
            );
        let ward = app
            .world_mut()
            .spawn(combatant(
                170,
                0,
                Vec3::ZERO,
                UnitOrder::HoldPosition,
                UnitKind::Tank,
            ))
            .id();
        let guard = app
            .world_mut()
            .spawn(combatant(
                171,
                0,
                Vec3::X * 20.0,
                UnitOrder::Guard { target: ward },
                UnitKind::Tank,
            ))
            .id();
        let threat = app
            .world_mut()
            .spawn(combatant(
                172,
                1,
                Vec3::X * 30.0,
                UnitOrder::HoldPosition,
                UnitKind::Tank,
            ))
            .id();
        let decoy = app
            .world_mut()
            .spawn(combatant(
                173,
                1,
                Vec3::X * 100.0,
                UnitOrder::HoldPosition,
                UnitKind::Tank,
            ))
            .id();
        app.world_mut()
            .entity_mut(guard)
            .insert(AcquisitionRange(30.0));
        for entity in [ward, threat, decoy] {
            app.world_mut().entity_mut(entity).remove::<Weapon>();
        }
        let step = |app: &mut App| {
            let entries: Vec<_> = app
                .world_mut()
                .query::<(Entity, &Transform)>()
                .iter(app.world())
                .map(|(entity, transform)| (entity, transform.translation))
                .collect();
            let mut grid = app.world_mut().resource_mut::<SpatialGrid>();
            grid.clear();
            for (entity, position) in entries {
                grid.insert(entity, position, 0.5);
            }
            for _ in 0..ACQUIRE_STRIDE + 1 {
                app.update();
            }
        };
        let place = |app: &mut App, entity: Entity, x: f32| {
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<Transform>()
                .unwrap()
                .translation = Vec3::X * x;
        };
        let locked = |app: &App| {
            app.world()
                .entity(guard)
                .get::<AttackTarget>()
                .map(|lock| lock.0)
        };
        step(&mut app);
        assert_eq!(locked(&app), Some(threat)); // acquisition boundary inclusive

        place(&mut app, threat, 45.0);
        place(&mut app, guard, 35.0);
        place(&mut app, decoy, 34.0); // closer/shootable but outside ward acquisition
        step(&mut app);
        assert_eq!(locked(&app), Some(threat)); // retain at release boundary; no retarget

        place(&mut app, threat, 45.1);
        step(&mut app);
        assert_eq!(locked(&app), None); // still only 10.1 from guard
        assert!(!app.world().entity(guard).contains::<Chasing>());
        assert!(!app.world().entity(guard).contains::<HoldFire>());
        assert_eq!(
            app.world().entity(guard).get::<MoveTarget>().unwrap().0,
            Vec3::ZERO
        );
        assert!(matches!(
            app.world().entity(guard).get::<UnitOrder>(),
            Some(UnitOrder::Guard { .. })
        ));

        for x in [44.9, 45.1, 30.1] {
            place(&mut app, threat, x);
            step(&mut app);
            assert_eq!(locked(&app), None); // no reacquisition flicker in the band
        }
        place(&mut app, threat, 30.0);
        step(&mut app);
        assert_eq!(locked(&app), Some(threat));

        // Moving the ward, with guard/enemy unchanged, also breaks the leash.
        place(&mut app, ward, -16.0);
        step(&mut app);
        assert_eq!(locked(&app), None);
        assert_eq!(
            app.world().entity(guard).get::<MoveTarget>().unwrap().0,
            Vec3::X * -16.0
        );

        // A closer excluded enemy must not hide an eligible second candidate.
        place(&mut app, ward, 0.0);
        place(&mut app, decoy, 34.0);
        step(&mut app);
        assert_eq!(locked(&app), Some(threat));
        // An eligible closer threat can still win a retarget.
        place(&mut app, threat, 25.0);
        place(&mut app, decoy, 29.5);
        step(&mut app);
        assert_eq!(locked(&app), Some(decoy));
        // Ordinary guard-to-enemy leash remains enforced too.
        place(&mut app, guard, -20.0);
        step(&mut app);
        assert_eq!(locked(&app), None);
    }

    #[test]
    fn attack_chase_syncs_move_target_to_moving_quarry() {
        let mut app = combat_app();
        let quarry = app
            .world_mut()
            .spawn(combatant(
                160,
                1,
                Vec3::new(0.0, 0.8, -20.0),
                UnitOrder::Idle,
                UnitKind::Tank,
            ))
            .id();
        let hunter = app
            .world_mut()
            .spawn(combatant(
                161,
                0,
                Vec3::new(-25.0, 0.8, -20.0),
                UnitOrder::Attack { target: quarry },
                UnitKind::Tank,
            ))
            .id();
        for _ in 0..30 {
            app.update();
        }
        // Hunter acquired the quarry and synced a MoveTarget near it.
        let world = app.world();
        assert!(world.entity(hunter).get::<AttackTarget>().is_some());
        let synced = world.entity(hunter).get::<MoveTarget>().unwrap().0;
        let quarry_pos = world.entity(quarry).get::<Transform>().unwrap().translation;
        assert!(
            synced.distance(quarry_pos) <= CHASE_REPLAN_DISTANCE + 0.01,
            "chase must track quarry, synced at {synced:?} quarry at {quarry_pos:?}"
        );
    }

    #[test]
    fn holders_fire_without_chasing_and_ignore_out_of_reach_targets() {
        let mut app = combat_app();

        let post = Vec3::new(-20.0, 0.8, -40.0);
        let holder = app
            .world_mut()
            .spawn(combatant(
                110,
                0,
                post,
                UnitOrder::HoldPosition,
                UnitKind::Tank,
            ))
            .id();
        // Unarmed: marches into holder range and stops 5 units past it.
        // Must die to holder fire without the holder ever chasing.
        let passer = app
            .world_mut()
            .spawn(combatant(
                111,
                1,
                Vec3::new(-5.0, 0.8, -40.0),
                UnitOrder::AttackMove {
                    destination: Vec3::new(-25.0, 0.8, -40.0),
                },
                UnitKind::Tank,
            ))
            .id();
        app.world_mut()
            .entity_mut(passer)
            .remove::<(Weapon, WeaponState)>();
        // Idles at 25 (inside acquisition, outside holder reach): untouchable.
        let bystander_spot = Vec3::new(-20.0, 0.8, -15.0);
        let bystander = app
            .world_mut()
            .spawn(combatant(
                112,
                1,
                bystander_spot,
                UnitOrder::Idle,
                UnitKind::Tank,
            ))
            .id();

        for _ in 0..900 {
            app.update();
        }
        let world = app.world();
        // The passer walked into range and died; the holder never chased it.
        assert!(world.entities().contains(holder));
        assert!(!world.entities().contains(passer));
        let held = world.entity(holder).get::<Transform>().unwrap().translation;
        assert!(held.distance(post) < 1.5, "holder drifted to {held:?}");
        assert_eq!(
            world.entity(holder).get::<UnitOrder>(),
            Some(&UnitOrder::HoldPosition)
        );
        // Out-of-reach bystander: full health, never moved, never engaged by
        // a holder that cannot close distance.
        let idle = world
            .entity(bystander)
            .get::<Transform>()
            .unwrap()
            .translation;
        assert!(idle.distance(bystander_spot) < 0.5);
        assert_eq!(
            world.entity(bystander).get::<Health>().unwrap().current,
            crate::units::archetype(UnitKind::Tank).max_health
        );
    }

    #[test]
    fn retarget_prefers_clearly_closer_or_shootable_targets() {
        let range_sq = 18.0_f32.powi(2);
        // Much closer wins, marginal gains do not (no thrash).
        assert!(should_retarget(100.0, 20.0, range_sq));
        assert!(!should_retarget(100.0, 90.0, range_sq));
        assert!(!should_retarget(100.0, 100.0, range_sq));
        assert!(!should_retarget(100.0, 400.0, range_sq));
        // A shootable candidate beats a lock outside weapon range.
        assert!(should_retarget(500.0, 300.0, range_sq));
        // Between two shootable locks, hysteresis still applies.
        assert!(!should_retarget(300.0, 200.0, range_sq));
        assert!(should_retarget(300.0, 100.0, range_sq));
    }

    #[test]
    fn attack_target_is_temporary_and_distinct_from_orders() {
        // The order keeps the strategic destination while the target is
        // a separate component that can be dropped without losing it.
        let destination = Vec3::new(10.0, 0.0, 0.0);
        let order = UnitOrder::AttackMove { destination };
        let target = AttackTarget(Entity::from_bits(7));
        assert!(allows_auto_targeting(&order));
        assert_ne!(target.0, Entity::from_bits(8));
        if let UnitOrder::AttackMove { destination } = order {
            assert_eq!(destination, Vec3::new(10.0, 0.0, 0.0));
        } else {
            panic!("order must keep its destination during engagements");
        }
    }
}
