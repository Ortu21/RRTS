use crate::{
    camera::RtsCamera,
    combat::{AcquisitionRange, WeaponState, default_weapon, desync_phase, full_health},
    formation::{formation_slots, skirmish_slots},
    movement::{MoveTarget, Movement},
    orders::UnitOrder,
    scenario::Scenario,
    spatial::DEFAULT_UNIT_RADIUS,
};
use bevy::prelude::*;

pub const UNIT_HALF_SIZE: Vec3 = Vec3::new(0.55, 0.8, 0.55);
/// Health bar dimensions: full-width quad floating above the unit.
pub const BAR_WIDTH: f32 = 1.3;
pub const BAR_HEIGHT: f32 = 0.1;
pub const BAR_Y: f32 = 1.25;
pub struct UnitPlugin {
    pub visuals: bool,
}
impl Plugin for UnitPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_units);
        if self.visuals {
            app.add_systems(Startup, add_visuals.after(spawn_units));
        }
        app.add_systems(PostUpdate, update_health_bars);
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

/// Left-anchored fill transform for a unit-health fraction in [0, 1].
/// Returns (scale_x, local_x_offset): the shared quad keeps full width for
/// the background, the foreground shrinks toward the left edge.
pub fn bar_fill(fraction: f32) -> (f32, f32) {
    let clamped = fraction.clamp(0.0, 1.0);
    (clamped, -(1.0 - clamped) * BAR_WIDTH * 0.5)
}

fn spawn_units(mut commands: Commands, scenario: Res<Scenario>) {
    let count = scenario.per_team();
    // Only the playground and skirmish scenarios field armed units.
    // Plain benchmarks stay movement-only so their numbers remain comparable.
    let combat_demo = matches!(*scenario, Scenario::Playground | Scenario::Skirmish { .. });
    for team in 0..scenario.teams() {
        let slots = match *scenario {
            Scenario::Skirmish { .. } => skirmish_slots(count, team),
            _ => formation_slots(count, scenario.center(team), 2.5),
        };
        for (index, position) in slots.into_iter().enumerate() {
            let mut unit = commands.spawn((
                Unit((team * count + index) as u32),
                Selectable,
                Team(team as u8),
                Movement { speed: 7.0 },
                UnitOrder::Idle,
                Transform::from_translation(position + Vec3::Y * UNIT_HALF_SIZE.y),
            ));
            if combat_demo {
                let weapon = default_weapon();
                let id = unit.id();
                unit.insert((
                    CollisionRadius(DEFAULT_UNIT_RADIUS),
                    full_health(),
                    weapon,
                    WeaponState {
                        // Stagger first volleys deterministically (see desync_phase).
                        remaining: desync_phase(id) * weapon.cooldown,
                    },
                    AcquisitionRange(30.0),
                ));
                if matches!(*scenario, Scenario::Playground) {
                    // The demo fights immediately; benchmark orders are
                    // issued at measurement start for timing symmetry.
                    let destination = scenario.attack_target(team);
                    unit.insert((
                        UnitOrder::AttackMove { destination },
                        MoveTarget(destination),
                    ));
                }
            }
        }
    }
}

fn add_visuals(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    units: Query<(Entity, &Team), With<Unit>>,
) {
    let body = meshes.add(Cuboid::from_size(UNIT_HALF_SIZE * 2.0));
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
    for (entity, team) in &units {
        commands
            .entity(entity)
            .insert((
                Mesh3d(body.clone()),
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
            });
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
    mut backgrounds: Query<(&ChildOf, &mut Transform), (With<HealthBarBg>, Without<HealthBarFg>)>,
    mut foregrounds: Query<(&ChildOf, &mut Transform), (With<HealthBarFg>, Without<HealthBarBg>)>,
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
        let phases: Vec<f32> = (1..=16)
            .map(|index| desync_phase(Entity::from_bits(index)))
            .collect();
        assert!(phases.iter().all(|phase| (0.0..1.0).contains(phase)));
        // Same entity, same phase across calls.
        assert_eq!(desync_phase(Entity::from_bits(3)), phases[2]);
        // Spread out instead of clustered like raw sequential indices.
        let mut sorted = phases.clone();
        sorted.sort_by(f32::total_cmp);
        assert!(sorted.last().unwrap() - sorted.first().unwrap() > 0.5);
        assert!(sorted.windows(2).all(|pair| pair[0] != pair[1]));
    }
}
