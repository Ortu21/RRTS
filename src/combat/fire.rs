//! Combattimento: HP, armi, targeting guidato da ordini, proiettili, morti.
//!
//! Regole BAR-style: `AttackMove` si ferma e riprende, `Move` spara marciando,
//! `Hold`/`Idle` difendono in piedi. Targeting sempre gated su fog (`can_target`).
//! I marker `Chasing`/`HoldFire` sono locomozione, l'intento resta in `orders::UnitOrder`.

//! Fuoco: armi, cooldown, traverse, spawn colpi.

use super::acquire::{AttackTarget, CombatClock, target_position};
use super::death::{Health, is_dead};
use super::projectile::{
    BEAM_TTL_SECS, BeamFlash, MORTAR_ARC_HEIGHT, PLUNGE_HEIGHT, Projectile, ProjectileKind,
    in_weapon_range, spread_offset,
};
use crate::{
    spatial::SpatialGrid,
    units::{Team, Unit, UnitKind, archetype::WeaponTech},
};
use bevy::prelude::*;

#[derive(Component, Debug, Clone, Copy)]
pub struct Weapon {
    pub range: f32,
    pub cooldown: f32,
    pub damage: f32,
    pub projectile_speed: f32,
}

/// Primary gun profile from the archetype table (`tech`/`spread`/`splash`).
/// Turrets and test dummies carry a bare `Weapon` with no profile and keep
/// the legacy precise-homing path: zero behaviour drift outside real units.
#[derive(Component, Debug, Clone, Copy)]
pub struct WeaponProfile {
    pub tech: WeaponTech,
    pub spread_rad: f32,
    pub splash: f32,
}

impl Default for WeaponProfile {
    fn default() -> Self {
        Self {
            tech: WeaponTech::Homing,
            spread_rad: 0.0,
            splash: 0.0,
        }
    }
}

#[derive(Component, Debug, Clone, Copy, Default)]
pub struct WeaponState {
    pub remaining: f32,
}

/// Secondary weapon (Commander missiles, Vanguard lasers). Shares the same
/// `AttackTarget` lock as the primary: no separate acquisition pass.
/// Own tech/range/cooldown/yaw so the two guns feel different and can be
/// tuned independently. Tech fields mirror `WeaponProfile`: both guns run the
/// same fire/move code paths, never per-kind branches.
#[derive(Component, Debug, Clone, Copy)]
pub struct SecondaryWeapon {
    pub tech: WeaponTech,
    pub range: f32,
    pub cooldown: f32,
    pub damage: f32,
    pub projectile_speed: f32,
    pub traverse: f32,
    pub aim_tolerance: f32,
    pub spread_rad: f32,
    pub splash: f32,
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

/// Turret world yaw, owned by the simulation. The visual barrel mirrors it;
/// fire is gated on it (see aim tolerance), so traverse rate is real DPS
/// handling rather than decoration.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct TurretYaw(pub f32);

pub(crate) fn tick_cooldowns(
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
pub(crate) fn traverse_turrets(
    time: Res<Time>,
    grid: Res<SpatialGrid>,
    mut units: Query<
        (
            &Transform,
            Option<&UnitKind>,
            Option<&crate::structures::Turret>,
            &mut TurretYaw,
            Option<&AttackTarget>,
        ),
        (
            Or<(With<Unit>, With<crate::structures::Building>)>,
            With<Weapon>,
        ),
    >,
) {
    use crate::movement::{rotate_toward, yaw_toward};
    use crate::units::archetype;

    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (transform, kind, turret, mut yaw, target) in &mut units {
        // Traverse rate comes from the unit table or the turret table;
        // every shooter carries exactly one of the two markers.
        let traverse = match (kind, turret) {
            (Some(k), _) => archetype(*k).traverse,
            (None, Some(_)) => {
                crate::economy::balance::turret_stats(crate::economy::balance::BuildingKind::Turret)
                    .map_or(0.0, |s| s.traverse)
            }
            (None, None) => 0.0,
        };
        let body_yaw = transform.rotation.to_euler(EulerRot::YXZ).0;
        let aim = target
            .and_then(|target| grid.position(target.0))
            .filter(|aim| aim.xz().distance_squared(transform.translation.xz()) > f32::EPSILON)
            .map(|aim| yaw_toward(aim - transform.translation))
            .unwrap_or(body_yaw);
        yaw.0 = rotate_toward(yaw.0, aim, traverse * dt);
    }
}

/// Secondary traverse: same lock, per-gun yaw rate from the secondary spec.
/// Slow secondaries (missiles) still traverse behind the mitra at close
/// range: double gun feeling, now data-driven per dual-gun unit.
#[allow(clippy::type_complexity)]
pub(crate) fn traverse_secondary(
    time: Res<Time>,
    grid: Res<SpatialGrid>,
    mut units: Query<
        (
            &Transform,
            &SecondaryWeapon,
            &mut SecondaryTurretYaw,
            Option<&AttackTarget>,
        ),
        (With<Unit>, With<SecondaryWeapon>),
    >,
) {
    use crate::movement::{rotate_toward, yaw_toward};

    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (transform, gun, mut turret, target) in &mut units {
        let body_yaw = transform.rotation.to_euler(EulerRot::YXZ).0;
        let aim = target
            .and_then(|target| grid.position(target.0))
            .filter(|aim| aim.xz().distance_squared(transform.translation.xz()) > f32::EPSILON)
            .map(|aim| yaw_toward(aim - transform.translation))
            .unwrap_or(body_yaw);
        turret.0 = rotate_toward(turret.0, aim, gun.traverse * dt);
    }
}

#[derive(Resource)]

pub(crate) struct ProjectileAssets {
    mesh: Handle<Mesh>,
    team_material: [Handle<StandardMaterial>; 2],
    /// Elongated tracer for guns (unit box, stretched per shot).
    tracer_mesh: Handle<Mesh>,
    /// Dark shell for dumbfire rockets and mortar rounds.
    shell_mesh: Handle<Mesh>,
    shell_material: Handle<StandardMaterial>,
    /// Shared unit cube for beams/columns + hot flash material.
    beam_mesh: Handle<Mesh>,
    beam_material: Handle<StandardMaterial>,
}

pub(crate) fn setup_projectile_assets(
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
        tracer_mesh: meshes.add(Cuboid::new(0.12, 0.12, 1.0)),
        shell_mesh: meshes.add(Sphere::new(0.28)),
        shell_material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.15, 0.14, 0.13),
            emissive: Color::srgb(0.9, 0.35, 0.05).into(),
            unlit: true,
            ..default()
        }),
        beam_mesh: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        beam_material: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.98, 0.85),
            emissive: Color::srgb(1.0, 0.95, 0.6).into(),
            unlit: true,
            ..default()
        }),
    });
}

