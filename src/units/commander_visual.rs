//! Official Blender GLB: independent weapon aim, rigid-part clips, team armor.
//! The scene is optional presentation; simulation never reads imported transforms.

use super::{
    Builder, Commander, SecondaryTurret, Team, Turret, UnitKind, UnitVisualsPresent, archetype,
};
use crate::{
    combat::{SecondaryTurretYaw, TurretYaw},
    movement::wrap_angle,
    orders::UnitOrder,
};
use bevy::{
    app::AnimationSystems, gltf::GltfMaterialName, prelude::*, transform::TransformSystems,
    world_serialization::WorldInstanceReady,
};

const MODEL: &str = "models/commander/commander.glb";
const CLIPS: [&str; 6] = [
    "Idle",
    "Move_L",
    "Move_R",
    "Fire_Primary",
    "Fire_Secondary",
    "Deploy",
];
// Ground-relative glTF socket contract, checked by scripts/check_commander_asset.py.
const PRIMARY_PIVOT: Vec3 = Vec3::new(0.0, 1.283325, -0.741);
const PRIMARY_MUZZLE: Vec3 = Vec3::new(0.0, 0.19525, -0.6954);
const SECONDARY_PIVOT: Vec3 = Vec3::new(0.0, 2.003975, 0.741);
const SECONDARY_MUZZLE: Vec3 = Vec3::new(0.0, 0.18815, -0.25935);

/// Deterministic socket position, also available headless. No asset-loading timing
/// or visual hierarchy can change ballistics. The root stays at body center.
pub(crate) fn muzzle(body: &Transform, world_yaw: f32, secondary: bool) -> Vec3 {
    let (pivot, tip) = if secondary {
        (SECONDARY_PIVOT, SECONDARY_MUZZLE)
    } else {
        (PRIMARY_PIVOT, PRIMARY_MUZZLE)
    };
    body.translation
        + body.rotation * (pivot - Vec3::Y * archetype(UnitKind::Commander).body.y)
        + Quat::from_rotation_y(world_yaw) * tip
}

/// Written only after successful fire, and only when graphics registered messages.
#[derive(Message, Clone, Copy)]
pub(crate) struct CommanderShot {
    pub owner: Entity,
    pub secondary: bool,
}

pub(super) struct CommanderVisualPlugin;
impl Plugin for CommanderVisualPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<CommanderShot>()
            .add_systems(Startup, load)
            .add_systems(Update, (prepare, spawn).chain())
            .add_systems(PostUpdate, animate.before(AnimationSystems))
            .add_systems(
                PostUpdate,
                aim.after(AnimationSystems)
                    .before(TransformSystems::Propagate),
            );
    }
}

#[derive(Resource)]
struct ModelAsset {
    gltf: Handle<Gltf>,
    ready: Option<ReadyModel>,
    failed: bool,
    team_materials: [Handle<StandardMaterial>; 2],
}
struct ReadyModel {
    scene: Handle<bevy::world_serialization::WorldAsset>,
    graph: Handle<AnimationGraph>,
    clips: [AnimationNodeIndex; 6],
}
#[derive(Component)]
struct ModelStarted;
#[derive(Component)]
struct ModelOwner(Entity);
#[derive(Component)]
struct Mount {
    owner: Entity,
    secondary: bool,
}
#[derive(Component)]
struct Motion {
    owner: Entity,
    previous_position: Vec3,
    previous_yaw: f32,
    deployment: f32,
}

fn load(
    mut commands: Commands,
    server: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let team_materials =
        [Color::srgb(0.28, 0.58, 0.90), Color::srgb(0.72, 0.27, 0.18)].map(|base_color| {
            materials.add(StandardMaterial {
                base_color,
                metallic: 0.5,
                perceptual_roughness: 0.4,
                ..default()
            })
        });
    commands.insert_resource(ModelAsset {
        gltf: server.load(MODEL),
        ready: None,
        failed: false,
        team_materials,
    });
}

