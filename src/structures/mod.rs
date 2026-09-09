//! MVP base construction: one site/team, fixed team work rate, factory radius.
mod deposits;
pub use deposits::{
    MetalDeposit, MetalDeposits, MetalYield, Mirror, SPOT_CAPTURE, SPOT_MAGNET,
    apply_deposit_yield, count_free, free_deposits, generate_deposits, metal_spot_ok, nearest_free,
};
mod visuals;
pub use visuals::draw_rallies;
#[cfg(test)]
mod tests;
use crate::{
    combat::{
        AcquisitionRange, DeathSystems, Health, ResolveBehaviour, TurretYaw, Weapon, WeaponState,
    },
    economy::{EconomyTick, Project, balance::*},
    movement::MovementSystems,
    navigation::{NavGrid, Obstacle, PlanPaths, Route, UNIT_CLEARANCE},
    orders::UnitOrder,
    production::Factory,
    scenario::Scenario,
    spatial::{SpatialSystems, apply_avoidance},
    units::{CollisionRadius, Selectable, Team, Unit, UnitKind},
};
use bevy::prelude::*;

#[derive(Component)]
pub struct Building;
/// Combat-capable static defense marker (future hook: overheat, targeting
/// priorities, AA — query `With<Turret>` without touching shared combat).
/// Behavior (acquire/validate/fire) is reused from units via the Weapon
/// bundle below, so no duplicated targeting logic.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turret;
#[derive(Component)]
pub struct Construction(pub Project);
#[derive(Component, Clone, Copy)]
pub struct Footprint(pub Vec3);
#[derive(Resource, Default)]
pub struct Placement {
    pub kind: Option<BuildingKind>,
    /// Builders tasked at Build-button press (frozen selection). Only these
    /// march on confirm — never a random nearest one. Cleared on confirm or
    /// cancel together with `kind`.
    pub builders: Vec<Entity>,
    pub message: String,
}
#[derive(Resource, Default)]
struct Occupancy {
    entries: Vec<(Entity, Obstacle)>,
    #[cfg(test)]
    rebuilds: u64,
}
#[derive(Component)]
struct BeforeMotion(Vec3);

pub struct StructuresPlugin {
    pub visuals: bool,
}
impl Plugin for StructuresPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Placement>()
            .init_resource::<Occupancy>()
            .add_systems(Startup, setup_base.after(crate::units::spawn_units))
            .add_systems(Startup, setup_deposits)
            .add_systems(FixedUpdate, finish_sites.after(EconomyTick))
            .add_systems(
                Update,
                (sync_occupancy, remember_motion)
                    .chain()
                    .before(SpatialSystems)
                    .before(ResolveBehaviour)
                    .before(PlanPaths),
            )
            .add_systems(
                Update,
                constrain_motion
                    .after(apply_avoidance)
                    .after(MovementSystems),
            )
            .add_systems(PostUpdate, sync_occupancy.after(DeathSystems));
        if self.visuals {
            app.add_plugins(visuals::BuildingVisualsPlugin);
            app.add_systems(Startup, spawn_deposit_markers.after(setup_deposits));
        }
    }
}

/// Depositi metallo (G1): terreno generato da grid + spawn, identico per ogni
/// team. Option-res: harness di test senza mappa girano a depositi vuoti
/// (regola chiusa → niente Metal, mai panic).
fn setup_deposits(
    mut commands: Commands,
    grid: Option<Res<NavGrid>>,
    scenario: Option<Res<Scenario>>,
) {
    let deposits = match (grid, scenario) {
        (Some(grid), Some(scenario)) => {
            let a = scenario.center(0).xz();
            let b = scenario.center(1).xz();
            generate_deposits(&grid, a, b, Mirror::Point)
        }
        _ => Vec::new(),
    };
    commands.insert_resource(MetalDeposits(deposits));
}

