//! Combattimento: HP, armi, targeting guidato da ordini, proiettili, morti.
//!
//! Regole BAR-style: `AttackMove` si ferma e riprende, `Move` spara marciando,
//! `Hold`/`Idle` difendono in piedi. Targeting sempre gated su fog (`can_target`).
//! I marker `Chasing`/`HoldFire` sono locomozione, l'intento resta in `orders::UnitOrder`.

//! Proiettili: guided/mortar/plunge/beam, splash, movimento.

use super::acquire::target_position;
use super::death::{Health, apply_damage_to, is_dead};
use super::fire::{ProjectileAssets, spawn_beam};
use crate::{spatial::SpatialGrid, units::Team};
use bevy::prelude::*;

/// Hit radius for projectile impact checks.
pub(crate) const PROJECTILE_HIT_RADIUS: f32 = 0.7;

#[derive(Component, Debug, Clone, Copy)]
pub struct Projectile {
    pub target: Entity,
    pub speed: f32,
    pub damage: f32,
    pub kind: ProjectileKind,
}

/// Projectile behaviour. `Guided` is the legacy steering missile (turrets and
/// homing tech). `Shot` flies straight to a locked aim point: guns bake their
/// deterministic spread into `aim`, mortars add a visual arc, splash shells
/// hurt an area. `Plunge` spawns high above the aim and falls straight down
/// (top-attack: dodgeable while falling, impact where it lands, not where
/// the target was). All fields are plain data: repeats stay bit-identical.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProjectileKind {
    Guided,
    Shot {
        aim: Vec3,
        arc: f32,
        splash: f32,
        t: f32,
        dur: f32,
    },
    Plunge {
        aim: Vec3,
        splash: f32,
    },
}

/// Mortar visual arc height (pure decoration: damage uses the aim point).
/// Top-attack spawn height: fall time at 30 m/s ≈ 1.4 s of dodge window.
pub const MORTAR_ARC_HEIGHT: f32 = 5.0;
pub const PLUNGE_HEIGHT: f32 = 42.0;
/// Laser/impact flash lifetime (visual only, despawned by `tick_beams`).
pub const BEAM_TTL_SECS: f32 = 0.12;

/// Deterministic aim spread: uniform disk of angular radius `spread_rad`
/// scaled by distance (far shots spray, point blank stays true). Pure hash of
/// (shooter, tick, gun): same battle replays bit-identically, no RNG state in
/// the sim. `gun_idx` 0 = primary, 1 = secondary (two guns same tick differ).
pub fn spread_offset(
    shooter_bits: u64,
    tick: u64,
    gun_idx: u64,
    spread_rad: f32,
    dist: f32,
) -> Vec2 {
    if !spread_rad.is_finite() || spread_rad <= 0.0 || !dist.is_finite() || dist <= 0.0 {
        return Vec2::ZERO;
    }
    let mut h = shooter_bits
        .wrapping_mul(0x9E3779B97F4A7C15)
        .wrapping_add(tick.wrapping_mul(0xBF58476D1CE4E5B9))
        .wrapping_add((gun_idx + 1).wrapping_mul(0x94D049BB133111EB));
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58476D1CE4E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D049BB133111EB);
    h ^= h >> 31;
    let u1 = ((h >> 32) as f32) / (u32::MAX as f32);
    let u2 = ((h & 0xFFFF_FFFF) as f32) / (u32::MAX as f32);
    let angle = u1 * std::f32::consts::TAU;
    let radius = spread_rad * dist * u2.sqrt();
    Vec2::new(angle.cos() * radius, angle.sin() * radius)
}

/// Splash victim selection: live enemies inside `radius` of impact, ordered
/// by entity bits (deterministic). Pure helper so the rule is unit-testable
/// without a World; the system feeds it from the spatial grid + queries.
pub fn splash_targets(
    candidates: &[(u64, u8, Vec3, f32)],
    impact: Vec3,
    radius: f32,
    shooter_team: u8,
) -> Vec<u64> {
    if !radius.is_finite() || radius <= 0.0 {
        return Vec::new();
    }
    let mut out: Vec<u64> = candidates
        .iter()
        .filter(|(_, team, _, hp)| *team != shooter_team && *hp > 0.0)
        .filter(|(_, _, pos, _)| pos.distance_squared(impact) <= radius * radius)
        .map(|(bits, _, _, _)| *bits)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Beam/impact flash: visual only, never sim state. Spawned stretched
/// from→to, shrinks and despawns in `tick_beams`. Lasers read as instant
/// light; splash impacts get a short vertical pop.
#[derive(Component, Debug, Clone, Copy)]
pub struct BeamFlash {
    pub ttl: f32,
    pub life: f32,
}

pub fn in_weapon_range(from: Vec3, to: Vec3, range: f32) -> bool {
    from.distance_squared(to) <= range * range
}

/// Impact resolution: splash hurts live enemies in radius (sorted, deterministic),
/// single-target hits the locked target only if still near the impact point
/// (movers dodge). Buildings take splash like units (they carry Team+Health).
/// Pure data flow except the damage writes, which stay in one place.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_impact(
    impact: Vec3,
    splash: f32,
    damage: f32,
    target: Entity,
    shooter_team: Team,
    grid: &SpatialGrid,
    teams: &Query<&Team>,
    health: &mut Query<&mut Health>,
) {
    if splash > 0.0 {
        let mut victims: Vec<(u64, Entity)> = Vec::new();
        grid.for_each_nearby(impact, splash, |entry| {
            victims.push((entry.entity.to_bits(), entry.entity));
        });
        victims.sort_unstable();
        victims.dedup();
        for (_, victim) in victims {
            let enemy = teams.get(victim).is_ok_and(|t| t.is_enemy(shooter_team));
            let alive = health.get(victim).is_ok_and(|h| !is_dead(h));
            if enemy
                && alive
                && let Ok(mut h) = health.get_mut(victim)
            {
                h.current = apply_damage_to(h.current, damage).clamp(0.0, h.max);
            }
        }
    } else if let Ok(target_health) = health.get(target) {
        let enemy = teams.get(target).is_ok_and(|t| t.is_enemy(shooter_team));
        let near = grid.position(target).is_some_and(|p| {
            p.distance_squared(impact) <= PROJECTILE_HIT_RADIUS * PROJECTILE_HIT_RADIUS
        });
        if enemy
            && !is_dead(target_health)
            && near
            && let Ok(mut h) = health.get_mut(target)
        {
            h.current = apply_damage_to(h.current, damage).clamp(0.0, h.max);
        }
    }
}