fn prepare(
    mut asset: ResMut<ModelAsset>,
    gltfs: Res<Assets<Gltf>>,
    server: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
) {
    if asset.ready.is_some() || asset.failed {
        return;
    }
    let Some(gltf) = gltfs.get(&asset.gltf) else {
        if let Some(bevy::asset::LoadState::Failed(error)) = server.get_load_state(asset.gltf.id())
        {
            warn!("Commander GLB unavailable; keeping procedural fallback: {error}");
            asset.failed = true;
        }
        return;
    };
    let Some(scene) = gltf
        .default_scene
        .clone()
        .or_else(|| gltf.scenes.first().cloned())
    else {
        error!("Commander GLB has no scene");
        asset.failed = true;
        return;
    };
    let mut graph = AnimationGraph::new();
    let mut nodes = Vec::new();
    for name in CLIPS {
        let Some(clip) = gltf.named_animations.get(name) else {
            error!("Commander GLB is missing clip {name}");
            asset.failed = true;
            return;
        };
        nodes.push(graph.add_clip(clip.clone(), 1.0, graph.root));
    }
    asset.ready = Some(ReadyModel {
        scene,
        graph: graphs.add(graph),
        clips: nodes.try_into().unwrap(),
    });
}

#[allow(clippy::type_complexity)]
fn spawn(
    mut commands: Commands,
    asset: Res<ModelAsset>,
    units: Query<
        Entity,
        (
            With<Commander>,
            With<UnitVisualsPresent>,
            Without<ModelStarted>,
        ),
    >,
) {
    let Some(model) = &asset.ready else {
        return;
    };
    for owner in &units {
        commands
            .entity(owner)
            .insert(ModelStarted)
            .with_children(|parent| {
                parent
                    .spawn((
                        Name::new("Commander model"),
                        ModelOwner(owner),
                        WorldAssetRoot(model.scene.clone()),
                        Transform::from_xyz(0.0, -archetype(UnitKind::Commander).body.y, 0.0),
                    ))
                    .observe(ready);
            });
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn ready(
    event: On<WorldInstanceReady>,
    mut commands: Commands,
    asset: Res<ModelAsset>,
    owners: Query<&ModelOwner>,
    units: Query<(&Team, &Transform), With<Commander>>,
    children: Query<&Children>,
    names: Query<&Name>,
    placeholders: Query<(), Or<(With<Turret>, With<SecondaryTurret>)>>,
    materials: Query<&GltfMaterialName>,
    mut players: Query<&mut AnimationPlayer>,
) {
    let Ok(owner) = owners.get(event.entity) else {
        return;
    };
    let Ok((team, body)) = units.get(owner.0) else {
        return;
    };
    let Some(model) = &asset.ready else {
        return;
    };
    // The fallback remains visible until the real scene is actually instantiated.
    commands
        .entity(owner.0)
        .remove::<(Mesh3d, MeshMaterial3d<StandardMaterial>)>();
    if let Ok(direct) = children.get(owner.0) {
        for child in direct.iter() {
            if placeholders.contains(child) {
                commands.entity(child).despawn();
            }
        }
    }
    for child in children.iter_descendants(event.entity) {
        if let Ok(name) = names.get(child) {
            match name.as_str() {
                "MG_Yaw" | "Rocket_Yaw" => {
                    commands.entity(child).insert(Mount {
                        owner: owner.0,
                        secondary: name.as_str() == "Rocket_Yaw",
                    });
                }
                _ => {}
            }
        }
        if materials.get(child).is_ok_and(|m| m.0 == "TEAM_ARMOR") {
            commands.entity(child).insert(MeshMaterial3d(
                asset.team_materials[team.0 as usize % 2].clone(),
            ));
        }
        if let Ok(mut player) = players.get_mut(child) {
            player.play(model.clips[0]).repeat();
            player.play(model.clips[5]).pause();
            commands.entity(child).insert((
                AnimationGraphHandle(model.graph.clone()),
                Motion {
                    owner: owner.0,
                    previous_position: body.translation,
                    previous_yaw: body.rotation.to_euler(EulerRot::YXZ).0,
                    deployment: 0.0,
                },
            ));
        }
    }
    info!(
        "Commander GLB ready for team {} (mitra + guided launcher)",
        team.0
    );
}

fn aim(
    units: Query<(&Transform, &TurretYaw, &SecondaryTurretYaw), With<Commander>>,
    mut mounts: Query<(&Mount, &mut Transform), Without<Commander>>,
) {
    for (mount, mut transform) in &mut mounts {
        let Ok((body, primary, secondary)) = units.get(mount.owner) else {
            continue;
        };
        let yaw = if mount.secondary {
            secondary.0
        } else {
            primary.0
        };
        transform.rotation =
            Quat::from_rotation_y(wrap_angle(yaw - body.rotation.to_euler(EulerRot::YXZ).0));
    }
}

fn animate(
    time: Res<Time>,
    asset: Res<ModelAsset>,
    mut shots: MessageReader<CommanderShot>,
    units: Query<(&Transform, &UnitOrder, &Builder), With<Commander>>,
    sites: Query<&Transform, With<crate::structures::Construction>>,
    mut players: Query<(&mut AnimationPlayer, &mut Motion)>,
) {
    let fired: Vec<_> = shots.read().copied().collect();
    let Some(model) = &asset.ready else {
        return;
    };
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (mut player, mut motion) in &mut players {
        let Ok((body, order, builder)) = units.get(motion.owner) else {
            continue;
        };
        let displacement = body.translation - motion.previous_position;
        let yaw = body.rotation.to_euler(EulerRot::YXZ).0;
        let forward = body.rotation * Vec3::NEG_Z;
        let speed = (displacement.dot(forward) / dt).clamp(-8.0, 8.0);
        let turning = (wrap_angle(yaw - motion.previous_yaw) / dt).clamp(-3.0, 3.0);
        // The two running gears animate independently, including in-place turns.
        for (index, velocity) in [(1, speed + turning * 1.254), (2, speed - turning * 1.254)] {
            player
                .play(model.clips[index])
                .repeat()
                .set_speed(-velocity / 1.516);
        }
        let building = match order {
            UnitOrder::Build { site } if speed.abs() < 0.1 => sites.get(*site).is_ok_and(|site| {
                body.translation.xz().distance(site.translation.xz()) <= builder.radius
            }),
            _ => false,
        };
        let target = if building { 1.0 } else { 0.0 };
        motion.deployment = move_toward(motion.deployment, target, dt);
        player
            .play(model.clips[5])
            .pause()
            .set_seek_time(motion.deployment);
        for shot in fired.iter().filter(|s| s.owner == motion.owner) {
            player.start(model.clips[if shot.secondary { 4 } else { 3 }]);
        }
        motion.previous_position = body.translation;
        motion.previous_yaw = yaw;
    }
}

fn move_toward(value: f32, target: f32, step: f32) -> f32 {
    value + (target - value).clamp(-step, step)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commander_sockets_follow_hull_and_independent_guns() {
        let body = Transform::from_xyz(10.0, 1.6, 20.0);
        assert!(muzzle(&body, 0.0, false).abs_diff_eq(Vec3::new(10.0, 1.478575, 18.5636), 0.0001));
        assert!(muzzle(&body, 0.0, true).abs_diff_eq(Vec3::new(10.0, 2.192125, 20.48165), 0.0001));
        let rotated = body.with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        let independently_aimed = muzzle(&rotated, 0.0, false);
        assert!((independently_aimed.x - 9.259).abs() < 0.0001);
        assert!((independently_aimed.z - 19.3046).abs() < 0.0001);
    }
    #[test]
    fn deployment_reverses_without_snapping_or_overshooting() {
        assert_eq!(move_toward(0.9, 1.0, 0.3), 1.0);
        assert_eq!(move_toward(0.1, 0.0, 0.3), 0.0);
        assert!((move_toward(0.6, 0.0, 0.1) - 0.5).abs() < 0.00001);
    }
}
