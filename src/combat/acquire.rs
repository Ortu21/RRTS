//! Combattimento: HP, armi, targeting guidato da ordini, proiettili, morti.
//!
//! Regole BAR-style: `AttackMove` si ferma e riprende, `Move` spara marciando,
//! `Hold`/`Idle` difendono in piedi. Targeting sempre gated su fog (`can_target`).
//! I marker `Chasing`/`HoldFire` sono locomozione, l'intento resta in `orders::UnitOrder`.

//! Acquisizione target: scan, lock, behaviour, chase.

use super::death::{Health, is_dead};
use super::fire::{SecondaryWeapon, Weapon};
use super::guard::GUARD_RADIUS;
use super::projectile::in_weapon_range;
use crate::{
    fog::{VisibilityMap, can_target},
    movement::{ARRIVE_RADIUS, MoveTarget, Movement, face_toward},
    navigation::Route,
    orders::{UnitOrder, UnitOrderQueue, allows_auto_targeting, allows_chase, complete_order},
    spatial::SpatialGrid,
    units::{Builder, CollisionRadius, Team, Unit, UnitKind},
};
use bevy::prelude::*;

/// Extra leash margin before an automatic chaser drops a distant target.
/// Hysteresis avoids acquire/drop flicker at the acquisition boundary.
const LEASH_MULTIPLIER: f32 = 1.5;
/// Holders never close distance, so a lock outside weapon reach is useless:
/// dropping it frees the slot for closer acquisitions instead of holding a
/// target the unit can never shoot.
const HOLD_MARGIN: f32 = 1.25;
/// Hysteresis for target re-evaluation (squared distances): switch locks
/// only when the candidate is clearly closer, to avoid thrash between
/// equidistant enemies.
const RETARGET_HYSTERESIS_SQ: f32 = 0.36;
/// Chase replan threshold: when an explicit Attack target drifts this far
/// from the synced MoveTarget, refresh it and drop the stale Route so the
/// budgeted planner routes to the target's live position (obstacle-aware
/// pursuit instead of a straight-line chase through walls).
pub const CHASE_REPLAN_DISTANCE: f32 = 4.0;
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

/// Acquisition range is deliberately separate from weapon range.
#[derive(Component, Debug, Clone, Copy)]
pub struct AcquisitionRange(pub f32);

/// Temporary combat target. Never replaces the unit's `UnitOrder`:
/// when it is removed, the behaviour resumes the standing order.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttackTarget(pub Entity);

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
pub(crate) fn target_position(grid: &SpatialGrid, target: Entity) -> Option<Vec3> {
    grid.position(target)
}

/// Guard ward validation, separate from enemy-lock validation: the ward
/// Targeting is a service, not a behaviour: it only answers requests from
/// orders that allow automatic acquisition, and only returns live enemies.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn validate_targets(
    mut commands: Commands,
    grid: Res<SpatialGrid>,
    health: Query<&Health>,
    teams: Query<&Team>,
    ranges: Query<&AcquisitionRange>,
    weapons: Query<&Weapon>,
    secondary: Query<&SecondaryWeapon>,
    map: Option<Res<VisibilityMap>>,
    units: Query<
        (Entity, &Transform, &UnitOrder, &AttackTarget),
        Or<(With<Unit>, With<crate::structures::Building>)>,
    >,
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
pub(crate) fn acquire_targets(
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
        (
            Or<(With<Unit>, With<crate::structures::Building>)>,
            With<Weapon>,
        ),
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
pub(crate) fn nearest_enemy(
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
pub(crate) fn resolve_behaviour(
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
        Or<(With<Unit>, With<crate::structures::Building>)>,
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
                // Body-aware repair: the chase anchor must fit this hull,
                // otherwise the planner would reject it and strand the chase.
                let body_radius = body.map_or(0.5, |b| b.0);
                let anchor = if gameplay.is_some() && !in_range {
                    target_position(&grid, target.0).map(|p| {
                        nav.as_ref()
                            .map_or(p, |nav| nav.clear_point_for(p.with_y(0.0), body_radius))
                    })
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
pub(crate) fn chase_targets(
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
        let body_radius = crate::units::archetype(*kind).radius;
        if gameplay.is_some()
            && nav.as_ref().is_some_and(|nav| {
                !nav.segment_clear_for(transform.translation, target_position, body_radius)
            })
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
            crate::movement::steer_for(&mut transform, offset, step.min(distance), body_radius);
        }
    }
}
