//! Combattimento: HP, armi, targeting guidato da ordini, proiettili, morti.
//!
//! Regole BAR-style: `AttackMove` si ferma e riprende, `Move` spara marciando,
//! `Hold`/`Idle` difendono in piedi. Targeting sempre gated su fog (`can_target`).
//! I marker `Chasing`/`HoldFire` sono locomozione, l'intento resta in `orders::UnitOrder`.

//! Split per dominio (re-export invariato):
//! - `acquire` — scan, lock, behaviour, chase.
//! - `fire` — armi, cooldown, traverse.
//! - `projectile` — guided/mortar/beam, splash.
//! - `death` — HP, danno, cleanup.
//! - `guard` — scorta ward.
//!
//! Vedi `ARCHITECTURE.md`.

pub mod acquire;
pub mod death;
pub mod fire;
pub mod guard;
pub mod projectile;

pub use acquire::{
    ACQUIRE_STRIDE, AcquisitionRange, AttackTarget, CHASE_REPLAN_DISTANCE, Chasing, CombatClock,
    HoldFire, should_retarget,
};
pub use death::{Health, apply_damage_to, is_dead};
pub use fire::{
    SecondaryTurretYaw, SecondaryWeapon, SecondaryWeaponState, TurretYaw, Weapon, WeaponProfile,
    WeaponState, desync_phase,
};
pub use guard::GUARD_RADIUS;
pub use projectile::{
    BEAM_TTL_SECS, BeamFlash, MORTAR_ARC_HEIGHT, PLUNGE_HEIGHT, Projectile, ProjectileKind,
    in_weapon_range, splash_targets, spread_offset,
};

