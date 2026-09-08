use super::*;
use crate::{
    navigation::{CELL_SIZE, HALF_SIZE, NavGrid, NavigationPlugin},
    production::{Factory, ProductionPlugin},
    scenario::Scenario,
    structures::{self, StructuresPlugin},
    units::{UnitKind, UnitPlugin},
};
use bevy::time::TimeUpdateStrategy;
use std::time::Duration;

pub(crate) fn app(dt: f64) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            dt,
        )))
        .insert_resource(Scenario::Benchmark { per_team: 0 })
        .add_plugins(bevy::asset::AssetPlugin::default())
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .add_plugins((
            NavigationPlugin,
            crate::spatial::SpatialPlugin,
            UnitPlugin { visuals: false },
            crate::combat::CombatPlugin,
            crate::movement::MovementPlugin,
            EconomyPlugin,
            StructuresPlugin { visuals: false },
            ProductionPlugin,
        ))
        .insert_resource(NavGrid::new(HALF_SIZE, CELL_SIZE, vec![]));
    app.finish();
    app.cleanup();
    app.update();
    app
}
pub(crate) fn building(
    app: &mut App,
    team: u8,
    kind: BuildingKind,
    at: Vec3,
    complete: bool,
) -> Entity {
    let entity = structures::spawn_building(
        &mut app.world_mut().commands(),
        Team(team),
        kind,
        at,
        complete,
    );
    app.world_mut().flush();
    entity
}
fn stock(app: &mut App, team: u8, values: [f64; 2]) {
    app.world_mut().resource_mut::<Economy>().0.insert(
        team,
        Account {
            stock: values,
            ..default()
        },
    );
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-7, "{a} != {b}");
}