/// Shared beam-flash spawner: stretched glowing box from→to, shrinks away in
/// `tick_beams`. Visual only, never sim state.
pub(crate) fn spawn_beam(
    commands: &mut Commands,
    assets: &ProjectileAssets,
    from: Vec3,
    to: Vec3,
    width: f32,
    ttl: f32,
) {
    let dir = to - from;
    let len = dir.length().max(0.5);
    let mut transform = Transform::from_translation((from + to) * 0.5);
    if dir.length_squared() > f32::EPSILON {
        transform = transform.looking_at(to, Vec3::Y);
    }
    transform.scale = Vec3::new(width, width, len);
    commands.spawn((
        BeamFlash { ttl, life: ttl },
        transform,
        Mesh3d(assets.beam_mesh.clone()),
        MeshMaterial3d(assets.beam_material.clone()),
    ));
}

#[allow(clippy::type_complexity)]
pub(crate) fn fire_weapons(
    mut commands: Commands,
    mut commander_shots: Option<ResMut<Messages<crate::units::commander_visual::CommanderShot>>>,
    grid: Res<SpatialGrid>,
    clock: Res<CombatClock>,
    assets: Option<Res<ProjectileAssets>>,
    health: Query<&Health>,
    mut shooters: Query<
        (
            Entity,
            &Transform,
            &Team,
            &Weapon,
            &mut WeaponState,
            &AttackTarget,
            &TurretYaw,
            Option<&WeaponProfile>,
            Option<&UnitKind>,
            Option<&crate::structures::Turret>,
        ),
        (
            Or<(With<Unit>, With<crate::structures::Building>)>,
            With<Weapon>,
        ),
    >,
) {
    let Some(assets) = assets else {
        return;
    };
    for (
        entity,
        transform,
        team,
        weapon,
        mut state,
        target,
        turret,
        profile,
        kind,
        turret_marker,
    ) in &mut shooters
    {
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
        // Tolerance comes from the unit table or the turret table.
        // Top-attack lobs skyward: no traverse gate, only range/cooldown.
        let tech = profile.map(|p| p.tech).unwrap_or(WeaponTech::Homing);
        let tolerance = match (kind, turret_marker) {
            (Some(k), _) => crate::units::archetype(*k).aim_tolerance,
            (None, Some(_)) => {
                crate::economy::balance::turret_stats(crate::economy::balance::BuildingKind::Turret)
                    .map_or(0.12, |s| s.aim_tolerance)
            }
            (None, None) => 0.12,
        };
        if !matches!(tech, WeaponTech::TopAttack) {
            let aim = crate::movement::yaw_toward(target_position - transform.translation);
            if crate::movement::wrap_angle(aim - turret.0).abs() > tolerance {
                continue;
            }
        }
        state.remaining = weapon.cooldown;
        let muzzle_dir = Vec3::new(-turret.0.sin(), 0.0, -turret.0.cos());
        let commander = kind.is_some_and(|kind| kind.is_commander());
        let muzzle = if commander {
            if let Some(messages) = commander_shots.as_deref_mut() {
                messages.write(crate::units::commander_visual::CommanderShot {
                    owner: entity,
                    secondary: false,
                });
            }
            crate::units::commander_visual::muzzle(transform, turret.0, false)
        } else {
            transform.translation + muzzle_dir * crate::units::MUZZLE_REACH
        };
        let team_mat = assets.team_material[team.0 as usize % 2].clone();
        match tech {
            WeaponTech::Laser => {
                // Instant beam: zero-travel shot impacting in
                // `move_projectiles` later this same tick (same DPS as table),
                // plus the light flash now.
                spawn_beam(
                    &mut commands,
                    &assets,
                    muzzle,
                    target_position,
                    0.22,
                    BEAM_TTL_SECS,
                );
                commands.spawn((
                    Projectile {
                        target: target.0,
                        speed: 1.0,
                        damage: weapon.damage,
                        kind: ProjectileKind::Shot {
                            aim: target_position,
                            arc: 0.0,
                            splash: 0.0,
                            t: 0.0,
                            dur: 0.0,
                        },
                    },
                    *team,
                    Transform::from_translation(muzzle),
                    Mesh3d(assets.mesh.clone()),
                    MeshMaterial3d(team_mat.clone()),
                ));
            }
            WeaponTech::TopAttack => {
                // Plunging strike above the locked point: dodgeable while it
                // falls, splash where it lands (not where the target was).
                let splash = profile.map(|p| p.splash).unwrap_or(0.0);
                let top = Vec3::new(
                    target_position.x,
                    target_position.y + PLUNGE_HEIGHT,
                    target_position.z,
                );
                commands.spawn((
                    Projectile {
                        target: target.0,
                        speed: weapon.projectile_speed.max(1.0),
                        damage: weapon.damage,
                        kind: ProjectileKind::Plunge {
                            aim: target_position,
                            splash,
                        },
                    },
                    *team,
                    Transform::from_translation(top).with_scale(Vec3::splat(1.6)),
                    Mesh3d(assets.shell_mesh.clone()),
                    MeshMaterial3d(assets.shell_material.clone()),
                ));
            }
            WeaponTech::Homing => {
                commands.spawn((
                    Projectile {
                        target: target.0,
                        speed: weapon.projectile_speed,
                        damage: weapon.damage,
                        kind: ProjectileKind::Guided,
                    },
                    *team,
                    Transform::from_translation(muzzle),
                    Mesh3d(assets.mesh.clone()),
                    MeshMaterial3d(team_mat.clone()),
                ));
            }
            WeaponTech::Gun | WeaponTech::Dumbfire | WeaponTech::Mortar => {
                // Point-targeted shot: guns bake deterministic spread into the
                // aim (movers dodge, close still targets eat it); mortars arc.
                let dist = transform.translation.distance(target_position);
                let spread = profile.map(|p| p.spread_rad).unwrap_or(0.0);
                let offset = spread_offset(entity.to_bits(), clock.tick, 0, spread, dist);
                let aim = Vec3::new(
                    target_position.x + offset.x,
                    target_position.y,
                    target_position.z + offset.y,
                );
                let splash = profile.map(|p| p.splash).unwrap_or(0.0);
                let arc = if matches!(tech, WeaponTech::Mortar) {
                    MORTAR_ARC_HEIGHT
                } else {
                    0.0
                };
                let dur = (dist / weapon.projectile_speed.max(0.05)).max(0.001);
                let (mesh, material, tint_scale) = match tech {
                    WeaponTech::Mortar => (
                        assets.shell_mesh.clone(),
                        assets.shell_material.clone(),
                        Vec3::splat(1.6),
                    ),
                    WeaponTech::Dumbfire => {
                        (assets.mesh.clone(), team_mat.clone(), Vec3::splat(1.4))
                    }
                    _ => (
                        assets.tracer_mesh.clone(),
                        team_mat.clone(),
                        Vec3::new(1.0, 1.0, dist.max(1.0)),
                    ),
                };
                let mut shot = Transform::from_translation(muzzle);
                if matches!(tech, WeaponTech::Gun) && dist > f32::EPSILON {
                    shot = shot.looking_at(aim, Vec3::Y);
                }
                shot.scale = tint_scale;
                commands.spawn((
                    Projectile {
                        target: target.0,
                        speed: weapon.projectile_speed.max(0.05),
                        damage: weapon.damage,
                        kind: ProjectileKind::Shot {
                            aim,
                            arc,
                            splash,
                            t: 0.0,
                            dur,
                        },
                    },
                    *team,
                    shot,
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                ));
            }
        }
    }
}

