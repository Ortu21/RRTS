use bevy::prelude::*;

use crate::{
    movement::{MoveTarget, Movement, MovementSystems},
    navigation::Route,
    orders::{UnitOrder, allows_auto_targeting, allows_chase, queue_stop},
    spatial::{MAP_BOUND, SpatialGrid},
    units::{Team, Unit},
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
                    validate_targets.in_set(ValidateTargets),
                    (acquire_targets, retarget_stale_locks)
                        .chain()
                        .in_set(AcquireTargets),
                    resolve_behaviour.in_set(ResolveBehaviour),
                    chase_targets.in_set(ChaseTargets),
                    (tick_cooldowns, fire_weapons).chain().in_set(WeaponSystems),
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

/// Extra leash margin before an attack-move drops a target it chased too far.
/// Hysteresis avoids acquire/drop flicker at the acquisition boundary.
const LEASH_MULTIPLIER: f32 = 1.5;
const PROJECTILE_HIT_RADIUS: f32 = 0.7;
/// Holders never close distance, so a lock outside weapon reach is useless:
/// dropping it frees the slot for closer acquisitions instead of holding a
/// target the unit can never shoot.
const HOLD_MARGIN: f32 = 1.25;
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

pub fn full_health() -> Health {
    Health {
        current: 100.0,
        max: 100.0,
    }
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

pub fn default_weapon() -> Weapon {
    Weapon {
        range: 18.0,
        cooldown: 1.0,
        damage: 10.0,
        projectile_speed: 30.0,
    }
}

#[derive(Component, Debug, Clone, Copy, Default)]
pub struct WeaponState {
    pub remaining: f32,
}

/// Deterministic cooldown phase in [0, 1): spreads first volleys over time
/// so damage arrives smoothly instead of synchronized waves. Pure function
/// of the entity, so repeated runs stay deterministic.
pub fn desync_phase(entity: Entity) -> f32 {
    (entity.to_bits().wrapping_mul(0x9E3779B97F4A7C15) % 1000) as f32 / 1000.0
}

/// Acquisition range is deliberately separate from weapon range.
#[derive(Component, Debug, Clone, Copy)]
pub struct AcquisitionRange(pub f32);

/// Temporary combat target. Never replaces the unit's `UnitOrder`:
/// when it is removed, the behaviour resumes the standing order.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttackTarget(pub Entity);

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

/// Targeting is a service, not a behaviour: it only answers requests from
/// orders that allow automatic acquisition, and only returns live enemies.
fn validate_targets(
    mut commands: Commands,
    grid: Res<SpatialGrid>,
    health: Query<&Health>,
    teams: Query<&Team>,
    ranges: Query<&AcquisitionRange>,
    weapons: Query<&Weapon>,
    units: Query<(Entity, &Transform, &UnitOrder, &AttackTarget), With<Unit>>,
) {
    for (entity, transform, order, target) in &units {
        let valid = match target_position(&grid, target.0)
            .and_then(|_| health.get(target.0).ok())
            .filter(|health| !is_dead(health))
            .and(teams.get(target.0).ok())
        {
            Some(target_team) => {
                let own_team = teams.get(entity).ok();
                match (own_team, order) {
                    (Some(own), UnitOrder::AttackMove { .. }) if own.is_enemy(*target_team) => {
                        // Leash: drop targets chased far outside acquisition.
                        match (ranges.get(entity).ok(), target_position(&grid, target.0)) {
                            (Some(range), Some(target_position)) => {
                                transform.translation.distance(target_position)
                                    <= range.0 * LEASH_MULTIPLIER
                            }
                            _ => true,
                        }
                    }
                    (Some(own), UnitOrder::Attack { target: ordered })
                        if own.is_enemy(*target_team) && target.0 == *ordered =>
                    {
                        true
                    }
                    (Some(own), UnitOrder::HoldPosition) if own.is_enemy(*target_team) => {
                        // Holders cannot chase: keep the lock only while the
                        // target stays within weapon reach (plus margin).
                        match (weapons.get(entity).ok(), target_position(&grid, target.0)) {
                            (Some(weapon), Some(target_position)) => {
                                transform.translation.distance(target_position)
                                    <= weapon.range * HOLD_MARGIN
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
                queue_stop(&mut commands.entity(entity));
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn acquire_targets(
    mut commands: Commands,
    grid: Res<SpatialGrid>,
    mut clock: ResMut<CombatClock>,
    units: Query<
        (Entity, &Transform, &Team, &UnitOrder, &AcquisitionRange),
        (With<Unit>, With<Weapon>, Without<AttackTarget>),
    >,
    candidates: Query<(&Team, &Health), With<Unit>>,
) {
    clock.tick += 1;
    for (entity, transform, team, order, range) in &units {
        if let UnitOrder::Attack { target } = order {
            // Explicit order, not automatic acquisition.
            let valid = candidates
                .get(*target)
                .is_ok_and(|(target_team, health)| team.is_enemy(*target_team) && !is_dead(health));
            if valid {
                commands.entity(entity).insert(AttackTarget(*target));
            } else {
                queue_stop(&mut commands.entity(entity));
            }
            continue;
        }
        if !allows_auto_targeting(order) {
            continue;
        }
        if !(clock.tick + entity.to_bits()).is_multiple_of(ACQUIRE_STRIDE) {
            continue;
        }
        if let Some((target, _)) = nearest_enemy(
            &grid,
            &candidates,
            team,
            transform.translation,
            range.0,
            entity,
            None,
        ) {
            commands.entity(entity).insert(AttackTarget(target));
        }
    }
}

/// Nearest live enemy within `range` of `position`, excluding `ignore` and
/// optionally `skip`. Ties break deterministically so repeated runs agree.
fn nearest_enemy(
    grid: &SpatialGrid,
    candidates: &Query<(&Team, &Health), With<Unit>>,
    team: &Team,
    position: Vec3,
    range: f32,
    ignore: Entity,
    skip: Option<Entity>,
) -> Option<(Entity, f32)> {
    let mut best: Option<(Entity, f32)> = None;
    grid.for_each_nearby(position, range, |entry| {
        let candidate = entry.entity;
        if candidate == ignore || Some(candidate) == skip {
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

/// Periodic lock re-evaluation for attack-moving units: on a unit's stagger
/// tick, replace a stale lock with a clearly closer enemy (see
/// `should_retarget`). Explicit `Attack` orders keep their ordered target;
/// invalid locks remain owned by validation.
#[allow(clippy::type_complexity)]
fn retarget_stale_locks(
    mut commands: Commands,
    grid: Res<SpatialGrid>,
    clock: Res<CombatClock>,
    weapons: Query<&Weapon>,
    units: Query<
        (
            Entity,
            &Transform,
            &Team,
            &UnitOrder,
            &AttackTarget,
            &AcquisitionRange,
        ),
        (With<Unit>, With<Weapon>),
    >,
    candidates: Query<(&Team, &Health), With<Unit>>,
) {
    for (entity, transform, team, order, target, range) in &units {
        if !matches!(order, UnitOrder::AttackMove { .. }) {
            continue;
        }
        if !(clock.tick + entity.to_bits()).is_multiple_of(ACQUIRE_STRIDE) {
            continue;
        }
        let Some(current_position) = target_position(&grid, target.0) else {
            continue; // owned by validation
        };
        let current_sq = transform.translation.distance_squared(current_position);
        let weapon_range_sq = weapons
            .get(entity)
            .map(|weapon| weapon.range.powi(2))
            .unwrap_or(f32::MAX);
        // Only strictly nearer candidates can win (see `should_retarget`),
        // so shrink the scan to the current lock distance: wide scans happen
        // only for stale far locks, settled melee costs almost nothing.
        let radius = range.0.min(current_sq.sqrt() + 0.01);
        if let Some((candidate, _)) = nearest_enemy(
            &grid,
            &candidates,
            team,
            transform.translation,
            radius,
            entity,
            Some(target.0),
        )
        .filter(|(_, candidate_sq)| should_retarget(current_sq, *candidate_sq, weapon_range_sq))
        {
            commands.entity(entity).insert(AttackTarget(candidate));
        }
    }
}

/// Resolves order intent into movement/fire state. Attack-move keeps its
/// strategic destination in the order while engaging temporary targets.
/// Engaged units deliberately keep their `MoveTarget`/`Route`: route
/// following pauses for them (see `move_units`), so when the engagement
/// ends the previous route resumes instantly without replanning. Dropping
/// routes on every engagement would churn the budgeted path planner and
/// strand units without routes for hundreds of ticks at scale.
#[allow(clippy::type_complexity)]
fn resolve_behaviour(
    mut commands: Commands,
    units: Query<
        (
            Entity,
            &UnitOrder,
            Option<&AttackTarget>,
            Option<&MoveTarget>,
            Has<Route>,
        ),
        With<Unit>,
    >,
) {
    for (entity, order, target, move_target, has_route) in &units {
        match (order, target) {
            (UnitOrder::Idle, _) => {}
            (UnitOrder::Move { destination }, _) => {
                if move_target.is_none_or(|current| current.0 != *destination) {
                    commands
                        .entity(entity)
                        .insert(MoveTarget(*destination))
                        .remove::<Route>();
                }
            }
            (UnitOrder::AttackMove { destination }, None) => {
                if move_target.is_none() && !has_route {
                    // Destination reached: the order completes.
                    commands.entity(entity).insert(UnitOrder::Idle);
                } else if move_target.is_none_or(|current| current.0 != *destination) {
                    commands
                        .entity(entity)
                        .insert(MoveTarget(*destination))
                        .remove::<Route>();
                }
            }
            // Engaging: chase steering and holding are handled by the combat
            // systems; the strategic route waits untouched underneath.
            // Holders simply keep firing from their position: no route, and
            // chase is gated off for their order (see `allows_chase`).
            (UnitOrder::AttackMove { .. } | UnitOrder::Attack { .. }, Some(_)) => {}
            (UnitOrder::HoldPosition, _) => {
                if move_target.is_some() || has_route {
                    commands.entity(entity).remove::<(MoveTarget, Route)>();
                }
            }
            (UnitOrder::Attack { .. }, None) => {
                commands.entity(entity).insert(UnitOrder::Idle);
            }
        }
    }
}

/// Direct kinematic chase toward the temporary target while outside weapon
/// range. No pathfinding here: strategic routes use the nav grid, combat
/// steering is local. Runs after route movement.
fn chase_targets(
    time: Res<Time>,
    grid: Res<SpatialGrid>,
    mut units: Query<
        (
            &mut Transform,
            &Movement,
            &Weapon,
            &AttackTarget,
            &UnitOrder,
        ),
        With<Unit>,
    >,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (mut transform, movement, weapon, target, order) in &mut units {
        if !allows_chase(order) {
            continue;
        }
        let Some(target_position) = target_position(&grid, target.0) else {
            continue;
        };
        let offset = target_position - transform.translation;
        if in_weapon_range(transform.translation, target_position, weapon.range) {
            continue;
        }
        let step = movement.speed * dt;
        let distance = offset.length();
        if distance > f32::EPSILON {
            transform.translation += offset / distance * step.min(distance);
            transform.translation.x = transform.translation.x.clamp(-MAP_BOUND, MAP_BOUND);
            transform.translation.z = transform.translation.z.clamp(-MAP_BOUND, MAP_BOUND);
        }
    }
}

fn tick_cooldowns(time: Res<Time>, mut states: Query<&mut WeaponState>) {
    let dt = time.delta_secs();
    for mut state in &mut states {
        state.remaining = (state.remaining - dt).max(0.0);
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

fn fire_weapons(
    mut commands: Commands,
    grid: Res<SpatialGrid>,
    assets: Option<Res<ProjectileAssets>>,
    health: Query<&Health>,
    mut shooters: Query<(&Transform, &Team, &Weapon, &mut WeaponState, &AttackTarget), With<Unit>>,
) {
    let Some(assets) = assets else {
        return;
    };
    for (transform, team, weapon, mut state, target) in &mut shooters {
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
        state.remaining = weapon.cooldown;
        commands.spawn((
            Projectile {
                target: target.0,
                speed: weapon.projectile_speed,
                damage: weapon.damage,
            },
            *team,
            Transform::from_translation(transform.translation),
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
    mut health: Query<&mut Health, With<Unit>>,
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

fn process_deaths(mut commands: Commands, units: Query<(Entity, &Health), With<Unit>>) {
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
        navigation::NavigationPlugin,
        scenario::Scenario,
        spatial::SpatialPlugin,
        units::{CollisionRadius, UnitPlugin},
    };
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    fn combatant(
        id: u32,
        team: u8,
        position: Vec3,
        order: UnitOrder,
    ) -> (
        Unit,
        Team,
        crate::movement::Movement,
        CollisionRadius,
        UnitOrder,
        MoveTarget,
        Health,
        Weapon,
        WeaponState,
        AcquisitionRange,
        Transform,
    ) {
        let destination = match order {
            UnitOrder::Move { destination } | UnitOrder::AttackMove { destination } => destination,
            _ => position,
        };
        (
            Unit(id),
            Team(team),
            crate::movement::Movement { speed: 7.0 },
            CollisionRadius(crate::spatial::DEFAULT_UNIT_RADIUS),
            order,
            MoveTarget(destination),
            full_health(),
            default_weapon(),
            WeaponState::default(),
            AcquisitionRange(30.0),
            Transform::from_translation(position),
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
        assert!(!is_dead(&full_health()));
    }

    #[test]
    fn weapon_range_is_inclusive_and_uses_full_distance() {
        let from = Vec3::ZERO;
        assert!(in_weapon_range(from, Vec3::new(18.0, 0.0, 0.0), 18.0));
        assert!(!in_weapon_range(from, Vec3::new(18.1, 0.0, 0.0), 18.0));
    }

    #[test]
    fn attack_move_squads_meet_fight_and_move_ignores_enemies() {
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
            ));
        app.finish();
        app.cleanup();
        app.update();

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
                // The plain move never acquires, even with enemies close.
                assert!(world.entity(mover).get::<AttackTarget>().is_none());
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
        // The mover kept its order and never engaged.
        let mover_order = world.entity(mover).get::<UnitOrder>().unwrap();
        assert!(matches!(mover_order, UnitOrder::Move { .. }));
        assert!(world.entity(mover).get::<AttackTarget>().is_none());
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
            ));
        app.finish();
        app.cleanup();
        app.update();

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
    fn holders_fire_without_chasing_and_ignore_out_of_reach_targets() {
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
            ));
        app.finish();
        app.cleanup();
        app.update();

        let post = Vec3::new(-20.0, 0.8, -40.0);
        let holder = app
            .world_mut()
            .spawn(combatant(110, 0, post, UnitOrder::HoldPosition))
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
            ))
            .id();
        app.world_mut()
            .entity_mut(passer)
            .remove::<(Weapon, WeaponState)>();
        // Idles at 25 (inside acquisition, outside holder reach): untouchable.
        let bystander_spot = Vec3::new(-20.0, 0.8, -15.0);
        let bystander = app
            .world_mut()
            .spawn(combatant(112, 1, bystander_spot, UnitOrder::Idle))
            .id();

        for _ in 0..800 {
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
            100.0
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