#[test]
fn incomes_capacity_teams_and_fixed_fps() {
    let run = |dt, frames| {
        let mut app = app(dt);
        building(&mut app, 0, BuildingKind::Metal, Vec3::ZERO, true);
        building(&mut app, 1, BuildingKind::Solar, Vec3::X * 25.0, true);
        stock(&mut app, 0, [995.0, 0.0]);
        stock(&mut app, 1, [0.0, 0.0]);
        for _ in 0..frames {
            app.update();
        }
        app.world().resource::<Economy>().0.clone()
    };
    let a = run(0.025, 400);
    let b = run(0.1, 100);
    for team in 0..2 {
        for r in 0..2 {
            close(a[&team].stock[r], b[&team].stock[r]);
        }
    }
    close(a[&0].stock[0], 1000.0);
    close(a[&0].stock[1], 0.0);
    close(a[&1].stock[0], 0.0);
    close(a[&1].stock[1], 120.0);
}
#[test]
fn fair_allocation_does_not_freeze_metal_only_recovery() {
    assert_eq!(
        allocate([5.0, 5.0], &[[5.0, 5.0], [5.0, 5.0]]),
        vec![0.5, 0.5]
    );
    assert_eq!(
        allocate([10.0, 0.0], &[[5.0, 5.0], [5.0, 0.0]]),
        vec![0.0, 1.0]
    );
    let forward = allocate([5.0, 1.0], &[[4.0, 2.0], [6.0, 0.0]]);
    let reverse = allocate([5.0, 1.0], &[[6.0, 0.0], [4.0, 2.0]]);
    close(forward[0], reverse[1]);
    close(forward[1], reverse[0]);
}
#[test]
fn metal_yield_multiplies_spot_income() {
    // G1: il Metal eredita il mult dallo spot (centro ×2 → 10/s).
    // Spawn diretti senza componente: 5/s invariati (back-compat).
    let mut app = app(0.05);
    let hot = building(&mut app, 0, BuildingKind::Metal, Vec3::ZERO, true);
    app.world_mut()
        .commands()
        .entity(hot)
        .insert(crate::structures::MetalYield(2.0));
    app.world_mut().flush();
    building(&mut app, 1, BuildingKind::Metal, Vec3::X * 25.0, true);
    for _ in 0..20 {
        app.update();
    }
    close(app.world().resource::<Economy>().0[&0].income[0], 10.0);
    close(app.world().resource::<Economy>().0[&1].income[0], 5.0);
}
#[test]
fn proportional_stall_resume_exact_costs_and_inactive_sites() {
    let mut app = app(0.05);
    let site = building(&mut app, 0, BuildingKind::Metal, Vec3::ZERO, false);
    stock(&mut app, 0, [100.0, 0.0]);
    for _ in 0..20 {
        app.update();
    }
    close(app.world().get::<Construction>(site).unwrap().0.done, 0.0);
    close(app.world().resource::<Economy>().0[&0].income[0], 0.0);
    app.world_mut()
        .resource_mut::<Economy>()
        .0
        .get_mut(&0)
        .unwrap()
        .stock[1] = 80.0;
    for _ in 0..100 {
        app.update();
    }
    let p = &app.world().get::<Construction>(site).unwrap().0;
    close(p.done, 50.0);
    close(app.world().resource::<Economy>().0[&0].stock[0], 50.0);
    close(app.world().resource::<Economy>().0[&0].stock[1], 40.0);
    for _ in 0..100 {
        app.update();
    }
    assert!(app.world().get::<Construction>(site).is_none());
    let a = &app.world().resource::<Economy>().0[&0];
    close(a.stock[0], 0.0);
    close(a.stock[1], 0.0);
    assert!(a.stock.iter().all(|v| *v >= 0.0));
    app.update();
    close(app.world().resource::<Economy>().0[&0].income[0], 5.0);
}
#[test]
fn simultaneous_consumers_share_resources_regardless_of_archetype_migration() {
    let mut app = app(0.05);
    let a = building(&mut app, 0, BuildingKind::Factory, Vec3::ZERO, true);
    let b = building(&mut app, 0, BuildingKind::Factory, Vec3::X * 30.0, true);
    for e in [a, b] {
        app.world_mut()
            .get_mut::<Factory>(e)
            .unwrap()
            .enqueue(UnitKind::HeavyTank);
    }
    // Force different ECS chunk order while retaining stable identities.
    app.world_mut()
        .entity_mut(a)
        .insert(crate::selection::Selected);
    stock(&mut app, 0, [0.45, 1.1]);
    app.update();
    // HeavyTank [110,260] work 120 at factory power 10: 0.5 work per 0.05s
    // tick, and metal binds the fair split: f = 0.45 / (2 * 110*0.5/120).
    let work = 10.0 * 0.05;
    let fraction = 0.45 / (2.0 * (110.0 * work / 120.0));
    for e in [a, b] {
        close(
            app.world().get::<Factory>(e).unwrap().queue[0].project.done,
            work * fraction,
        );
    }
    let a = &app.world().resource::<Economy>().0[&0];
    close(a.stock[0], 0.0);
    close(a.stock[1], 1.1 - 2.0 * (260.0 * work / 120.0) * fraction);
}
#[test]
fn fractional_final_tick_charges_only_remaining_work() {
    let mut app = app(0.05);
    let site = building(&mut app, 0, BuildingKind::Solar, Vec3::ZERO, false);
    app.world_mut()
        .get_mut::<Construction>(site)
        .unwrap()
        .0
        .done = 79.9;
    stock(&mut app, 0, [1.0, 0.0]);
    app.update();
    assert!(app.world().get::<Construction>(site).is_none());
    close(app.world().resource::<Economy>().0[&0].stock[0], 0.9);
}

fn spawn_unit(app: &mut App, id: u32, team: u8, kind: UnitKind, at: Vec3) -> Entity {
    let entity =
        crate::units::spawn_combat_unit(&mut app.world_mut().commands(), id, Team(team), kind, at);
    app.world_mut().flush();
    entity
}

