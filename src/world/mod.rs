use bevy::prelude::*;

pub use crate::navigation::HALF_SIZE as GROUND_HALF_SIZE;
use crate::navigation::NavGrid;

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_world);
    }
}

fn setup_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    grid: Res<NavGrid>,
) {
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(200.0, 200.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.21, 0.29, 0.23))),
    ));
    let line_mesh = meshes.add(Cuboid::new(0.035, 0.015, 200.0));
    let line_material = materials.add(Color::srgb(0.30, 0.38, 0.31));
    for index in -20..=20 {
        for rotation in [0.0, std::f32::consts::FRAC_PI_2] {
            let rotation = Quat::from_rotation_y(rotation);
            commands.spawn((
                Mesh3d(line_mesh.clone()),
                MeshMaterial3d(line_material.clone()),
                Transform::from_translation(rotation * Vec3::new(index as f32 * 5.0, 0.01, 0.0))
                    .with_rotation(rotation),
            ));
        }
    }
    let obstacle_material = materials.add(Color::srgb(0.44, 0.40, 0.32));
    for obstacle in &grid.obstacles {
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(
                obstacle.half_size.x * 2.0,
                3.0,
                obstacle.half_size.y * 2.0,
            ))),
            MeshMaterial3d(obstacle_material.clone()),
            Transform::from_xyz(obstacle.center.x, 1.5, obstacle.center.y),
        ));
    }
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            ..default()
        },
        Transform::from_xyz(40.0, 60.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}