/// Second-gun fire for dual-gun units (Commander missiles, Vanguard lasers).
/// Same `AttackTarget` as the primary, own tech/range/cooldown/yaw gate from
/// the secondary spec: the two guns overlap without syncing, tuned per unit.
#[allow(clippy::type_complexity)]
pub(crate) fn fire_secondary(
    mut commands: Commands,
    mut commander_shots: Option<ResMut<Messages<crate::units::commander_visual::CommanderShot>>>,
    grid: Res<SpatialGrid>,
    clock: Res<CombatClock>,
    assets: Option<Res<ProjectileAssets>>,
    health: Query<&Health>,
    mut shooters: Query<
        (
            Entity,
            &Transform,
            &Team,
            &SecondaryWeapon,
            &mut SecondaryWeaponState,
            &AttackTarget,
            &SecondaryTurretYaw,
            Option<&UnitKind>,
        ),
        With<Unit>,
    >,
) {
    let Some(assets) = assets else {
        return;
    };
    for (entity, transform, team, weapon, mut state, target, turret, kind) in &mut shooters {
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
        // Lasers ride straight off the pod: no traverse gate, like TopAttack.
        if !matches!(weapon.tech, WeaponTech::Laser) {
            let aim = crate::movement::yaw_toward(target_position - transform.translation);
            if crate::movement::wrap_angle(aim - turret.0).abs() > weapon.aim_tolerance {
                continue;
            }
        }
        state.remaining = weapon.cooldown;
        let commander = kind.is_some_and(|kind| kind.is_commander());
        if commander && let Some(messages) = commander_shots.as_deref_mut() {
            messages.write(crate::units::commander_visual::CommanderShot {
                owner: entity,
                secondary: true,
            });
        }
        let team_mat = assets.team_material[team.0 as usize % 2].clone();
        match weapon.tech {
            WeaponTech::Laser => {
                let muzzle = transform.translation + Vec3::Y * 1.6;
                spawn_beam(
                    &mut commands,
                    &assets,
                    muzzle,
                    target_position,
                    0.22,
                    BEAM_TTL_SECS,
                );
                commands.spawn((
                    Projectile {
                        target: target.0,
                        speed: 1.0,
                        damage: weapon.damage,
                        kind: ProjectileKind::Shot {
                            aim: target_position,
                            arc: 0.0,
                            splash: 0.0,
                            t: 0.0,
                            dur: 0.0,
                        },
                    },
                    *team,
                    Transform::from_translation(muzzle),
                    Mesh3d(assets.mesh.clone()),
                    MeshMaterial3d(team_mat),
                ));
            }
            // Missiles launch higher off the hull so the two muzzles read apart.
            // Top-attack lobs from the sky above the locked point (dodgeable).
            _ => {
                let muzzle_dir = Vec3::new(-turret.0.sin(), 0.0, -turret.0.cos());
                let muzzle = if commander {
                    crate::units::commander_visual::muzzle(transform, turret.0, true)
                } else {
                    transform.translation + muzzle_dir * 2.2 + Vec3::Y * 1.6
                };
                let dist = transform.translation.distance(target_position);
                let offset =
                    spread_offset(entity.to_bits(), clock.tick, 1, weapon.spread_rad, dist);
                let aim = Vec3::new(
                    target_position.x + offset.x,
                    target_position.y,
                    target_position.z + offset.y,
                );
                let dur = (dist / weapon.projectile_speed.max(0.05)).max(0.001);
                let is_plunge = matches!(weapon.tech, WeaponTech::TopAttack);
                let spawn = if is_plunge {
                    Vec3::new(aim.x, aim.y + PLUNGE_HEIGHT, aim.z)
                } else {
                    muzzle
                };
                commands.spawn((
                    Projectile {
                        target: target.0,
                        speed: weapon.projectile_speed.max(0.05),
                        damage: weapon.damage,
                        kind: match weapon.tech {
                            WeaponTech::Homing => ProjectileKind::Guided,
                            WeaponTech::TopAttack => ProjectileKind::Plunge {
                                aim,
                                splash: weapon.splash,
                            },
                            _ => ProjectileKind::Shot {
                                aim,
                                arc: if matches!(weapon.tech, WeaponTech::Mortar) {
                                    MORTAR_ARC_HEIGHT
                                } else {
                                    0.0
                                },
                                splash: weapon.splash,
                                t: 0.0,
                                dur,
                            },
                        },
                    },
                    *team,
                    Transform::from_translation(spawn),
                    Mesh3d(assets.mesh.clone()),
                    MeshMaterial3d(team_mat),
                ));
            }
        }
    }
}
