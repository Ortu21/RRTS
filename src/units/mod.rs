use crate::{
    camera::RtsCamera,
    combat::{
        AcquisitionRange, Health, SecondaryTurretYaw, SecondaryWeapon, SecondaryWeaponState,
        Weapon, WeaponState, desync_phase,
    },
    formation::{formation_slots, skirmish_slots},
    movement::{Movement, wrap_angle},
    navigation::{HALF_SIZE, NavGrid},
    orders::UnitOrder,
    scenario::Scenario,
};
use bevy::prelude::*;

pub mod archetype;
pub use archetype::{UnitKind, archetype, kind_for_index};

pub const UNIT_HALF_SIZE: Vec3 = Vec3::new(0.55, 0.8, 0.55);
/// Health bar dimensions: full-width quad floating above the unit.
pub const BAR_WIDTH: f32 = 1.3;
pub const BAR_HEIGHT: f32 = 0.1;
pub const BAR_Y: f32 = 1.25;
/// Turret pivot height above the unit origin and muzzle reach. The barrel
/// offset is baked into the shared mesh so aiming needs a single child.
pub const TURRET_Y: f32 = 0.95;
pub const MUZZLE_REACH: f32 = 0.9;
pub struct UnitPlugin {
    pub visuals: bool,
}
impl Plugin for UnitPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UnitIds>()
            .add_systems(Startup, spawn_units);
        if self.visuals {
            app.add_systems(Startup, setup_visual_assets)
                .add_systems(PostUpdate, add_visuals);
        }
        app.add_systems(PostUpdate, (update_health_bars, aim_turrets, aim_secondary_turrets));
    }
}
#[derive(Component)]
pub struct Unit(pub u32);
#[derive(Component)]
pub struct Selectable;
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub struct Team(pub u8);
pub const PLAYER_TEAM: Team = Team(0);

impl Team {
    pub fn is_enemy(self, other: Team) -> bool {
        self != other
    }
}

/// Soft-body radius used by the spatial-grid avoidance system.
/// Units never get physics rigid bodies.
#[derive(Component)]
pub struct CollisionRadius(pub f32);
#[derive(Component)]
pub struct SelectionRing;
#[derive(Component)]
pub struct HealthBarBg;
#[derive(Component)]
pub struct HealthBarFg;
#[derive(Component)]
pub struct Turret;
/// Second barrel marker for the Commander missiles. Visual-only like
/// `Turret`: mirrors `SecondaryTurretYaw`, never read back by simulation.
#[derive(Component)]
pub struct SecondaryTurret;
/// Beacon marking unarmed builders (Engineer): orange tool box on the hull.
/// Visual-only, never read back by simulation.
#[derive(Component)]
pub struct BuilderBeacon;
/// Marker for the initial builder/base unit. Never queued in factories.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Commander;
/// Construction capability: build radius + work power. Lives on the
/// Commander now; future mobile builders reuse the same component.
#[derive(Component, Debug, Clone, Copy)]
pub struct Builder {
    pub radius: f32,
    pub power: f64,
}

/// Left-anchored fill transform for a unit-health fraction in [0, 1].
/// Returns (scale_x, local_x_offset): the shared quad keeps full width for
/// the background, the foreground shrinks toward the left edge.
pub fn bar_fill(fraction: f32) -> (f32, f32) {
    let clamped = fraction.clamp(0.0, 1.0);
    (clamped, -(1.0 - clamped) * BAR_WIDTH * 0.5)
}