/// Marker depositi (solo grafica): cristalli oro, centro più grosso e caldo.
/// Neutri e sempre visibili: sono terreno, mai coperti dal fog.
fn spawn_deposit_markers(
    mut commands: Commands,
    deposits: Res<MetalDeposits>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    use std::f32::consts::FRAC_PI_4;
    let gold = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.75, 0.2),
        unlit: true,
        ..default()
    });
    let hot = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.45, 0.1),
        unlit: true,
        ..default()
    });
    for dep in &deposits.0 {
        let s = if dep.mult > 1.0 { 1.7 } else { 1.0 };
        let mat = if dep.mult > 1.0 {
            hot.clone()
        } else {
            gold.clone()
        };
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(1.4 * s, 2.4 * s, 1.4 * s))),
            MeshMaterial3d(mat),
            Transform::from_translation(dep.pos.with_y(1.2 * s))
                .with_rotation(Quat::from_rotation_y(FRAC_PI_4)),
        ));
    }
}
pub fn spawn_building(
    commands: &mut Commands,
    team: Team,
    kind: BuildingKind,
    position: Vec3,
    complete: bool,
) -> Entity {
    let stats = kind.stats();
    let mut entity = commands.spawn((
        Building,
        kind,
        team,
        Selectable,
        Footprint(Vec3::new(stats.half.x, stats.height * 0.5, stats.half.y)),
        Health {
            current: stats.health,
            max: stats.health,
        },
        Transform::from_translation(position.with_y(stats.height * 0.5)),
    ));
    if !complete {
        entity.insert(Construction(Project::new(stats.cost)));
    }
    if kind == BuildingKind::Factory {
        entity.insert(Factory::default());
    }
    if kind == BuildingKind::LabT2 {
        entity.insert(Factory {
            tier: 2,
            ..Default::default()
        });
    }
    // Static defense gun: reuses the whole unit combat pipeline
    // (acquire/validate/hold/fire/fog) via HoldPosition — never chases,
    // never receives move orders (no Unit component, see orders). Armed only
    // when complete: construction sites hold fire (see finish_sites).
    if complete {
        arm_turret(&mut entity, kind);
    }
    entity.id()
}

/// Insert the Weapon bundle for a completed turret. Pure helper so both
/// `spawn_building(complete=true)` and `finish_sites` arm identically.
fn arm_turret(entity: &mut EntityCommands, kind: BuildingKind) {
    if let Some(gun) = turret_stats(kind) {
        entity.insert((
            Turret,
            Weapon {
                range: gun.range,
                cooldown: gun.cooldown,
                damage: gun.damage,
                projectile_speed: gun.projectile_speed,
            },
            WeaponState { remaining: 0.0 },
            AcquisitionRange(gun.acquisition),
            TurretYaw::default(),
            UnitOrder::HoldPosition,
        ));
    }
}

/// Grid-based construction: every site snaps its center to this step so
/// buildings line up and micro-overlaps from free-float clicks disappear.
/// 2m squares keep small (2.5) and large (5x4) footprints placeable without
/// forcing the nav cell size (2.5m) onto the base layout.
pub const BUILD_GRID: f32 = 2.0;

