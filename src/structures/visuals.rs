use super::*;
use crate::{
    selection::Selected,
    units::{BAR_HEIGHT, BAR_WIDTH, HealthBarBg, HealthBarFg, SelectionRing},
};
use bevy::prelude::*;

pub struct BuildingVisualsPlugin;
impl Plugin for BuildingVisualsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_assets)
            .add_systems(PostUpdate, (add_visuals, show_progress).chain());
    }
}
#[derive(Resource)]
struct BuildingAssets {
    bodies: [Handle<Mesh>; 3],
    accents: [Handle<Mesh>; 3],
    team: [Handle<StandardMaterial>; 2],
    detail: [Handle<StandardMaterial>; 3],
    site: Handle<StandardMaterial>,
    ring: Handle<Mesh>,
    selected: Handle<StandardMaterial>,
    bar: Handle<Mesh>,
    dark: Handle<StandardMaterial>,
    green: Handle<StandardMaterial>,
}
#[derive(Component)]
struct BuildingBody;
fn setup_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(BuildingAssets {
        bodies: BuildingKind::ALL.map(|k| {
            let s = k.stats();
            meshes.add(Cuboid::new(s.half.x * 2.0, s.height, s.half.y * 2.0))
        }),
        accents: [
            meshes.add(Cylinder::new(1.0, 2.5)),
            meshes.add(Cuboid::new(5.6, 0.15, 3.6)),
            meshes.add(Cuboid::new(6.0, 3.2, 0.2)),
        ],
        team: [
            materials.add(Color::srgb(0.15, 0.45, 0.7)),
            materials.add(Color::srgb(0.7, 0.2, 0.12)),
        ],
        detail: [
            materials.add(Color::srgb(0.95, 0.62, 0.15)),
            materials.add(Color::srgb(0.06, 0.12, 0.35)),
            materials.add(Color::srgb(0.06, 0.08, 0.09)),
        ],
        site: materials.add(Color::srgb(0.5, 0.4, 0.2)),
        ring: meshes.add(Annulus::new(0.94, 1.0)),
        selected: materials.add(StandardMaterial {
            base_color: Color::srgb(0.3, 1.0, 0.4),
            unlit: true,
            ..default()
        }),
        bar: meshes.add(Cuboid::new(BAR_WIDTH, BAR_HEIGHT, 0.02)),
        dark: materials.add(StandardMaterial {
            base_color: Color::srgb(0.12, 0.05, 0.05),
            unlit: true,
            ..default()
        }),
        green: materials.add(StandardMaterial {
            base_color: Color::srgb(0.25, 0.85, 0.3),
            unlit: true,
            ..default()
        }),
    });
}
#[allow(clippy::type_complexity)]
fn add_visuals(
    mut commands: Commands,
    assets: Res<BuildingAssets>,
    buildings: Query<(Entity, &BuildingKind, &Team), (With<Building>, Without<Visibility>)>,
) {
    for (entity, kind, team) in &buildings {
        let s = kind.stats();
        commands
            .entity(entity)
            .insert(Visibility::Inherited)
            .with_children(|p| {
                p.spawn((
                    BuildingBody,
                    Mesh3d(assets.bodies[*kind as usize].clone()),
                    MeshMaterial3d(assets.team[team.0 as usize % 2].clone()),
                    Transform::default(),
                ));
                let accent_y = if *kind == BuildingKind::Factory {
                    -0.2
                } else {
                    s.height * 0.5 + 0.5
                };
                let accent_z = if *kind == BuildingKind::Factory {
                    s.half.y + 0.03
                } else {
                    0.0
                };
                p.spawn((
                    Mesh3d(assets.accents[*kind as usize].clone()),
                    MeshMaterial3d(assets.detail[*kind as usize].clone()),
                    Transform::from_xyz(0.0, accent_y, accent_z),
                ));
                p.spawn((
                    SelectionRing,
                    Mesh3d(assets.ring.clone()),
                    MeshMaterial3d(assets.selected.clone()),
                    Transform::from_xyz(0.0, -s.height * 0.5 + 0.07, 0.0)
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                        .with_scale(Vec3::splat(s.half.length() + 0.4)),
                    Visibility::Hidden,
                ));
                p.spawn((
                    HealthBarBg,
                    Mesh3d(assets.bar.clone()),
                    MeshMaterial3d(assets.dark.clone()),
                    Transform::from_xyz(0.0, s.height * 0.5 + 2.0, 0.0),
                ));
                p.spawn((
                    HealthBarFg,
                    Mesh3d(assets.bar.clone()),
                    MeshMaterial3d(assets.green.clone()),
                    Transform::from_xyz(0.0, s.height * 0.5 + 2.0, 0.011),
                ));
            });
    }
}
fn show_progress(
    assets: Res<BuildingAssets>,
    buildings: Query<(&BuildingKind, &Team, Option<&Construction>)>,
    mut bodies: Query<
        (
            &ChildOf,
            &mut Transform,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        With<BuildingBody>,
    >,
) {
    for (parent, mut transform, mut material) in &mut bodies {
        let Ok((kind, team, site)) = buildings.get(parent.parent()) else {
            continue;
        };
        let fraction = site.map_or(1.0, |s| s.0.fraction() as f32).max(0.08);
        transform.scale.y = fraction;
        transform.translation.y = -(1.0 - fraction) * kind.stats().height * 0.5;
        material.0 = if site.is_some() {
            assets.site.clone()
        } else {
            assets.team[team.0 as usize % 2].clone()
        };
    }
}

#[allow(clippy::type_complexity)]
pub fn draw_rallies(
    mut gizmos: Gizmos,
    factories: Query<(&Transform, &Factory), (With<Selected>, Without<Construction>)>,
) {
    for (transform, factory) in &factories {
        if let Some(point) = factory.rally {
            gizmos.line(
                transform.translation,
                point.with_y(0.12),
                Color::srgb(0.1, 0.9, 1.0),
            );
            gizmos.circle(
                Isometry3d::new(
                    point.with_y(0.12),
                    Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
                ),
                1.5,
                Color::srgb(0.1, 0.9, 1.0),
            );
        }
    }
}