#[test]
fn build_power_sums_builders_and_stalls_when_they_die() {
    use crate::economy::balance::{COMMANDER_BUILD_POWER, ENGINEER_BUILD_POWER};
    let mut app = app(0.05);
    // Team 0: site + commander + engineer. Team 1: commander keeps the world
    // non-empty so team 0 stalls at 0 instead of hitting the no-builder
    // fallback once its builders die.
    let site = building(&mut app, 0, BuildingKind::Metal, Vec3::ZERO, false);
    let cmd = spawn_unit(&mut app, 100, 0, UnitKind::Commander, Vec3::X * -20.0);
    let eng = spawn_unit(&mut app, 101, 0, UnitKind::Engineer, Vec3::X * -10.0);
    let _other = spawn_unit(&mut app, 102, 1, UnitKind::Commander, Vec3::X * 60.0);
    // Explicit build tasks: proximity alone is not enough.
    for e in [cmd, eng] {
        crate::orders::queue_build(&mut app.world_mut().commands().entity(e), site);
    }
    app.world_mut().flush();
    stock(&mut app, 0, [1000.0, 1000.0]);
    for _ in 0..10 {
        app.update();
    }
    let speed = app.world().get::<Construction>(site).unwrap().0.speed;
    close(speed, COMMANDER_BUILD_POWER + ENGINEER_BUILD_POWER);
    // Engineer is unarmed but builds; commander carries both guns.
    assert!(app.world().get::<crate::combat::Weapon>(eng).is_none());
    assert!(app.world().get::<crate::units::Builder>(eng).is_some());
    assert!(app.world().get::<crate::combat::Weapon>(cmd).is_some());
    // Kill team 0 builders: construction stalls, team 1 unaffected.
    for e in [cmd, eng] {
        app.world_mut()
            .get_mut::<crate::combat::Health>(e)
            .unwrap()
            .current = 0.0;
    }
    for _ in 0..5 {
        app.update();
    }
    assert!(app.world().get_entity(cmd).is_err());
    assert!(app.world().get_entity(eng).is_err());
    let done_before = app.world().get::<Construction>(site).unwrap().0.done;
    for _ in 0..10 {
        app.update();
    }
    let after = &app.world().get::<Construction>(site).unwrap().0;
    close(after.done, done_before);
    close(after.speed, 0.0);
}

#[test]
fn tasked_builder_marches_to_standoff_then_builds_and_pauses_when_moved() {
    use crate::movement::queue_move;
    let mut app = app(0.05);
    // Factory site (200 work): never completes inside this test at 5 work/s.
    let site = building(&mut app, 0, BuildingKind::Factory, Vec3::ZERO, false);
    let eng = spawn_unit(&mut app, 300, 0, UnitKind::Engineer, Vec3::X * 100.0);
    stock(&mut app, 0, [1000.0, 1000.0]);
    // Stalled while out of range with an explicit task but nobody in reach.
    crate::orders::queue_build(&mut app.world_mut().commands().entity(eng), site);
    app.world_mut().flush();
    for _ in 0..10 {
        app.update();
    }
    close(app.world().get::<Construction>(site).unwrap().0.done, 0.0);
    // Resolve marches to the footprint-edge stand-off and holds there with
    // the Build order live (it completes only when the site does).
    for _ in 0..400 {
        app.update();
    }
    assert!(
        app.world()
            .get::<crate::movement::MoveTarget>(eng)
            .is_none()
    );
    assert!(matches!(
        app.world().get::<crate::orders::UnitOrder>(eng),
        Some(crate::orders::UnitOrder::Build { .. })
    ));
    let working = &app.world().get::<Construction>(site).unwrap().0;
    assert!(working.done > 0.0);
    close(working.speed, crate::economy::balance::ENGINEER_BUILD_POWER);
    // Manual move away replaces Build: the site pauses.
    queue_move(&mut app.world_mut().commands().entity(eng), Vec3::X * 100.0);
    app.world_mut().flush();
    for _ in 0..400 {
        app.update();
    }
    let stalled = &app.world().get::<Construction>(site).unwrap().0;
    close(stalled.speed, 0.0);
    assert_eq!(stalled.status(), "WAITING FOR BUILDER: move one in range");
}
#[test]
fn factory_builds_engineer_but_rejects_commander() {
    let mut app = app(0.05);
    let f = building(&mut app, 0, BuildingKind::Factory, Vec3::ZERO, true);
    let mut q = app.world_mut().get_mut::<Factory>(f).unwrap();
    assert!(q.enqueue(UnitKind::Engineer));
    assert!(!q.enqueue(UnitKind::Commander));
    assert_eq!(q.queue.len(), 1);
}

