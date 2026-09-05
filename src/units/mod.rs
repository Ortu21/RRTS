use crate::{formation::formation_slots, movement::Movement, scenario::Scenario};
use bevy::prelude::*;

pub const UNIT_HALF_SIZE: Vec3 = Vec3::new(0.55, 0.8, 0.55);
pub struct UnitPlugin {
    pub visuals: bool,
}
impl Plugin for UnitPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_units);
        if self.visuals {
            app.add_systems(Startup, add_visuals.after(spawn_units));
        }
    }
}
#[derive(Component)]
pub struct Unit(pub u32);
#[derive(Component)]
pub struct Selectable;
#[derive(Component, PartialEq, Eq)]
pub struct Team(pub u8);
pub const PLAYER_TEAM: Team = Team(0);
#[derive(Component)]
pub struct SelectionRing;

fn spawn_units(mut commands: Commands, scenario: Res<Scenario>) {
    let count = scenario.per_team();
    for team in 0..scenario.teams() {
        for (index, position) in formation_slots(count, scenario.center(team), 2.5)
            .into_iter()
            .enumerate()
        {
            commands.spawn((
                Unit((team * count + index) as u32),
                Selectable,
                Team(team as u8),
                Movement { speed: 7.0 },
                Transform::from_translation(position + Vec3::Y * UNIT_HALF_SIZE.y),
            ));
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
            });
    }
}