use self::acquire::{acquire_targets, chase_targets, resolve_behaviour, validate_targets};
use self::death::process_deaths;
use self::fire::{
    fire_secondary, fire_weapons, setup_projectile_assets, tick_cooldowns, traverse_secondary,
    traverse_turrets,
};
use self::guard::validate_guards;
use self::projectile::{move_projectiles, tick_beams};
use crate::movement::MovementSystems;
use bevy::prelude::*;

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
                    (validate_guards, validate_targets)
                        .chain()
                        .in_set(ValidateTargets),
                    acquire_targets.in_set(AcquireTargets),
                    resolve_behaviour.in_set(ResolveBehaviour),
                    chase_targets.in_set(ChaseTargets),
                    (
                        traverse_turrets,
                        traverse_secondary,
                        tick_cooldowns,
                        fire_weapons,
                        fire_secondary,
                    )
                        .chain()
                        .in_set(WeaponSystems),
                    (move_projectiles, tick_beams)
                        .chain()
                        .in_set(ProjectileSystems),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        movement::{MoveTarget, MovementPlugin},
        navigation::{CELL_SIZE, HALF_SIZE, NavGrid, NavigationPlugin},
        orders::{UnitOrder, UnitOrderQueue, allows_auto_targeting},
        scenario::Scenario,
        spatial::{SpatialGrid, SpatialPlugin},
        units::{CollisionRadius, Team, Unit, UnitKind, UnitPlugin},
    };
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    /// Headless combat harness on a hermetic open field: gameplay logic
    /// must not depend on the generated map layout.
    fn combat_app() -> App {
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
            ))
            .insert_resource(NavGrid::new(HALF_SIZE, CELL_SIZE, Vec::new()));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    #[allow(clippy::type_complexity)]
    fn combatant(
        id: u32,
        team: u8,
        position: Vec3,
        order: UnitOrder,
        kind: UnitKind,
    ) -> (
        Unit,
        Team,
        crate::movement::Movement,
        UnitOrder,
        MoveTarget,
        Transform,
        UnitKind,
        CollisionRadius,
        Health,
        Weapon,
        WeaponProfile,
        WeaponState,
        AcquisitionRange,
        TurretYaw,
    ) {
        let destination = match order {
            UnitOrder::Move { destination } | UnitOrder::AttackMove { destination } => destination,
            _ => position,
        };
        let stats = crate::units::archetype(kind);
        let (kind_component, radius, health, weapon, profile, weapon_state, acquisition, turret) =
            crate::units::arm_bundle(id, kind);
        (
            Unit(id),
            Team(team),
            crate::movement::Movement { speed: stats.speed },
            order,
            MoveTarget(destination),
            Transform::from_translation(position),
            kind_component,
            radius,
            health,
            weapon,
            profile,
            weapon_state,
            acquisition,
            turret,
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
        assert!(!is_dead(&Health {
            current: 100.0,
            max: 100.0
        }));
    }

    #[test]
    fn weapon_range_is_inclusive_and_uses_full_distance() {
        let from = Vec3::ZERO;
        assert!(in_weapon_range(from, Vec3::new(18.0, 0.0, 0.0), 18.0));
        assert!(!in_weapon_range(from, Vec3::new(18.1, 0.0, 0.0), 18.0));
    }

    /// Fog harness: same combat scene plus the visibility plugin, so locks
    /// need team sight. Plain combat_app (no fog data) stays open.
    fn combat_fog_app() -> App {
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
                crate::fog::FogPlugin { render: false },
            ))
            .insert_resource(NavGrid::new(HALF_SIZE, CELL_SIZE, Vec::new()));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    #[test]
    fn fog_blocks_locks_until_spotted_then_drops_them() {
        // Tank sights (15m) are shorter than guns (18m): holders 25m apart
        // are mutually blind and must not engage.
        let mut app = combat_fog_app();
        let blue = app
            .world_mut()
            .spawn(combatant(
                600,
                0,
                Vec3::new(-30.0, 0.8, 0.0),
                UnitOrder::HoldPosition,
                UnitKind::HeavyTank,
            ))
            .id();
        let red = app
            .world_mut()
            .spawn(combatant(
                601,
                1,
                Vec3::new(-5.0, 0.8, 0.0),
                UnitOrder::HoldPosition,
                UnitKind::HeavyTank,
            ))
            .id();
        // Fog rasters immediately, then the test advances through 1s blind.
        for _ in 0..20 {
            app.update();
        }
        for _ in 0..60 {
            app.update();
        }
        assert!(app.world().get::<AttackTarget>(blue).is_none());
        assert!(app.world().get::<AttackTarget>(red).is_none());
        // Walked into sight: both lock.
        app.world_mut()
            .get_mut::<Transform>(red)
            .unwrap()
            .translation = Vec3::new(-20.0, 0.8, 0.0);
        for _ in 0..60 {
            app.update();
        }
        assert!(app.world().get::<AttackTarget>(blue).is_some());
        assert!(app.world().get::<AttackTarget>(red).is_some());
        // Gone far beyond sight and leash: locks drop.
        app.world_mut()
            .get_mut::<Transform>(red)
            .unwrap()
            .translation = Vec3::new(70.0, 0.8, 0.0);
        for _ in 0..60 {
            app.update();
        }
        assert!(app.world().get::<AttackTarget>(blue).is_none());
        assert!(app.world().get::<AttackTarget>(red).is_none());
    }

    #[test]
    fn spotter_shares_vision_for_long_guns() {
        // Blue heavy at 0, red at 20: beyond the heavy's own sight (15) but
        // inside its gun (19) and acquisition (30). A forward engineer at 12
        // (sight 22) spots red for the team, so the heavy locks a target its
        // own eyes cannot see — without firing out of range.
        let mut app = combat_fog_app();
        let gun = app
            .world_mut()
            .spawn(combatant(
                610,
                0,
                Vec3::new(0.0, 0.8, 0.0),
                UnitOrder::HoldPosition,
                UnitKind::HeavyTank,
            ))
            .id();
        let red = app
            .world_mut()
            .spawn(combatant(
                611,
                1,
                Vec3::new(20.0, 0.8, 0.0),
                UnitOrder::HoldPosition,
                UnitKind::HeavyTank,
            ))
            .id();
        let _spotter = crate::units::spawn_combat_unit(
            &mut app.world_mut().commands(),
            612,
            Team(0),
            UnitKind::Engineer,
            Vec3::new(12.0, 0.0, 0.0),
        );
        app.world_mut().flush();
        for _ in 0..80 {
            app.update();
        }
        assert!(app.world().get::<AttackTarget>(gun).is_some());
        // Lock holds via team vision (acquisition 30) but 20m is outside the
        // 19m gun: red stays healthy until the gun closes in.
        let hp = app.world().get::<Health>(red).unwrap().current;
        let full = crate::units::archetype(UnitKind::HeavyTank).max_health;
        assert!((hp - full).abs() < 0.001, "spotted but out of range: {hp}");
    }

    #[test]
    fn turret_holds_and_fires_without_chasing() {
        use crate::{economy::balance::BuildingKind, structures::spawn_building};
        let mut app = combat_app();
        let home = Vec3::ZERO;
        let turret = spawn_building(
            &mut app.world_mut().commands(),
            Team(0),
            BuildingKind::Turret,
            home,
            true,
        );
        let enemy = app
            .world_mut()
            .spawn(combatant(
                700,
                1,
                Vec3::new(15.0, 0.8, 0.0),
                UnitOrder::HoldPosition,
                UnitKind::HeavyTank,
            ))
            .id();
        app.world_mut().flush();
        let full = crate::units::archetype(UnitKind::HeavyTank).max_health;
        for _ in 0..120 {
            app.update();
        }
        // Locked and damaged the intruder (turret 12dmg/0.8s vs 170 HP).
        assert!(app.world().get::<AttackTarget>(turret).is_some());
        assert!(app.world().get::<Health>(enemy).unwrap().current < full);
        // ...without ever moving or chasing: no MoveTarget, no Chasing.
        let at = app.world().get::<Transform>(turret).unwrap().translation;
        assert!((at - home.with_y(at.y)).length() < 0.01);
        assert!(app.world().get::<Chasing>(turret).is_none());
        assert!(app.world().get::<MoveTarget>(turret).is_none());
    }

    #[test]
    fn explicit_attack_on_unseen_target_completes() {
        let mut app = combat_fog_app();
        let blue = app
            .world_mut()
            .spawn(combatant(
                620,
                0,
                Vec3::new(0.0, 0.8, 0.0),
                UnitOrder::HoldPosition,
                UnitKind::HeavyTank,
            ))
            .id();
        let red = app
            .world_mut()
            .spawn(combatant(
                621,
                1,
                Vec3::new(100.0, 0.8, 0.0),
                UnitOrder::Idle,
                UnitKind::HeavyTank,
            ))
            .id();
        crate::orders::queue_attack(&mut app.world_mut().commands().entity(blue), red);
        app.world_mut().flush();
        for _ in 0..40 {
            app.update();
        }
        assert!(app.world().get::<AttackTarget>(blue).is_none());
        assert!(matches!(
            app.world().get::<UnitOrder>(blue),
            Some(UnitOrder::Idle)
        ));
    }

    #[test]
    fn commander_fires_missiles_beyond_mitra_range() {
        use crate::units::{Commander, secondary_bundle};
        let mut app = combat_app();
        // Commander vs tank a 30m: fuori mitra (20), dentro missili (34) e
        // dentro acquisition (36). Solo i missili devono partire.
        let commander = app
            .world_mut()
            .spawn(combatant(
                500,
                0,
                Vec3::new(0.0, 1.6, 0.0),
                UnitOrder::HoldPosition,
                UnitKind::Commander,
            ))
            .id();
        let target = app
            .world_mut()
            .spawn(combatant(
                501,
                1,
                Vec3::new(30.0, 0.8, 0.0),
                UnitOrder::Idle,
                UnitKind::HeavyTank,
            ))
            .id();
        app.world_mut()
            .entity_mut(commander)
            .insert(secondary_bundle(500, UnitKind::Commander).unwrap())
            .insert((
                Commander,
                crate::units::builder_bundle(UnitKind::Commander).unwrap(),
            ))
            .insert(AttackTarget(target));
        // Cooldown azzerati + turret allineate: isola il gating di range.
        // (Il bundle desynca il primo colpo fino a 2.5s, il test gira 0.5s.)
        app.world_mut()
            .entity_mut(commander)
            .insert(SecondaryWeaponState { remaining: 0.0 })
            .insert(WeaponState { remaining: 99.0 });
        // Aim yaw verso +X: yaw_toward((30,0,0)) ≈ -PI/2? Allinea entrambe.
        app.world_mut()
            .entity_mut(commander)
            .insert(TurretYaw(0.0))
            .insert(SecondaryTurretYaw(0.0));
        // Aim yaw verso +X: yaw_toward((30,0,0)) ≈ -PI/2? Allinea entrambe.
        let aim = crate::movement::yaw_toward(Vec3::X * 30.0);
        app.world_mut()
            .entity_mut(commander)
            .insert(TurretYaw(aim))
            .insert(SecondaryTurretYaw(aim));
        for _ in 0..30 {
            app.update();
        }
        let missiles: Vec<_> = app
            .world_mut()
            .query::<&Projectile>()
            .iter(app.world())
            .map(|p| (p.damage, p.speed))
            .collect();
        // Solo proiettili da 40 danni (missili), mai da 7 (mitra).
        assert!(!missiles.is_empty());
        assert!(missiles.iter().all(|(d, _)| (*d - 40.0).abs() < 0.001));
    }

    #[test]
    fn builder_assists_builder_by_guarding_and_closing_in() {
        use crate::orders::queue_guard;
        // Assist = Guard su builder alleato (right-click): il follower sta
        // nel raggio del sito quando il ward costruisce, e site_power somma
        // entrambi. Qui si prova la marcia di avvicinamento.
        let mut app = combat_app();
        let ward = crate::units::spawn_combat_unit(
            &mut app.world_mut().commands(),
            600,
            Team(0),
            UnitKind::Engineer,
            Vec3::ZERO,
        );
        let follower = crate::units::spawn_combat_unit(
            &mut app.world_mut().commands(),
            601,
            Team(0),
            UnitKind::Engineer,
            Vec3::X * 20.0,
        );
        app.world_mut().flush();
        // Disarmati ma costruibili: niente Weapon, con Builder.
        for e in [ward, follower] {
            assert!(app.world().get::<Weapon>(e).is_none());
            assert!(app.world().get::<crate::units::Builder>(e).is_some());
        }
        queue_guard(&mut app.world_mut().commands().entity(follower), ward);
        app.world_mut().flush();
        for _ in 0..600 {
            app.update();
        }
        let w = app.world().get::<Transform>(ward).unwrap().translation;
        let f = app.world().get::<Transform>(follower).unwrap().translation;
        assert!(
            w.distance(f) <= super::GUARD_RADIUS + 1.0,
            "follower must close in on the ward, dist={}",
            w.distance(f)
        );
        assert!(matches!(
            app.world().get::<UnitOrder>(follower),
            Some(UnitOrder::Guard { .. })
        ));
    }

    #[test]
    fn attack_move_squads_meet_fight_and_move_fires_on_the_march() {
        let mut app = combat_app();

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
                UnitKind::HeavyTank,
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
                UnitKind::HeavyTank,
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
                UnitKind::HeavyTank,
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
                // The marching unit acquires too, but keeps marching: it
                // fires on the move instead of stopping or chasing.
                assert!(world.entity(mover).get::<AttackTarget>().is_some());
                assert!(matches!(
                    world.entity(mover).get::<UnitOrder>(),
                    Some(UnitOrder::Move { .. })
                ));
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
        // The mover kept marching (or arrived) and never held a stale state.
        let mover_order = world.entity(mover).get::<UnitOrder>().unwrap();
        assert!(matches!(
            mover_order,
            UnitOrder::Move { .. } | UnitOrder::Idle
        ));
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
        let mut app = combat_app();

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
                UnitKind::HeavyTank,
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
        // Heavies (170 HP) take longer to resolve than the old 120 HP tanks:
        // 1200 ticks keeps the same "packed melee resolves" bar.
        let mut kills_seen = 0;
        let mut max_projectiles = 0;
        for tick in 0..1200 {
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
    fn attack_move_stops_to_fight_while_move_fires_on_the_march() {
        let mut app = combat_app();

        // Unarmed stand-ins: valid targets that never fight back, so the
        // measured behaviour belongs to the blue orders under test.
        let attacker = app
            .world_mut()
            .spawn(combatant(
                120,
                0,
                Vec3::new(-30.0, 0.8, -40.0),
                UnitOrder::AttackMove {
                    destination: Vec3::new(60.0, 0.8, -40.0),
                },
                UnitKind::HeavyTank,
            ))
            .id();
        let victim = app
            .world_mut()
            .spawn(combatant(
                121,
                1,
                Vec3::new(-10.0, 0.8, -40.0),
                UnitOrder::Idle,
                UnitKind::HeavyTank,
            ))
            .id();
        let marcher = app
            .world_mut()
            .spawn(combatant(
                122,
                0,
                Vec3::new(-30.0, 0.8, 40.0),
                UnitOrder::Move {
                    destination: Vec3::new(60.0, 0.8, 40.0),
                },
                UnitKind::HeavyTank,
            ))
            .id();
        let bystander = app
            .world_mut()
            .spawn(combatant(
                123,
                1,
                Vec3::new(30.0, 0.8, 30.0),
                UnitOrder::Idle,
                UnitKind::HeavyTank,
            ))
            .id();
        for target in [victim, bystander] {
            app.world_mut()
                .entity_mut(target)
                .remove::<(Weapon, WeaponState)>();
        }

        // Displacement of the attacker while its victim lives: stop-to-fight
        // must hold it near its start while the marcher crosses the map.
        // The marcher's detour around the north wall needs ~1100 ticks.
        // Lanes sit 80 units apart so the pairs never interact.
        let mut held_disp = 0.0_f32;
        let mut march_yaw = 0.0_f32;
        for tick in 0..1100 {
            app.update();
            let world = app.world();
            if world.entities().contains(victim) {
                held_disp = held_disp.max(
                    (world
                        .entity(attacker)
                        .get::<Transform>()
                        .unwrap()
                        .translation
                        .x
                        + 30.0)
                        .abs(),
                );
            }
            // Mid-march the route runs due east: body must face travel.
            if tick == 500 {
                march_yaw = world
                    .entity(marcher)
                    .get::<Transform>()
                    .unwrap()
                    .rotation
                    .to_euler(EulerRot::YXZ)
                    .0;
            }
        }
        let world = app.world();
        assert!(!world.entities().contains(victim), "attacker must kill");
        assert!(
            held_disp < 12.0,
            "attack-move must stop to fight, drifted {held_disp}"
        );
        // Destination far from reached: the order (not just the target)
        // survives the engagement.
        assert!(matches!(
            world.entity(attacker).get::<UnitOrder>(),
            Some(UnitOrder::AttackMove { .. })
        ));
        // The marcher arrives while wounding the bystander in passing.
        let marched = world
            .entity(marcher)
            .get::<Transform>()
            .unwrap()
            .translation;
        assert!(
            marched.distance(Vec3::new(60.0, 0.8, 40.0)) < 1.0,
            "mover must arrive, at {marched:?}"
        );
        assert!(matches!(
            world.entity(marcher).get::<UnitOrder>(),
            Some(UnitOrder::Move { .. }) | Some(UnitOrder::Idle)
        ));
        // Marching body faces travel direction (due east mid-route).
        assert!(
            -march_yaw.sin() > 0.9,
            "body must face east mid-march, yaw={march_yaw}"
        );
        let bystander_hp = world.entity(bystander).get::<Health>().unwrap().current;
        assert!(
            bystander_hp < 100.0,
            "mover must fire on the march, bystander at {bystander_hp}"
        );
        assert!(world.entities().contains(bystander));
    }

    #[test]
    fn queued_orders_pop_on_completion() {
        let mut app = combat_app();
        let unit = app
            .world_mut()
            .spawn(combatant(
                130,
                0,
                Vec3::new(-30.0, 0.8, 60.0),
                UnitOrder::Move {
                    destination: Vec3::new(30.0, 0.8, 60.0),
                },
                UnitKind::HeavyTank,
            ))
            .id();
        app.world_mut()
            .entity_mut(unit)
            .insert(UnitOrderQueue(vec![UnitOrder::HoldPosition]));
        for _ in 0..900 {
            app.update();
        }
        let world = app.world();
        // Arrived AND popped: Hold is live (never plain Idle), the queue
        // drained, the body sits at the march destination.
        assert_eq!(
            world.entity(unit).get::<UnitOrder>(),
            Some(&UnitOrder::HoldPosition)
        );
        assert!(
            world
                .entity(unit)
                .get::<UnitOrderQueue>()
                .map(|queue| queue.0.is_empty())
                .unwrap_or(true)
        );
        let position = world.entity(unit).get::<Transform>().unwrap().translation;
        assert!(position.distance(Vec3::new(30.0, 0.8, 60.0)) < 1.0);
    }

    #[test]
    fn patrol_advances_waypoints_and_loops_forever() {
        let mut app = combat_app();
        let a = Vec3::new(-30.0, 0.8, 60.0);
        let b = Vec3::new(30.0, 0.8, 60.0);
        let unit = app
            .world_mut()
            .spawn(combatant(
                140,
                0,
                a,
                UnitOrder::Patrol {
                    points: vec![a, b],
                    next: 0,
                },
                UnitKind::HeavyTank,
            ))
            .id();
        for _ in 0..200 {
            app.update();
        }
        // Spawned on waypoint A: advanced to leg B without completing.
        assert!(matches!(
            app.world().entity(unit).get::<UnitOrder>(),
            Some(UnitOrder::Patrol { next: 1, .. })
        ));
        for _ in 0..700 {
            app.update();
        }
        // Reached B and looped back to leg A: patrols never complete.
        let world = app.world();
        assert!(matches!(
            world.entity(unit).get::<UnitOrder>(),
            Some(UnitOrder::Patrol { next: 0, .. })
        ));
        assert!(world.entities().contains(unit));
    }

    #[test]
    fn guard_follows_ward_and_completes_when_ward_dies() {
        let mut app = combat_app();
        let ward = app
            .world_mut()
            .spawn(combatant(
                150,
                0,
                Vec3::new(0.0, 0.8, 60.0),
                UnitOrder::Idle,
                UnitKind::HeavyTank,
            ))
            .id();
        let guard = app
            .world_mut()
            .spawn(combatant(
                151,
                0,
                Vec3::new(-30.0, 0.8, 60.0),
                UnitOrder::Guard { target: ward },
                UnitKind::HeavyTank,
            ))
            .id();
        for _ in 0..900 {
            app.update();
        }
        // Closed the 30-unit gap and holds near the ward, order intact.
        let world = app.world();
        let guard_pos = world.entity(guard).get::<Transform>().unwrap().translation;
        let ward_pos = world.entity(ward).get::<Transform>().unwrap().translation;
        assert!(
            guard_pos.distance(ward_pos) <= GUARD_RADIUS + 2.0,
            "guard must close in, at {guard_pos:?} ward at {ward_pos:?}"
        );
        assert!(matches!(
            world.entity(guard).get::<UnitOrder>(),
            Some(UnitOrder::Guard { .. })
        ));
        // Ward destroyed: guard completes to Idle instead of following a ghost.
        app.world_mut()
            .entity_mut(ward)
            .get_mut::<Health>()
            .unwrap()
            .current = 0.0;
        for _ in 0..10 {
            app.update();
        }
        let world = app.world();
        assert_eq!(
            world.entity(guard).get::<UnitOrder>(),
            Some(&UnitOrder::Idle)
        );
    }

    #[test]
    fn guard_leash_is_ward_centered_with_acquisition_and_retarget_hysteresis() {
        // Isolate targeting from damage/motion so every boundary is exact.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<CombatClock>()
            .insert_resource(SpatialGrid::new(10.0))
            .add_systems(
                Update,
                (
                    validate_guards,
                    validate_targets,
                    acquire_targets,
                    resolve_behaviour,
                )
                    .chain(),
            );
        let ward = app
            .world_mut()
            .spawn(combatant(
                170,
                0,
                Vec3::ZERO,
                UnitOrder::HoldPosition,
                UnitKind::HeavyTank,
            ))
            .id();
        let guard = app
            .world_mut()
            .spawn(combatant(
                171,
                0,
                Vec3::X * 20.0,
                UnitOrder::Guard { target: ward },
                UnitKind::HeavyTank,
            ))
            .id();
        let threat = app
            .world_mut()
            .spawn(combatant(
                172,
                1,
                Vec3::X * 30.0,
                UnitOrder::HoldPosition,
                UnitKind::HeavyTank,
            ))
            .id();
        let decoy = app
            .world_mut()
            .spawn(combatant(
                173,
                1,
                Vec3::X * 100.0,
                UnitOrder::HoldPosition,
                UnitKind::HeavyTank,
            ))
            .id();
        app.world_mut()
            .entity_mut(guard)
            .insert(AcquisitionRange(30.0));
        for entity in [ward, threat, decoy] {
            app.world_mut().entity_mut(entity).remove::<Weapon>();
        }
        let step = |app: &mut App| {
            let entries: Vec<_> = app
                .world_mut()
                .query::<(Entity, &Transform)>()
                .iter(app.world())
                .map(|(entity, transform)| (entity, transform.translation))
                .collect();
            let mut grid = app.world_mut().resource_mut::<SpatialGrid>();
            grid.clear();
            for (entity, position) in entries {
                grid.insert(entity, position, 0.5);
            }
            for _ in 0..ACQUIRE_STRIDE + 1 {
                app.update();
            }
        };
        let place = |app: &mut App, entity: Entity, x: f32| {
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<Transform>()
                .unwrap()
                .translation = Vec3::X * x;
        };
        let locked = |app: &App| {
            app.world()
                .entity(guard)
                .get::<AttackTarget>()
                .map(|lock| lock.0)
        };
        step(&mut app);
        assert_eq!(locked(&app), Some(threat)); // acquisition boundary inclusive

        place(&mut app, threat, 45.0);
        place(&mut app, guard, 35.0);
        place(&mut app, decoy, 34.0); // closer/shootable but outside ward acquisition
        step(&mut app);
        assert_eq!(locked(&app), Some(threat)); // retain at release boundary; no retarget

        place(&mut app, threat, 45.1);
        step(&mut app);
        assert_eq!(locked(&app), None); // still only 10.1 from guard
        assert!(!app.world().entity(guard).contains::<Chasing>());
        assert!(!app.world().entity(guard).contains::<HoldFire>());
        assert_eq!(
            app.world().entity(guard).get::<MoveTarget>().unwrap().0,
            Vec3::ZERO
        );
        assert!(matches!(
            app.world().entity(guard).get::<UnitOrder>(),
            Some(UnitOrder::Guard { .. })
        ));

        for x in [44.9, 45.1, 30.1] {
            place(&mut app, threat, x);
            step(&mut app);
            assert_eq!(locked(&app), None); // no reacquisition flicker in the band
        }
        place(&mut app, threat, 30.0);
        step(&mut app);
        assert_eq!(locked(&app), Some(threat));

        // Moving the ward, with guard/enemy unchanged, also breaks the leash.
        place(&mut app, ward, -16.0);
        step(&mut app);
        assert_eq!(locked(&app), None);
        assert_eq!(
            app.world().entity(guard).get::<MoveTarget>().unwrap().0,
            Vec3::X * -16.0
        );

        // A closer excluded enemy must not hide an eligible second candidate.
        place(&mut app, ward, 0.0);
        place(&mut app, decoy, 34.0);
        step(&mut app);
        assert_eq!(locked(&app), Some(threat));
        // An eligible closer threat can still win a retarget.
        place(&mut app, threat, 25.0);
        place(&mut app, decoy, 29.5);
        step(&mut app);
        assert_eq!(locked(&app), Some(decoy));
        // Ordinary guard-to-enemy leash remains enforced too.
        place(&mut app, guard, -20.0);
        step(&mut app);
        assert_eq!(locked(&app), None);
    }

    #[test]
    fn attack_chase_syncs_move_target_to_moving_quarry() {
        let mut app = combat_app();
        let quarry = app
            .world_mut()
            .spawn(combatant(
                160,
                1,
                Vec3::new(0.0, 0.8, -20.0),
                UnitOrder::Idle,
                UnitKind::HeavyTank,
            ))
            .id();
        let hunter = app
            .world_mut()
            .spawn(combatant(
                161,
                0,
                Vec3::new(-25.0, 0.8, -20.0),
                UnitOrder::Attack { target: quarry },
                UnitKind::HeavyTank,
            ))
            .id();
        for _ in 0..30 {
            app.update();
        }
        // Hunter acquired the quarry and synced a MoveTarget near it.
        let world = app.world();
        assert!(world.entity(hunter).get::<AttackTarget>().is_some());
        let synced = world.entity(hunter).get::<MoveTarget>().unwrap().0;
        let quarry_pos = world.entity(quarry).get::<Transform>().unwrap().translation;
        assert!(
            synced.distance(quarry_pos) <= CHASE_REPLAN_DISTANCE + 0.01,
            "chase must track quarry, synced at {synced:?} quarry at {quarry_pos:?}"
        );
    }

    #[test]
    fn holders_fire_without_chasing_and_ignore_out_of_reach_targets() {
        let mut app = combat_app();

        let post = Vec3::new(-20.0, 0.8, -40.0);
        let holder = app
            .world_mut()
            .spawn(combatant(
                110,
                0,
                post,
                UnitOrder::HoldPosition,
                UnitKind::HeavyTank,
            ))
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
                UnitKind::HeavyTank,
            ))
            .id();
        app.world_mut()
            .entity_mut(passer)
            .remove::<(Weapon, WeaponState)>();
        // Idles at 25 (inside acquisition, outside holder reach): untouchable.
        let bystander_spot = Vec3::new(-20.0, 0.8, -15.0);
        let bystander = app
            .world_mut()
            .spawn(combatant(
                112,
                1,
                bystander_spot,
                UnitOrder::Idle,
                UnitKind::HeavyTank,
            ))
            .id();

        // Heavies bring 170 HP (was 120) at ~11.5 dps: the unarmed passer
        // needs a longer exposure to die without any chase.
        for _ in 0..1200 {
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
            crate::units::archetype(UnitKind::HeavyTank).max_health
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

    #[test]
    fn spread_is_deterministic_bounded_and_varied() {
        // Same inputs, same offset (repeats bit-identical); bounded by the
        // angular cone; varied across ticks (no two volleys alike).
        let a = spread_offset(12345, 100, 0, 0.05, 15.0);
        let b = spread_offset(12345, 100, 0, 0.05, 15.0);
        assert_eq!(a, b);
        assert!(a.length() <= 0.05 * 15.0 + 1e-4);
        assert_eq!(spread_offset(1, 1, 0, 0.0, 15.0), Vec2::ZERO);
        assert_eq!(spread_offset(1, 1, 0, 0.05, 0.0), Vec2::ZERO);
        assert_eq!(spread_offset(1, 1, 0, 0.05, -4.0), Vec2::ZERO);
        let mut seen = std::collections::BTreeSet::new();
        for tick in 0..200u64 {
            let o = spread_offset(777, tick, 0, 0.05, 15.0);
            assert!(o.length() <= 0.75 + 1e-4);
            seen.insert((o.x.to_bits(), o.y.to_bits()));
        }
        assert!(seen.len() > 150, "spread must vary, got {}", seen.len());
        // Primary/secondary same tick differ.
        assert_ne!(
            spread_offset(777, 42, 0, 0.05, 15.0),
            spread_offset(777, 42, 1, 0.05, 15.0)
        );
    }

    #[test]
    fn splash_hits_live_enemies_only_sorted() {
        let origin = Vec3::ZERO;
        let near = Vec3::new(2.0, 0.0, 0.0);
        let far = Vec3::new(50.0, 0.0, 0.0);
        let candidates = vec![
            (9u64, 0u8, origin, 100.0), // shooter team: skipped
            (4u64, 1u8, near, 100.0),   // enemy in radius
            (7u64, 1u8, near, 100.0),   // enemy in radius (order!)
            (2u64, 1u8, far, 100.0),    // too far
            (5u64, 0u8, near, 100.0),   // friendly: never
            (6u64, 1u8, near, 0.0),     // dead: never
        ];
        assert_eq!(splash_targets(&candidates, origin, 3.0, 0), vec![4, 7]);
        assert!(splash_targets(&candidates, origin, 0.0, 0).is_empty());
        assert!(splash_targets(&candidates, far, 3.0, 0) == vec![2]);
    }

    /// Aimed single shot: aligned turret, zeroed cooldown, static target.
    /// Returns (shooter, target).
    fn aimed_duel(
        app: &mut App,
        shooter_id: u32,
        shooter_kind: UnitKind,
        target_id: u32,
        target_kind: UnitKind,
        distance: f32,
    ) -> (Entity, Entity) {
        let shooter = app
            .world_mut()
            .spawn(combatant(
                shooter_id,
                0,
                Vec3::new(0.0, 0.8, 0.0),
                UnitOrder::HoldPosition,
                shooter_kind,
            ))
            .id();
        let target = app
            .world_mut()
            .spawn(combatant(
                target_id,
                1,
                Vec3::new(distance, 0.8, 0.0),
                UnitOrder::Idle,
                target_kind,
            ))
            .id();
        app.world_mut()
            .entity_mut(shooter)
            .insert(AttackTarget(target));
        app.world_mut()
            .entity_mut(shooter)
            .insert(WeaponState { remaining: 0.0 });
        let aim = crate::movement::yaw_toward(Vec3::X * distance);
        app.world_mut().entity_mut(shooter).insert(TurretYaw(aim));
        (shooter, target)
    }

    #[test]
    fn laser_hits_instantly_with_beam() {
        let mut app = combat_app();
        let (_, target) = aimed_duel(
            &mut app,
            700,
            UnitKind::LaserTank,
            701,
            UnitKind::HeavyTank,
            15.0,
        );
        for _ in 0..120 {
            app.update();
        }
        // 11 dmg/1.1s over 2s: 1-2 hits landed, beam flashes came and went.
        let hp = app.world().get::<Health>(target).unwrap().current;
        assert!((170.0 - 22.0 - 1e-3..170.0).contains(&hp), "hp={hp}");
        let flashes = app
            .world_mut()
            .query::<&BeamFlash>()
            .iter(app.world())
            .count();
        assert!(flashes <= 2, "flashes must decay, got {flashes}");
    }

    #[test]
    fn mortar_splashes_groups() {
        let mut app = combat_app();
        let (_, first) = aimed_duel(
            &mut app,
            710,
            UnitKind::MortarTank,
            711,
            UnitKind::HeavyTank,
            10.0,
        );
        let second = app
            .world_mut()
            .spawn(combatant(
                712,
                1,
                Vec3::new(10.0, 0.8, 2.5),
                UnitOrder::Idle,
                UnitKind::HeavyTank,
            ))
            .id();
        // One shell (~0.6s flight) then splash 3.5 covers both at 2.5m.
        for _ in 0..150 {
            app.update();
        }
        let hp1 = app.world().get::<Health>(first).unwrap().current;
        let hp2 = app.world().get::<Health>(second).unwrap().current;
        assert!(hp1 < 170.0, "direct victim damaged, hp={hp1}");
        assert!(hp2 < 170.0, "splash victim damaged, hp={hp2}");
    }

    #[test]
    fn top_attack_lands_delayed_and_dodgeable() {
        let mut app = combat_app();
        let (_, target) = aimed_duel(
            &mut app,
            720,
            UnitKind::SkyArtillery,
            721,
            UnitKind::HeavyTank,
            30.0,
        );
        // Fall from 42m at 30 m/s ≈ 1.4s: nothing landed yet at 0.5s...
        for _ in 0..30 {
            app.update();
        }
        assert_eq!(app.world().get::<Health>(target).unwrap().current, 170.0);
        // ...but the delayed AoE connects afterwards.
        for _ in 0..170 {
            app.update();
        }
        let hp = app.world().get::<Health>(target).unwrap().current;
        assert!(hp < 170.0, "delayed strike landed, hp={hp}");
    }

    #[test]
    fn mg_spread_misses_sometimes() {
        let mut app = combat_app();
        let (_, target) = aimed_duel(
            &mut app,
            730,
            UnitKind::MgTank,
            731,
            UnitKind::HeavyTank,
            15.0,
        );
        // 0.28s cycle over 10s ≈ 35 shots of 3 dmg: some must spray wide
        // (0.055 rad at 15m reaches 0.8m off a 0.7m hit disc).
        for _ in 0..600 {
            app.update();
        }
        let hp = app.world().get::<Health>(target).unwrap().current;
        let dealt = 170.0 - hp;
        assert!(dealt > 20.0, "mitra must connect, dealt={dealt}");
        assert!(dealt < 35.0 * 3.0, "spread must miss some, dealt={dealt}");
    }

    #[test]
    fn dumbfire_rocket_alpha_with_light_splash() {
        let mut app = combat_app();
        let (_, first) = aimed_duel(
            &mut app,
            740,
            UnitKind::RocketTank,
            741,
            UnitKind::HeavyTank,
            18.0,
        );
        let second = app
            .world_mut()
            .spawn(combatant(
                742,
                1,
                Vec3::new(18.0, 0.8, 2.0),
                UnitOrder::Idle,
                UnitKind::HeavyTank,
            ))
            .id();
        // 2.4s cycle: first impact (~0.7s flight) then splash 2.5 clips both.
        for _ in 0..300 {
            app.update();
        }
        let hp1 = app.world().get::<Health>(first).unwrap().current;
        let hp2 = app.world().get::<Health>(second).unwrap().current;
        assert!(hp1 <= 170.0 - 26.0 + 1e-3, "direct alpha landed, hp={hp1}");
        assert!(hp2 < 170.0, "splash clipped the neighbour, hp={hp2}");
    }
}