#[test]
fn range_without_task_builds_nothing_but_guard_assist_stacks() {
    use crate::economy::balance::{COMMANDER_BUILD_POWER, ENGINEER_BUILD_POWER};
    let mut app = app(0.05);
    let site = building(&mut app, 0, BuildingKind::Metal, Vec3::ZERO, false);
    // Bystander WITHIN range but Idle: proximity alone builds nothing.
    let idle = spawn_unit(&mut app, 400, 0, UnitKind::Engineer, Vec3::X * 5.0);
    stock(&mut app, 0, [1000.0, 1000.0]);
    for _ in 0..20 {
        app.update();
    }
    close(app.world().get::<Construction>(site).unwrap().0.done, 0.0);
    // Direct task + guard-assist both in range: powers stack.
    let cmd = spawn_unit(&mut app, 401, 0, UnitKind::Commander, Vec3::X * -10.0);
    crate::orders::queue_build(&mut app.world_mut().commands().entity(cmd), site);
    crate::orders::queue_guard(&mut app.world_mut().commands().entity(idle), cmd);
    app.world_mut().flush();
    for _ in 0..20 {
        app.update();
    }
    let working = &app.world().get::<Construction>(site).unwrap().0;
    close(working.speed, COMMANDER_BUILD_POWER + ENGINEER_BUILD_POWER);
}

#[test]
fn site_stalls_out_of_range_and_resumes_on_arrival() {
    use crate::economy::balance::ENGINEER_BUILD_POWER;
    let mut app = app(0.05);
    // Engineer far from the site: alive but out of its 16m radius.
    let site = building(&mut app, 0, BuildingKind::Metal, Vec3::ZERO, false);
    let eng = spawn_unit(&mut app, 200, 0, UnitKind::Engineer, Vec3::X * 100.0);
    crate::orders::queue_build(&mut app.world_mut().commands().entity(eng), site);
    app.world_mut().flush();
    stock(&mut app, 0, [1000.0, 1000.0]);
    for _ in 0..10 {
        app.update();
    }
    let stalled = &app.world().get::<Construction>(site).unwrap().0;
    close(stalled.done, 0.0);
    close(stalled.speed, 0.0);
    assert_eq!(stalled.status(), "WAITING FOR BUILDER: move one in range");
    // Builder walks into range: work resumes at engineer power.
    app.world_mut()
        .get_mut::<Transform>(eng)
        .unwrap()
        .translation = Vec3::X * 10.0;
    for _ in 0..10 {
        app.update();
    }
    let working = &app.world().get::<Construction>(site).unwrap().0;
    assert!(working.done > 0.0);
    close(working.speed, ENGINEER_BUILD_POWER);
    assert_eq!(working.status(), "Working");
}
#[test]
fn allocation_conserves_resources_for_many_asymmetric_consumers() {
    for n in 1..40 {
        let demand: Vec<_> = (0..n)
            .map(|i| [((i * 17 + 3) % 19) as f64, ((i * 7) % 23) as f64])
            .collect();
        let stock = [n as f64 * 0.4, n as f64 * 0.7];
        let shares = allocate(stock, &demand);
        assert!(shares.iter().all(|f| (0.0..=1.0).contains(f)));
        for r in 0..2 {
            assert!(
                demand
                    .iter()
                    .zip(&shares)
                    .map(|(d, f)| d[r] * f)
                    .sum::<f64>()
                    <= stock[r] + 1e-8
            );
        }
    }
}