pub(crate) fn spawn_units(
    mut commands: Commands,
    scenario: Res<Scenario>,
    grid: Res<NavGrid>,
    mut ids: ResMut<UnitIds>,
) {
    // Playground (cargo run): solo comandante per team. E' la base operativa
    // iniziale: costruisce, spara con mitra + missili, in futuro potenziabile.
    // Niente esercito precostituito, niente base precostruita: il loop
    // costruzione parte da qui.
    if matches!(*scenario, Scenario::Playground) {
        ids.0 = scenario.teams() as u32;
        for team in 0..scenario.teams() {
            // Commander hull needs body-aware repair, not the scout margin.
            let radius = archetype(UnitKind::Commander).radius;
            let position = grid.clear_point_for(scenario.center(team), radius);
            spawn_combat_unit(
                &mut commands,
                team as u32,
                Team(team as u8),
                UnitKind::Commander,
                position,
            );
        }
        return;
    }
    ids.0 = (scenario.per_team() * scenario.teams()) as u32;
    let count = scenario.per_team();
    // Only the playground fields armed units at spawn. Skirmish units spawn
    // disarmed (movement-only) and are armed together with their orders at
    // measurement start, keeping the warmup neutral. Plain benchmarks stay
    // movement-only so their numbers remain comparable.
    let combat_demo = matches!(*scenario, Scenario::Playground);
    for team in 0..scenario.teams() {
        // Raw formation ideals are map-blind: repair every slot into the
        // clear so no unit ever spawns inside an obstacle margin (which
        // would fail pathfinding and idle correctness checks).
        // Body-aware repair: large hulls need more than the scout margin.
        let slots: Vec<Vec3> = match *scenario {
            Scenario::Skirmish { .. } => skirmish_slots(count, team, HALF_SIZE),
            _ => formation_slots(count, scenario.center(team), 2.5),
        }
        .into_iter()
        .enumerate()
        .map(|(i, slot)| {
            let kind = match *scenario {
                Scenario::Benchmark { .. } => UnitKind::Tank,
                _ => kind_for_index(team * count + i),
            };
            grid.clear_point_for(slot, archetype(kind).radius)
        })
        .collect();
        for (index, position) in slots.into_iter().enumerate() {
            let global = team * count + index;
            // Playground and skirmish field the same mixed force; plain
            // benchmarks stay uniform tanks so movement numbers remain
            // comparable across versions.
            let kind = match *scenario {
                Scenario::Benchmark { .. } => UnitKind::Tank,
                _ => kind_for_index(global),
            };
            if combat_demo {
                spawn_combat_unit(
                    &mut commands,
                    global as u32,
                    Team(team as u8),
                    kind,
                    position,
                );
                continue;
            }
            let stats = archetype(kind);
            commands.spawn((
                Unit(global as u32),
                Selectable,
                Team(team as u8),
                kind,
                Movement { speed: stats.speed },
                UnitOrder::Idle,
                Transform::from_translation(position + Vec3::Y * UNIT_HALF_SIZE.y),
            ));
        }
    }
}

/// Combat capability bundle shared by gameplay spawns and benchmark orders.
/// Deterministic per numeric id (see desync_phase), so repeats stay
/// comparable. Every value comes from the archetype table: no per-kind
/// branching anywhere in systems code.
pub fn arm_bundle(
    id: u32,
    kind: UnitKind,
) -> (
    UnitKind,
    CollisionRadius,
    Health,
    Weapon,
    WeaponState,
    AcquisitionRange,
    crate::combat::TurretYaw,
) {
    let stats = archetype(kind);
    // Dual-gun units lock targets once for both guns: acquisition covers the
    // longest gun (missiles), each gun still gates fire on its own range.
    let acquisition = archetype::secondary_stats(kind).map_or(
        stats.acquisition,
        |secondary| stats.acquisition.max(secondary.acquisition),
    );
    (
        kind,
        CollisionRadius(stats.radius),
        Health {
            current: stats.max_health,
            max: stats.max_health,
        },
        Weapon {
            range: stats.range,
            cooldown: stats.cooldown,
            damage: stats.damage,
            projectile_speed: stats.projectile_speed,
        },
        WeaponState {
            // Stagger first volleys deterministically (see desync_phase).
            remaining: desync_phase(id) * stats.cooldown,
        },
        AcquisitionRange(acquisition),
        crate::combat::TurretYaw::default(),
    )
}