pub(crate) fn move_projectiles(
    mut commands: Commands,
    time: Res<Time>,
    grid: Res<SpatialGrid>,
    assets: Option<Res<ProjectileAssets>>,
    teams: Query<&Team>,
    mut health: Query<&mut Health>,
    mut projectiles: Query<(Entity, &mut Transform, &mut Projectile, &Team)>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, mut projectile, team) in &mut projectiles {
        let kind = projectile.kind;
        match kind {
            ProjectileKind::Guided => {
                // Legacy steering missile: tracks the locked entity, dies
                // quietly with it. Turrets and homing tech live here.
                let Some(target_position) = target_position(&grid, projectile.target) else {
                    commands.entity(entity).despawn();
                    continue;
                };
                let offset = target_position - transform.translation;
                let distance = offset.length();
                let step = projectile.speed * dt;
                if distance <= step.max(PROJECTILE_HIT_RADIUS) {
                    if let Ok(mut target_health) = health.get_mut(projectile.target) {
                        target_health.current =
                            apply_damage_to(target_health.current, projectile.damage)
                                .clamp(0.0, target_health.max);
                    }
                    commands.entity(entity).despawn();
                } else if distance > f32::EPSILON {
                    transform.translation += offset / distance * step;
                }
            }
            ProjectileKind::Shot {
                aim,
                arc,
                splash,
                t,
                dur,
            } => {
                // Straight flight to the locked aim point (guns, dumbfire,
                // lasers, mortars with a visual parabola). Damage on arrival:
                // splash hurts the area, single-target needs the lock still
                // near the impact (spread misses and movers live here).
                let base = transform.translation;
                let to_aim = aim - base;
                // Arrival check on flat distance (arc lifts y, never target).
                let flat = Vec3::new(to_aim.x, 0.0, to_aim.z).length();
                let step = projectile.speed * dt;
                if flat <= step.max(PROJECTILE_HIT_RADIUS) || t >= dur {
                    let impact = Vec3::new(aim.x, aim.y, aim.z);
                    let (damage, target, shooter) = (projectile.damage, projectile.target, *team);
                    resolve_impact(
                        impact,
                        splash,
                        damage,
                        target,
                        shooter,
                        &grid,
                        &teams,
                        &mut health,
                    );
                    if splash > 0.0
                        && let Some(assets) = assets.as_deref()
                    {
                        spawn_beam(
                            &mut commands,
                            assets,
                            impact,
                            impact + Vec3::Y * 3.0,
                            0.9,
                            BEAM_TTL_SECS,
                        );
                    }
                    commands.entity(entity).despawn();
                } else {
                    let dir = to_aim / to_aim.length().max(f32::EPSILON);
                    let nt = (t + dt).min(dur);
                    let frac = if dur > 0.0 { nt / dur } else { 1.0 };
                    let mut next = base + dir * step.min(flat);
                    if arc > 0.0 {
                        // Parabola over the whole flight (visual only).
                        next.y = base.y + (aim.y - base.y) * frac + arc * 4.0 * frac * (1.0 - frac);
                    }
                    transform.translation = next;
                    projectile.kind = ProjectileKind::Shot {
                        aim,
                        arc,
                        splash,
                        t: nt,
                        dur,
                    };
                }
            }
            ProjectileKind::Plunge { aim, splash } => {
                // Top-attack: straight down above the locked point. Lands
                // where it lands (dodgeable): splash at current xz on touchdown.
                let step = projectile.speed * dt;
                let ground_y = aim.y;
                if transform.translation.y - step <= ground_y {
                    let impact =
                        Vec3::new(transform.translation.x, ground_y, transform.translation.z);
                    let (damage, target, shooter) = (projectile.damage, projectile.target, *team);
                    resolve_impact(
                        impact,
                        splash.max(PROJECTILE_HIT_RADIUS),
                        damage,
                        target,
                        shooter,
                        &grid,
                        &teams,
                        &mut health,
                    );
                    if let Some(assets) = assets.as_deref() {
                        spawn_beam(
                            &mut commands,
                            assets,
                            impact,
                            impact + Vec3::Y * 6.0,
                            1.4,
                            BEAM_TTL_SECS * 2.0,
                        );
                    }
                    commands.entity(entity).despawn();
                } else {
                    transform.translation.y -= step;
                }
            }
        }
    }
}

/// Beam/impact flashes shrink away on a fixed visual lifetime. Pure
/// presentation: sim state never reads them back.
pub(crate) fn tick_beams(
    mut commands: Commands,
    time: Res<Time>,
    mut flashes: Query<(Entity, &mut BeamFlash, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (entity, mut flash, mut transform) in &mut flashes {
        flash.ttl -= dt;
        if flash.ttl <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let k = (flash.ttl / flash.life).clamp(0.0, 1.0);
        transform.scale.x *= k.max(0.2);
        transform.scale.y *= k.max(0.2);
    }
}
