use crate::{
    camera::RtsCamera,
    combat::{AcquisitionRange, Health, Weapon, WeaponState, desync_phase},
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
        app.add_systems(Startup, spawn_units);
        if self.visuals {
            app.add_systems(Startup, add_visuals.after(spawn_units));
        }
        app.add_systems(PostUpdate, (update_health_bars, aim_turrets));
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

/// Left-anchored fill transform for a unit-health fraction in [0, 1].
/// Returns (scale_x, local_x_offset): the shared quad keeps full width for
/// the background, the foreground shrinks toward the left edge.
pub fn bar_fill(fraction: f32) -> (f32, f32) {
    let clamped = fraction.clamp(0.0, 1.0);
    (clamped, -(1.0 - clamped) * BAR_WIDTH * 0.5)
}

fn spawn_units(mut commands: Commands, scenario: Res<Scenario>, grid: Res<NavGrid>) {
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
        let slots: Vec<Vec3> = match *scenario {
            Scenario::Skirmish { .. } => skirmish_slots(count, team, HALF_SIZE),
            _ => formation_slots(count, scenario.center(team), 2.5),
        }
        .into_iter()
        .map(|slot| grid.clear_point(slot))
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
            let stats = archetype(kind);
            let mut unit = commands.spawn((
                Unit(global as u32),
                Selectable,
                Team(team as u8),
                kind,
                Movement { speed: stats.speed },
                UnitOrder::Idle,
                Transform::from_translation(position + Vec3::Y * UNIT_HALF_SIZE.y),
            ));
            if combat_demo {
                // Armed but standing: no orders at spawn, so the playground
                // is a manual test bench (right-click move, G attack-move,
                // H hold, S stop) instead of an auto-battle.
                unit.insert(arm_bundle(global as u32, kind));
            }
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
        AcquisitionRange(stats.acquisition),
        crate::combat::TurretYaw::default(),
    )
}

fn add_visuals(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    units: Query<(Entity, &Team, &UnitKind), With<Unit>>,
) {
    // One body mesh per archetype (sizes differ); team colors stay shared.
    let body_meshes: [Handle<Mesh>; 3] = UnitKind::ALL.map(|kind| {
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
    for (entity, team, kind) in &units {
        commands
            .entity(entity)
            .insert((
                Mesh3d(body_meshes[kind.index()].clone()),
                MeshMaterial3d(materials_by_team[team.0 as usize].clone()),
            ))
            .with_children(|parent| {
                parent.spawn((
                    SelectionRing,
                    Mesh3d(ring.clone()),
                    MeshMaterial3d(ring_material.clone()),
                    Transform::from_xyz(0.0, -UNIT_HALF_SIZE.y + 0.04, 0.0)
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
                    Visibility::Hidden,
                ));
                parent.spawn((
                    HealthBarBg,
                    Mesh3d(bar_mesh.clone()),
                    MeshMaterial3d(bar_bg_material.clone()),
                    Transform::from_xyz(0.0, BAR_Y, 0.0),
                ));
                parent.spawn((
                    HealthBarFg,
                    Mesh3d(bar_mesh.clone()),
                    MeshMaterial3d(bar_fg_material.clone()),
                    Transform::from_xyz(0.0, BAR_Y, 0.011),
                ));
                parent.spawn((
                    Turret,
                    Mesh3d(turret_mesh.clone()),
                    MeshMaterial3d(turret_material.clone()),
                    Transform::from_xyz(0.0, TURRET_Y, 0.0),
                ));
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
/// Camera-facing health bars: copy the camera rotation onto every bar so
/// quads stay readable, shrink the foreground by current/max health.
/// Visual-only transforms on child entities: the simulation never reads
/// them back, so determinism is untouched. Headless runs spawn no bars.
#[allow(clippy::type_complexity)]
fn update_health_bars(
    camera: Option<Single<&GlobalTransform, With<RtsCamera>>>,
    health: Query<&crate::combat::Health, With<Unit>>,
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
}