/// Secondary bundle for dual-gun units (Commander missiles, prova).
/// Returns None for regular units: no extra components, no system cost.
pub fn secondary_bundle(
    id: u32,
    kind: UnitKind,
) -> Option<(SecondaryWeapon, SecondaryWeaponState, SecondaryTurretYaw)> {
    let stats = archetype::secondary_stats(kind)?;
    Some((
        SecondaryWeapon {
            range: stats.range,
            cooldown: stats.cooldown,
            damage: stats.damage,
            projectile_speed: stats.projectile_speed,
        },
        SecondaryWeaponState {
            remaining: desync_phase(id.wrapping_add(0x9E37)) * stats.cooldown,
        },
        SecondaryTurretYaw::default(),
    ))
}

/// Builder bundle from the archetype table: Some for Commander/Engineer,
/// None for troops. Upgrade hook: future levels just swap table values.
pub fn builder_bundle(kind: UnitKind) -> Option<Builder> {
    let stats = archetype(kind);
    if stats.build_power > 0.0 {
        Some(Builder {
            radius: stats.build_radius,
            power: stats.build_power,
        })
    } else {
        None
    }
}
fn setup_visual_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // One body mesh per archetype (sizes differ); team colors stay shared.
    let body_meshes: [Handle<Mesh>; 5] = UnitKind::ALL.map(|kind| {
        let half = archetype(kind).body;
        meshes.add(Cuboid::from_size(half * 2.0))
    });
    let materials_by_team = [
        materials.add(Color::srgb(0.28, 0.58, 0.90)),
        materials.add(Color::srgb(0.90, 0.28, 0.20)),
    ];
    let ring = meshes.add(Annulus::new(0.78, 0.95));
    let ring_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.3, 1.0, 0.4),
        unlit: true,
        ..default()
    });
    let bar_mesh = meshes.add(Cuboid::new(BAR_WIDTH, BAR_HEIGHT, 0.02));
    let bar_bg_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.12, 0.05, 0.05),
        unlit: true,
        ..default()
    });
    let bar_fg_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.25, 0.85, 0.3),
        unlit: true,
        ..default()
    });
    let turret_mesh = meshes.add(
        Cuboid::new(0.3, 0.25, 1.1)
            .mesh()
            .build()
            .translated_by(Vec3::new(0.0, 0.0, -0.35)),
    );
    let turret_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.16, 0.17, 0.20),
        unlit: false,
        ..default()
    });
    // Commander missile pod: boxy launcher on the rear deck, distinct from
    // the mitra barrel.
    let missile_mesh = meshes.add(Cuboid::new(1.2, 0.5, 0.9));
    let missile_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.35, 0.22, 0.12),
        unlit: false,
        ..default()
    });
    // Engineer tool box: orange crate on the hull, distinct from gun barrels.
    let beacon_mesh = meshes.add(Cuboid::new(0.5, 0.4, 0.5));
    let beacon_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.95, 0.6, 0.1),
        unlit: false,
        ..default()
    });
    commands.insert_resource(UnitVisualAssets {
        body_meshes,
        materials_by_team,
        ring,
        ring_material,
        bar_mesh,
        bar_bg_material,
        bar_fg_material,
        turret_mesh,
        turret_material,
        missile_mesh,
        missile_material,
        beacon_mesh,
        beacon_material,
    });
}
#[derive(Resource)]
struct UnitVisualAssets {
    body_meshes: [Handle<Mesh>; 5],
    materials_by_team: [Handle<StandardMaterial>; 2],
    ring: Handle<Mesh>,
    ring_material: Handle<StandardMaterial>,
    bar_mesh: Handle<Mesh>,
    bar_bg_material: Handle<StandardMaterial>,
    bar_fg_material: Handle<StandardMaterial>,
    turret_mesh: Handle<Mesh>,
    turret_material: Handle<StandardMaterial>,
    missile_mesh: Handle<Mesh>,
    missile_material: Handle<StandardMaterial>,
    beacon_mesh: Handle<Mesh>,
    beacon_material: Handle<StandardMaterial>,
}
#[allow(clippy::type_complexity)]
fn add_visuals(
    mut commands: Commands,
    assets: Res<UnitVisualAssets>,
    units: Query<(Entity, &Team, &UnitKind), (With<Unit>, Without<Mesh3d>)>,
) {
    let UnitVisualAssets {
        body_meshes,
        materials_by_team,
        ring,
        ring_material,
        bar_mesh,
        bar_bg_material,
        bar_fg_material,
        turret_mesh,
        turret_material,
        missile_mesh,
        missile_material,
        beacon_mesh,
        beacon_material,
    } = &*assets;
    for (entity, team, kind) in &units {
        // Commander reads much larger: ring scaled by footprint, health bar
        // above the tall hull, mitra barrel + rear missile pod.
        let is_commander = kind.is_commander();
        let stats = archetype(*kind);
        let ring_scale = if is_commander {
            stats.body.xz().length() + 0.6
        } else {
            1.0
        };
        let bar_y = if is_commander {
            stats.body.y + 1.1
        } else {
            BAR_Y
        };
        commands
            .entity(entity)
            .insert((
                Mesh3d(body_meshes[kind.index()].clone()),
                MeshMaterial3d(materials_by_team[team.0 as usize % 2].clone()),
            ))
            .with_children(|parent| {
                parent.spawn((
                    SelectionRing,
                    Mesh3d(ring.clone()),
                    MeshMaterial3d(ring_material.clone()),
                    Transform::from_xyz(0.0, -UNIT_HALF_SIZE.y + 0.04, 0.0)
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                        .with_scale(Vec3::splat(ring_scale)),
                    Visibility::Hidden,
                ));
                parent.spawn((
                    HealthBarBg,
                    Mesh3d(bar_mesh.clone()),
                    MeshMaterial3d(bar_bg_material.clone()),
                    Transform::from_xyz(0.0, bar_y, 0.0).with_scale(Vec3::new(
                        if is_commander { 2.2 } else { 1.0 },
                        1.0,
                        1.0,
                    )),
                ));
                parent.spawn((
                    HealthBarFg,
                    Mesh3d(bar_mesh.clone()),
                    MeshMaterial3d(bar_fg_material.clone()),
                    Transform::from_xyz(0.0, bar_y, 0.011).with_scale(Vec3::new(
                        if is_commander { 2.2 } else { 1.0 },
                        1.0,
                        1.0,
                    )),
                ));
                if stats.armed {
                    parent.spawn((
                        Turret,
                        Mesh3d(turret_mesh.clone()),
                        MeshMaterial3d(turret_material.clone()),
                        Transform::from_xyz(
                            0.0,
                            TURRET_Y + if is_commander { 0.9 } else { 0.0 },
                            0.0,
                        )
                        .with_scale(Vec3::splat(if is_commander { 1.8 } else { 1.0 })),
                    ));
                }
                if is_commander {
                    parent.spawn((
                        SecondaryTurret,
                        Mesh3d(missile_mesh.clone()),
                        MeshMaterial3d(missile_material.clone()),
                        Transform::from_xyz(0.0, TURRET_Y + 1.4, 0.9),
                    ));
                }
                if kind.is_builder() && !stats.armed {
                    // Engineer tool box instead of a barrel.
                    parent.spawn((
                        BuilderBeacon,
                        Mesh3d(beacon_mesh.clone()),
                        MeshMaterial3d(beacon_material.clone()),
                        Transform::from_xyz(0.0, TURRET_Y + 0.1, 0.0),
                    ));
                }
            });
    }
}