/// Snap a ground point onto the build grid (XZ only, Y untouched — spawn
/// sets height from the footprint). Non-finite input passes through so
/// validation still rejects it downstream instead of NaN-poisoning gizmos.
pub fn snap_to_grid(point: Vec3) -> Vec3 {
    if !point.is_finite() {
        return point;
    }
    Vec3::new(
        (point.x / BUILD_GRID).round() * BUILD_GRID,
        point.y,
        (point.z / BUILD_GRID).round() * BUILD_GRID,
    )
}
/// March destination for a tasked builder: a stand-off at the footprint
/// edge, repaired into walkable hull-clear ground. Never the site center:
/// the center sits inside the site's own nav obstacle, so the planner
/// strips the MoveTarget and the builder stands still forever. Edge points
/// stay inside every build radius while remaining plannable.
pub fn site_approach(grid: &NavGrid, site: Vec3, from: Vec3, half: Vec2, body_radius: f32) -> Vec3 {
    let away = from.xz() - site.xz();
    let dir = if away.length_squared() > 1e-6 {
        away.normalize()
    } else {
        Vec2::X
    };
    let edge = site.xz() + dir * (half.length() + body_radius + 1.5);
    grid.clear_point_for(Vec3::new(edge.x, site.y, edge.y), body_radius)
}
pub fn valid_ground(
    grid: &NavGrid,
    kind: BuildingKind,
    position: Vec3,
    units: &[(Vec3, f32)],
) -> Result<(), &'static str> {
    let half = kind.stats().half;
    let center = position.xz();
    if !position.is_finite()
        || (center.abs() + half + Vec2::splat(UNIT_CLEARANCE)).max_element()
            > crate::navigation::HALF_SIZE
    {
        return Err("Outside map");
    }
    if grid.obstacles.iter().any(|o| {
        let delta = (center - o.center).abs();
        if kind == BuildingKind::Wall {
            // Walls tile flush on the 2m grid: strict footprint overlap only,
            // so snapped neighbors touch (delta == sum) without triggering.
            // Rocks (half >= 4) and buildings still block as before.
            delta.x < half.x + o.half_size.x && delta.y < half.y + o.half_size.y
        } else {
            delta.x < half.x + o.half_size.x + 1.6 && delta.y < half.y + o.half_size.y + 1.6
        }
    }) {
        return Err("Overlaps obstacle / building");
    }
    if units.iter().any(|(p, radius)| {
        let delta = (p.xz() - center).abs();
        delta.x < half.x + radius + 2.0 && delta.y < half.y + radius + 2.0
    }) {
        return Err("Unit inside footprint / clearance");
    }
    Ok(())
}
/// Nav obstacle a building would add at `position` (same shape as
/// `sync_occupancy`). Placement probes clone the grid with it to validate
/// paths against the post-placement world.
pub fn building_obstacle(kind: BuildingKind, position: Vec3) -> Obstacle {
    Obstacle {
        center: position.xz(),
        half_size: kind.stats().half,
    }
}
/// Factory placement sanity: a lab whose doors all open into rock (or off
/// map) would queue troops that never spawn — `release_products` retains the
/// finished product forever and the queue stalls. Reject the site up front so
/// builders (player ghost or AI spiral search) pick a spot with a free door.
/// Checked with empty occupancy (units move away; walls don't) and the
/// largest producible hull, so anything queued later can exit. Other
/// buildings don't spawn units and skip the check.
pub fn factory_spawn_ok(
    grid: &NavGrid,
    kind: BuildingKind,
    position: Vec3,
) -> Result<(), &'static str> {
    if !kind.is_factory() {
        return Ok(());
    }
    let half = kind.stats().half;
    let radius = UnitKind::PRODUCIBLE
        .iter()
        .map(|k| crate::units::archetype(*k).radius)
        .fold(0.0, f32::max);
    if crate::production::free_exit(grid, position, half, radius, &[], None).is_none() {
        return Err("Factory exits blocked — pick a spot with a free door");
    }
    Ok(())
}
/// Builders build anywhere with valid ground: the laboratory only makes
/// units. Placement needs one live builder of the team (anywhere); the
/// economy trickles work only from builders within their own radius of the
/// site, so an out-of-range placement means someone has to walk over.
/// One active site per team keeps the streaming economy readable.
pub fn placement_rule(
    team: Team,
    _position: Vec3,
    _buildings: &[(Team, BuildingKind, Vec3, bool)],
    builders: &[(Team, Vec3, f32)],
) -> Result<(), &'static str> {
    // BAR-style: nessun tetto per-team ai cantieri — ogni builder porta avanti
    // il suo; il limite emerge dai builder vivi, non dalle regole. Cantieri
    // multipli (anche stessa specie) sono intenzionali: l'eco parallela è
    // strategia, non exploit. Resta il gate builder-vivi.
    if !builders.iter().any(|(t, _, _)| *t == team) {
        return Err("No builders alive: build an Engineer first");
    }
    Ok(())
}
fn setup_base(
    mut commands: Commands,
    scenario: Res<Scenario>,
    mut grid: ResMut<NavGrid>,
    units: Query<(&Transform, Option<&CollisionRadius>), With<Unit>>,
) {
    // Commander-only start: nessuna base precostruita in Playground.
    // I builder costruiscono ovunque su terreno valido; il lavoro avanza
    // solo dai builder nel loro raggio dal cantiere (vedi economy::site_power).
    // La firma resta per futuri setup scenario; per ora no-op intenzionale.
    let _ = (&mut commands, &scenario, &mut grid, &units);
}
fn finish_sites(
    mut commands: Commands,
    sites: Query<(Entity, &BuildingKind, &Construction, &Health)>,
) {
    for (entity, kind, site, health) in &sites {
        if health.current > 0.0 && site.0.complete() {
            commands.entity(entity).remove::<Construction>();
            // Turrets arm on completion: construction sites hold fire.
            arm_turret(&mut commands.entity(entity), *kind);
        }
    }
}
#[allow(clippy::type_complexity)]
fn sync_occupancy(
    mut commands: Commands,
    mut occupancy: ResMut<Occupancy>,
    mut grid: ResMut<NavGrid>,
    changed_buildings: Query<
        (Entity, &Transform, &BuildingKind),
        (
            With<Building>,
            Or<(Added<Building>, Changed<Transform>, Changed<BuildingKind>)>,
        ),
    >,
    mut removed_buildings: RemovedComponents<Building>,
    routes: Query<(
        Entity,
        &Transform,
        &Route,
        Option<&crate::units::UnitKind>,
        Option<&crate::units::CollisionRadius>,
    )>,
) {
    let previous_count = occupancy.entries.len();
    let mut changed = false;

    for (entity, transform, kind) in &changed_buildings {
        let obstacle = Obstacle {
            center: transform.translation.xz(),
            half_size: kind.stats().half,
        };
        match occupancy
            .entries
            .binary_search_by_key(&entity.to_bits(), |(entry, _)| entry.to_bits())
        {
            Ok(index) => {
                if occupancy.entries[index].1 != obstacle {
                    occupancy.entries[index].1 = obstacle;
                    changed = true;
                }
            }
            Err(index) => {
                occupancy.entries.insert(index, (entity, obstacle));
                changed = true;
            }
        }
    }
    for entity in removed_buildings.read() {
        if let Ok(index) = occupancy
            .entries
            .binary_search_by_key(&entity.to_bits(), |(entry, _)| entry.to_bits())
        {
            occupancy.entries.remove(index);
            changed = true;
        }
    }
    if !changed {
        return;
    }
    #[cfg(test)]
    {
        occupancy.rebuilds += 1;
    }
    grid.replace_dynamic(
        previous_count,
        &occupancy
            .entries
            .iter()
            .map(|(_, obstacle)| *obstacle)
            .collect::<Vec<_>>(),
    );
    // Only routes intersecting changed occupancy are discarded; the existing
    // planner still caps work per frame. No permanent global revision replan.
    // Body-aware: a route that was scout-clear may still be blocked for a
    // large hull, so validate with the same hull the planner used.
    for (entity, transform, route, kind, body) in &routes {
        let radius = kind
            .map(|k| crate::units::archetype(*k).radius)
            .or(body.map(|b| b.0));
        let mut previous = transform.translation;
        if route.points.iter().skip(route.next).any(|point| {
            let blocked = match radius {
                Some(r) => !grid.segment_clear_for(previous, *point, r),
                None => !grid.segment_clear(previous, *point),
            };
            previous = *point;
            blocked
        }) {
            commands.entity(entity).remove::<Route>();
        }
    }
}
fn remember_motion(
    mut commands: Commands,
    mut units: Query<(Entity, &Transform, Option<&mut BeforeMotion>), With<Unit>>,
) {
    for (entity, transform, before) in &mut units {
        if let Some(mut before) = before {
            before.0 = transform.translation;
        } else {
            commands
                .entity(entity)
                .insert(BeforeMotion(transform.translation));
        }
    }
}
/// Swept collision stops route fallback, chase and avoidance from tunnelling
/// through buildings, including at low FPS. This plugin is absent in benchmarks.
fn constrain_motion(
    grid: Res<NavGrid>,
    mut units: Query<(&BeforeMotion, &mut Transform, &CollisionRadius), With<Unit>>,
) {
    for (previous, mut transform, radius) in &mut units {
        let next = transform.translation;
        // Stationary units never tunnel: skip the swept rect scans entirely.
        // Exact equality is intentional — any real displacement (even sub-mm
        // from avoidance/chase) still takes the full body-aware check below.
        if previous.0 == next {
            continue;
        }
        if grid.segment_clear_for(previous.0, next, radius.0) {
            continue;
        }
        // Try sliding along a free axis before stopping. Never repair through
        // an obstacle or teleport to its other side.
        let x = Vec3::new(next.x, next.y, previous.0.z);
        let z = Vec3::new(previous.0.x, next.y, next.z);
        transform.translation = if grid.segment_clear_for(previous.0, x, radius.0) {
            x
        } else if grid.segment_clear_for(previous.0, z, radius.0) {
            z
        } else {
            previous.0
        };
    }
}
