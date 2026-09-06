use super::*;
use crate::{
    economy::{
        Account, Economy,
        tests::{app, building},
    },
    orders::{self, UnitOrder},
    production::{Factory, free_exit},
    units::{UnitIds, UnitKind},
};

#[test]
fn placement_rejects_bounds_obstacles_units_and_invalid_base_rules() {
    let mut grid = NavGrid::new(crate::navigation::HALF_SIZE, 2.5, vec![]);
    let kind = BuildingKind::Solar;
    assert!(valid_ground(&grid, kind, Vec3::ZERO, &[]).is_ok());
    assert!(valid_ground(&grid, kind, Vec3::X * 199.0, &[]).is_err());
    assert!(valid_ground(&grid, kind, Vec3::ZERO, &[(Vec3::X, 0.55)]).is_err());
    grid.replace_dynamic(
        0,
        &[Obstacle {
            center: Vec2::ZERO,
            half_size: Vec2::splat(2.0),
        }],
    );
    assert!(valid_ground(&grid, kind, Vec3::X * 4.0, &[]).is_err());
    // Builders build anywhere: no live builder -> no placement.
    assert!(placement_rule(Team(0), Vec3::ZERO, &[], &[]).is_err());
    // One live builder anywhere on the team enables far placement: the
    // builder walks over, work starts on arrival (see economy site_power).
    let builders = [(Team(0), Vec3::ZERO, COMMAND_BUILD_RADIUS)];
    assert!(placement_rule(Team(0), Vec3::X * 150.0, &[], &builders).is_ok());
    // Wrong team, and a second site while one is active, are rejected.
    assert!(placement_rule(Team(1), Vec3::ZERO, &[], &builders).is_err());
    let busy = [(Team(0), BuildingKind::Metal, Vec3::ZERO, true)];
    assert!(placement_rule(Team(0), Vec3::ZERO, &busy, &builders).is_err());
    // Factories never enable placement: the lab only makes units.
    let base = [(Team(0), BuildingKind::Factory, Vec3::ZERO, false)];
    assert!(placement_rule(Team(0), Vec3::ZERO, &base, &[]).is_err());
}
#[test]
fn build_grid_snaps_centers_idempotently() {
    // 2m step, XZ only, Y preserved for spawn height.
    assert_eq!(snap_to_grid(Vec3::new(3.4, 0.0, -2.6)), Vec3::new(4.0, 0.0, -2.0));
    assert_eq!(snap_to_grid(Vec3::new(3.0, 1.2, 2.0)), Vec3::new(4.0, 1.2, 2.0));
    let p = Vec3::new(-8.0, 0.5, 12.0);
    assert_eq!(snap_to_grid(p), p);
    assert_eq!(snap_to_grid(snap_to_grid(Vec3::new(1.2, 0.0, 8.8))), Vec3::new(2.0, 0.0, 8.0));
    // Snapped centers keep every footprint inside the map when the raw
    // click was valid: max rounding shift is half a step (1m).
    for kind in BuildingKind::ALL {
        let half = kind.stats().half.max_element();
        let edge = crate::navigation::HALF_SIZE - half - 2.0;
        let snapped = snap_to_grid(Vec3::new(edge + 0.9, 0.0, edge - 0.9));
        assert!((snapped.x - (edge + 0.9)).abs() <= 1.0 + f32::EPSILON);
        assert!((snapped.z - (edge - 0.9)).abs() <= 1.0 + f32::EPSILON);
    }
}
#[test]
fn footprint_occupies_and_destruction_frees_nav_without_rebuilding_unchanged_frames() {
    let mut app = app(0.05);
    let site = building(&mut app, 0, BuildingKind::Metal, Vec3::ZERO, false);
    app.update();
    assert!(!app.world().resource::<NavGrid>().is_walkable(Vec3::ZERO));
    assert!(
        !app.world()
            .resource::<NavGrid>()
            .segment_clear(Vec3::X * -10.0, Vec3::X * 10.0)
    );
    let obstacles = app.world().resource::<NavGrid>().obstacles.len();
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(obstacles, app.world().resource::<NavGrid>().obstacles.len());
    app.world_mut().get_mut::<Health>(site).unwrap().current = 0.0;
    app.update();
    assert!(app.world().get_entity(site).is_err());
    assert!(app.world().resource::<NavGrid>().is_walkable(Vec3::ZERO));
}
#[test]
fn queues_cancel_without_refund_and_destroyed_factory_stops_spending() {
    let mut app = app(0.05);
    let f = building(&mut app, 0, BuildingKind::Factory, Vec3::ZERO, true);
    app.world_mut()
        .resource_mut::<Economy>()
        .0
        .insert(0, Account::default());
    {
        let mut q = app.world_mut().get_mut::<Factory>(f).unwrap();
        q.enqueue(UnitKind::Tank);
        q.enqueue(UnitKind::Scout);
        q.enqueue(UnitKind::Tank);
    }
    app.update();
    let spent = app.world().resource::<Economy>().0[&0].stock;
    {
        let mut q = app.world_mut().get_mut::<Factory>(f).unwrap();
        assert_eq!(q.queue[1].project.done, 0.0);
        q.cancel(1);
        assert_eq!(q.queue.len(), 2);
        q.cancel(0);
        assert_eq!(q.queue[0].project.done, 0.0);
    }
    assert_eq!(spent, app.world().resource::<Economy>().0[&0].stock);
    app.world_mut().get_mut::<Health>(f).unwrap().current = 0.0;
    app.update();
    app.update();
    assert_eq!(spent, app.world().resource::<Economy>().0[&0].stock);
    assert!(app.world().get_entity(f).is_err());
}
#[test]
fn blocked_product_is_retained_then_spawns_once_with_all_capabilities() {
    let mut app = app(0.05);
    let f = building(&mut app, 0, BuildingKind::Factory, Vec3::ZERO, true);
    {
        let mut factory = app.world_mut().get_mut::<Factory>(f).unwrap();
        factory.enqueue(UnitKind::Tank);
        factory.queue[0].project.done = unit_cost(UnitKind::Tank).work;
        factory.rally = Some(Vec3::X * 500.0);
    }
    for _ in 0..4 {
        app.update();
    }
    assert!(app.world().get::<Factory>(f).unwrap().blocked);
    assert_eq!(
        app.world_mut().query::<&Unit>().iter(app.world()).count(),
        0
    );
    let stock = app.world().resource::<Economy>().0[&0].stock;
    app.world_mut().get_mut::<Factory>(f).unwrap().rally = Some(Vec3::new(20.0, 0.0, 20.0));
    app.update();
    assert!(app.world().get::<Factory>(f).unwrap().queue.is_empty());
    assert_eq!(stock, app.world().resource::<Economy>().0[&0].stock);
    for _ in 0..3 {
        app.update();
    }
    let entities: Vec<_> = app
        .world_mut()
        .query_filtered::<Entity, With<Unit>>()
        .iter(app.world())
        .collect();
    assert_eq!(entities.len(), 1);
    let unit = app.world().entity(entities[0]);
    assert!(
        unit.contains::<Selectable>()
            && unit.contains::<crate::movement::Movement>()
            && unit.contains::<Health>()
            && unit.contains::<crate::combat::Weapon>()
            && unit.contains::<Team>()
    );
    assert!(matches!(
        unit.get::<UnitOrder>(),
        Some(UnitOrder::Move { .. })
    ));
    let id = unit.get::<Unit>().unwrap().0;
    assert_ne!(app.world_mut().resource_mut::<UnitIds>().allocate(), id);
}
#[test]
fn no_exit_through_walls_or_bodies_and_no_remote_teleport() {
    let grid = NavGrid::new(crate::navigation::HALF_SIZE, 2.5, vec![]);
    let half = BuildingKind::Factory.stats().half;
    assert!(free_exit(&grid, Vec3::ZERO, half, 0.55, &[], None).is_some());
    let bodies = [(Vec3::ZERO, 30.0)];
    assert!(free_exit(&grid, Vec3::ZERO, half, 0.55, &bodies, None).is_none());
    let walls = NavGrid::new(
        crate::navigation::HALF_SIZE,
        2.5,
        vec![Obstacle {
            center: Vec2::ZERO,
            half_size: Vec2::splat(20.0),
        }],
    );
    assert!(free_exit(&walls, Vec3::ZERO, half, 0.55, &[], None).is_none());
}
#[test]
fn produced_unit_attacks_and_destroys_building_without_mobile_components() {
    let mut app = app(0.05);
    let target = building(&mut app, 1, BuildingKind::Solar, Vec3::ZERO, true);
    app.world_mut().get_mut::<Health>(target).unwrap().current = 20.0;
    let id = app.world_mut().resource_mut::<UnitIds>().allocate();
    let unit = crate::units::spawn_combat_unit(
        &mut app.world_mut().commands(),
        id,
        Team(0),
        UnitKind::Tank,
        Vec3::X * 12.0,
    );
    app.world_mut().flush();
    orders::queue_attack(&mut app.world_mut().commands().entity(unit), target);
    app.world_mut().flush();
    assert!(app.world().get::<Unit>(target).is_none());
    assert!(
        app.world()
            .get::<crate::movement::Movement>(target)
            .is_none()
    );
    for _ in 0..150 {
        app.update();
    }
    assert!(app.world().get_entity(target).is_err());
    assert!(app.world().get_entity(unit).is_ok());
    assert!(app.world().resource::<NavGrid>().has_clearance(Vec3::ZERO));
}