/// Turret aim, visual-only: mirror the simulation-owned `TurretYaw` onto
/// the barrel child, relative to the hull. Simulation never reads turret
/// transforms back: determinism untouched. Headless runs spawn no turrets.
#[allow(clippy::type_complexity)]
fn aim_turrets(
    units: Query<(&Transform, &crate::combat::TurretYaw), With<Unit>>,
    mut turrets: Query<
        (&ChildOf, &mut Transform),
        (
            With<Turret>,
            Without<Unit>,
            Without<HealthBarBg>,
            Without<HealthBarFg>,
        ),
    >,
) {
    for (parent, mut transform) in &mut turrets {
        let Ok((body, turret)) = units.get(parent.parent()) else {
            continue;
        };
        let body_yaw = body.rotation.to_euler(EulerRot::YXZ).0;
        transform.rotation = Quat::from_rotation_y(wrap_angle(turret.0 - body_yaw));
    }
}

/// Secondary barrel aim: mirrors `SecondaryTurretYaw` onto the missile pod.
#[allow(clippy::type_complexity)]
fn aim_secondary_turrets(
    units: Query<(&Transform, &crate::combat::SecondaryTurretYaw), With<Unit>>,
    mut turrets: Query<(&ChildOf, &mut Transform), (With<SecondaryTurret>, Without<Unit>)>,
) {
    for (parent, mut transform) in &mut turrets {
        let Ok((body, turret)) = units.get(parent.parent()) else {
            continue;
        };
        let body_yaw = body.rotation.to_euler(EulerRot::YXZ).0;
        let yaw = Quat::from_rotation_y(wrap_angle(turret.0 - body_yaw));
        // Keep pod position, only rotate.
        let pos = transform.translation;
        transform.rotation = yaw;
        transform.translation = pos;
    }
}
/// Camera-facing health bars: copy the camera rotation onto every bar so
/// quads stay readable, shrink the foreground by current/max health.
/// Visual-only transforms on child entities: the simulation never reads
/// them back, so determinism is untouched. Headless runs spawn no bars.
#[allow(clippy::type_complexity)]
fn update_health_bars(
    camera: Option<Single<&GlobalTransform, With<RtsCamera>>>,
    health: Query<&crate::combat::Health>,
    mut backgrounds: Query<
        (&ChildOf, &mut Transform),
        (With<HealthBarBg>, Without<HealthBarFg>, Without<Turret>),
    >,
    mut foregrounds: Query<
        (&ChildOf, &mut Transform),
        (With<HealthBarFg>, Without<HealthBarBg>, Without<Turret>),
    >,
) {
    let Some(camera) = camera else {
        return;
    };
    let rotation = camera.rotation();
    for (_, mut transform) in &mut backgrounds {
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
    }
    for (parent, mut transform) in &mut foregrounds {
        transform.rotation = rotation;
        let fraction = health
            .get(parent.parent())
            .map(|health| health.current / health.max)
            .unwrap_or(1.0);
        let (scale_x, offset_x) = bar_fill(fraction);
        transform.scale.x = scale_x.max(0.0001);
        transform.translation.x = offset_x;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economy::balance::{COMMANDER_BUILD_POWER, COMMAND_BUILD_RADIUS};

    #[test]
    fn health_bar_fill_is_left_anchored_and_clamped() {
        assert_eq!(bar_fill(1.0), (1.0, 0.0));
        assert_eq!(bar_fill(0.0), (0.0, -BAR_WIDTH * 0.5));
        let (scale, offset) = bar_fill(0.5);
        assert!((scale - 0.5).abs() < 0.0001);
        assert!((offset + BAR_WIDTH * 0.25).abs() < 0.0001);
        assert_eq!(bar_fill(1.5), (1.0, 0.0));
        assert_eq!(bar_fill(-0.5), (0.0, -BAR_WIDTH * 0.5));
    }

    #[test]
    fn desync_phases_are_deterministic_and_spread() {
        use crate::combat::desync_phase;
        let phases: Vec<f32> = (1..=16).map(desync_phase).collect();
        assert!(phases.iter().all(|phase| (0.0..1.0).contains(phase)));
        // Same id, same phase across calls.
        assert_eq!(desync_phase(3), phases[2]);
        // Spread out instead of clustered like raw sequential indices.
        let mut sorted = phases.clone();
        sorted.sort_by(f32::total_cmp);
        assert!(sorted.last().unwrap() - sorted.first().unwrap() > 0.5);
        assert!(sorted.windows(2).all(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn commander_has_dual_guns_builder_and_big_hull() {
        use crate::units::archetype::{COMMANDER_MISSILES, secondary_stats};
        // Mitra sul primario, missili sul secondario, mai sulle truppe.
        assert!(secondary_stats(UnitKind::Commander).is_some());
        assert!(secondary_stats(UnitKind::Tank).is_none());
        let (_, _, _, weapon, _, acquisition, _) = arm_bundle(7, UnitKind::Commander);
        let (secondary, _, _) = secondary_bundle(7, UnitKind::Commander).unwrap();
        assert!(secondary_bundle(7, UnitKind::Tank).is_none());
        // Lock unico sul gun più lungo, fuoco gated per gun.
        assert_eq!(acquisition.0, COMMANDER_MISSILES.acquisition.max(weapon.range));
        assert!(secondary.range > weapon.range);
        assert!(secondary.cooldown > weapon.cooldown);
        assert!(secondary.damage > weapon.damage);
        // Scafo molto grande, builder con raggio.
        let stats = archetype(UnitKind::Commander);
        assert!(stats.radius > archetype(UnitKind::Tank).radius * 2.0);
        assert!(stats.max_health >= 1000.0);
        let builder = builder_bundle(UnitKind::Commander).unwrap();
        assert!((builder.radius - COMMAND_BUILD_RADIUS).abs() < 0.001);
        assert!((builder.power - COMMANDER_BUILD_POWER).abs() < 0.001);
        assert!(builder_bundle(UnitKind::Tank).is_none());
        // Engineer: disarmato, mobile, builder leggero producibile.
        let eng = archetype(UnitKind::Engineer);
        assert!(!eng.armed);
        assert!(UnitKind::Engineer.is_builder());
        assert!(UnitKind::PRODUCIBLE.contains(&UnitKind::Engineer));
        assert!(!UnitKind::PRODUCIBLE.contains(&UnitKind::Commander));
        let eb = builder_bundle(UnitKind::Engineer).unwrap();
        assert!(eb.power < builder.power);
    }
}

/// IDs are monotonic across deaths and factory spawns. Never derive from live count.
#[derive(Resource, Default)]
pub struct UnitIds(u32);
impl UnitIds {
    pub fn allocate(&mut self) -> u32 {
        let id = self.0;
        self.0 = self.0.checked_add(1).expect("unit id space exhausted");
        id
    }
}
pub fn spawn_combat_unit(
    commands: &mut Commands,
    id: u32,
    team: Team,
    kind: UnitKind,
    position: Vec3,
) -> Entity {
    let stats = archetype(kind);
    // Tall hulls sit higher off the ground.
    let y = if kind.is_commander() {
        stats.body.y
    } else {
        UNIT_HALF_SIZE.y
    };
    let mut entity = commands.spawn((
        Unit(id),
        Selectable,
        team,
        kind,
        Movement { speed: stats.speed },
        UnitOrder::Idle,
        Transform::from_translation(position.with_y(y)),
        CollisionRadius(stats.radius),
        Health {
            current: stats.max_health,
            max: stats.max_health,
        },
    ));
    if stats.armed {
        let (_, _, _, weapon, weapon_state, acquisition, turret) = arm_bundle(id, kind);
        entity.insert((weapon, weapon_state, acquisition, turret));
    }
    if let Some(secondary) = secondary_bundle(id, kind) {
        entity.insert(secondary);
    }
    if let Some(builder) = builder_bundle(kind) {
        entity.insert(builder);
    }
    if kind.is_commander() {
        entity.insert(Commander);
    }
    entity.id()
}
