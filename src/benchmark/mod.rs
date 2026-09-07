pub mod cli;
pub mod profile;
// Riusati dalla AI suite (stesso formato metadata/history dei benchmark).
pub use report::{HISTORY_ROOT, Metadata};
mod report;

use crate::{
    combat::{AttackTarget, CombatPlugin, Health, Projectile},
    movement::{MoveTarget, MovementPlugin},
    navigation::{NavGrid, NavigationPlugin, NavigationStats, PlanPaths, Route},
    orders::UnitOrder,
    scenario::Scenario,
    spatial::SpatialPlugin,
    units::{CollisionRadius, Team, Unit, UnitKind, UnitPlugin, arm_bundle},
};
use bevy::{prelude::*, time::TimeUpdateStrategy};
use cli::{Config, SuitePreset, Workload};
use report::{Report, Run, Sample, Stats, TELEMETRY_INTERVAL_TICKS};
use std::{
    collections::HashMap,
    error::Error,
    time::{Duration, Instant},
};

const WARMUP_TICKS: usize = 120;

#[derive(Clone, Copy)]
struct Case {
    workload: Workload,
    per_team: usize,
    ticks: usize,
    repeats: usize,
}

#[derive(Clone, Copy, Default)]
struct Telemetry {
    moving: usize,
    pending: usize,
    engaging: usize,
    projectiles: usize,
}

fn suite_cases(config: &Config) -> Vec<Case> {
    let mut cases = match config.preset {
        SuitePreset::Quick => vec![
            Case {
                workload: Workload::Idle,
                per_team: 1000,
                ticks: 600,
                repeats: 3,
            },
            Case {
                workload: Workload::Crossing,
                per_team: 1000,
                ticks: 3600,
                repeats: 3,
            },
            Case {
                workload: Workload::Crowd,
                per_team: 1000,
                ticks: 900,
                repeats: 3,
            },
            Case {
                workload: Workload::Skirmish,
                per_team: 1000,
                ticks: 600,
                repeats: 3,
            },
        ],
        SuitePreset::Full => {
            let mut cases = Vec::new();
            for per_team in [100, 1000] {
                cases.push(Case {
                    workload: Workload::Idle,
                    per_team,
                    ticks: 600,
                    repeats: 5,
                });
            }
            for per_team in [100, 500, 1000] {
                cases.push(Case {
                    workload: Workload::Crossing,
                    per_team,
                    ticks: 3600,
                    repeats: 5,
                });
            }
            for per_team in [500, 1000, 2500] {
                cases.push(Case {
                    workload: Workload::Crowd,
                    per_team,
                    ticks: 900,
                    repeats: 5,
                });
            }
            for per_team in [100, 500, 1000] {
                cases.push(Case {
                    workload: Workload::Skirmish,
                    per_team,
                    ticks: 1800,
                    repeats: 5,
                });
            }
            cases.push(Case {
                workload: Workload::Skirmish,
                per_team: 5000,
                ticks: 600,
                repeats: 3,
            });
            cases
        }
    };
    if config.ticks_overridden {
        for case in &mut cases {
            case.ticks = config.ticks;
        }
    }
    if config.repeats_overridden {
        for case in &mut cases {
            case.repeats = config.repeats;
        }
    }
    cases
}

fn positions(world: &mut World) -> Vec<(u32, Vec3, bool)> {
    let mut units: Vec<_> = world
        .query::<(&Unit, &Transform, Has<MoveTarget>)>()
        .iter(world)
        .map(|(unit, transform, target)| (unit.0, transform.translation, target))
        .collect();
    units.sort_unstable_by_key(|(id, _, _)| *id);
    units
}

fn crossing_order(world: &mut World) -> Result<(Vec<Vec3>, f64), String> {
    let start = Instant::now();
    let scenario = *world.resource::<Scenario>();
    let mut units: Vec<_> = world
        .query::<(Entity, &Unit, &Team)>()
        .iter(world)
        .map(|(entity, unit, team)| (entity, unit.0, team.0))
        .collect();
    units.sort_unstable_by_key(|(_, id, _)| *id);
    let grid = world.resource::<NavGrid>();
    let destinations: [Option<Vec<Vec3>>; 2] = std::array::from_fn(|team| {
        grid.formation(scenario.per_team(), scenario.center(1 - team), 2.5)
    });
    let mut expected = Vec::with_capacity(units.len());
    for (entity, id, team) in units {
        let goals = destinations[team as usize]
            .as_ref()
            .ok_or_else(|| format!("benchmark formations do not fit the map for team {team}"))?;
        let goal = goals[id as usize % scenario.per_team()];
        world
            .entity_mut(entity)
            .insert(MoveTarget(goal))
            .remove::<Route>();
        expected.push(goal.with_y(0.8));
    }
    Ok((expected, start.elapsed().as_secs_f64() * 1000.0))
}

/// Strategic crowd/combat order issued after neutral warmup. Combat cases
/// are armed here so setup does not leak into the measured simulation.
fn strategic_order(world: &mut World, attack_move: bool) -> (Vec<Vec3>, f64) {
    let start = Instant::now();
    let scenario = *world.resource::<Scenario>();
    let mut units: Vec<_> = world
        .query::<(Entity, &Unit, &Team, &Transform, &UnitKind)>()
        .iter(world)
        .map(|(entity, unit, team, transform, kind)| {
            (
                entity,
                unit.0,
                team.0,
                transform.translation.with_y(0.8),
                *kind,
            )
        })
        .collect();
    units.sort_unstable_by_key(|(_, id, _, _, _)| *id);
    let mut initial = Vec::with_capacity(units.len());
    for (entity, id, team, position, kind) in units {
        let destination = scenario.attack_target(team as usize);
        let mut entity = world.entity_mut(entity);
        if attack_move {
            entity
                .insert(arm_bundle(id, kind))
                .insert(UnitOrder::AttackMove { destination });
        } else {
            // Crowd: same march, plus body separation. CollisionRadius turns
            // on avoidance (SpatialPlugin is loaded for this workload), so
            // crowd measures movement + avoidance without any combat.
            entity.insert((
                UnitOrder::Move { destination },
                CollisionRadius(crate::units::archetype(kind).radius),
            ));
        }
        entity
            .insert(MoveTarget(destination))
            .remove::<(AttackTarget, Route)>();
        initial.push(position);
    }
    (initial, start.elapsed().as_secs_f64() * 1000.0)
}

/// Sustained escort workload: one patrolling ward per team, all other units
/// guarding it at speed 7. The teams stay on their own side, so deaths cannot
/// make follow churn disappear partway through the measurement. This custom
/// diagnostic is deliberately outside the historical benchmark suites.
fn guard_order(world: &mut World) -> Result<(Vec<Vec3>, f64), String> {
    let start = Instant::now();
    let scenario = *world.resource::<Scenario>();
    let mut units: Vec<_> = world
        .query::<(Entity, &Unit, &Team, &Transform, &UnitKind)>()
        .iter(world)
        .map(|(entity, unit, team, transform, kind)| {
            (entity, unit.0, team.0, transform.translation, *kind)
        })
        .collect();
    units.sort_unstable_by_key(|(_, id, _, _, _)| *id);
    let grid = world.resource::<NavGrid>();
    let mut patrols = Vec::new();
    for team in 0..2 {
        let mut points = Vec::new();
        for z in [100.0, -100.0] {
            let ideal = scenario.center(team).with_z(z);
            let point = grid
                .formation(1, ideal, 2.5)
                .and_then(|points| points.into_iter().next())
                .ok_or_else(|| "guard patrol does not fit the map".to_owned())?;
            points.push(point);
        }
        patrols.push(points);
    }
    let mut wards = [None; 2];
    let mut initial = Vec::with_capacity(units.len());
    for (entity, id, team, position, kind) in units {
        let order = if let Some(ward) = wards[team as usize] {
            UnitOrder::Guard { target: ward }
        } else {
            wards[team as usize] = Some(entity);
            UnitOrder::Patrol {
                points: patrols[team as usize].clone(),
                next: 0,
            }
        };
        world
            .entity_mut(entity)
            .insert((
                arm_bundle(id, kind),
                crate::movement::Movement { speed: 7.0 },
                order,
            ))
            .remove::<(MoveTarget, AttackTarget, Route)>();
        initial.push(position);
    }
    Ok((initial, start.elapsed().as_secs_f64() * 1000.0))
}

fn collect_telemetry(world: &mut World) -> Telemetry {
    let mut telemetry = Telemetry::default();
    for route in world
        .query_filtered::<Has<Route>, With<MoveTarget>>()
        .iter(world)
    {
        if route {
            telemetry.moving += 1;
        } else {
            telemetry.pending += 1;
        }
    }
    telemetry.engaging = world
        .query_filtered::<Entity, (With<Unit>, With<AttackTarget>)>()
        .iter(world)
        .count();
    telemetry.projectiles = world
        .query_filtered::<Entity, With<Projectile>>()
        .iter(world)
        .count();
    telemetry
}

fn sample(
    world: &World,
    tick: usize,
    update_ms: f64,
    frame_ms: f64,
    telemetry: Telemetry,
) -> Sample {
    Sample {
        tick,
        update_ms,
        frame_ms,
        planning_ms: world.resource::<NavigationStats>().last_ms,
        moving: telemetry.moving,
        pending: telemetry.pending,
        engaging: telemetry.engaging,
        projectiles: telemetry.projectiles,
    }
}

fn finish(world: &mut World, expected: &[Vec3], run: &mut Run) {
    let positions = positions(world);
    let grid = world.resource::<NavGrid>();
    let mut checksum = 0xcbf29ce484222325_u64;
    let allows_local_steering = matches!(
        run.workload,
        Workload::Crowd | Workload::Skirmish | Workload::Guard
    );
    let requires_arrival = matches!(run.workload, Workload::Idle | Workload::Crossing);
    let mut valid = positions.len() == expected.len() || run.workload == Workload::Skirmish;
    for ((id, position, moving), goal) in positions.iter().zip(expected) {
        if requires_arrival && !moving && position.distance(*goal) < 0.001 {
            run.arrived += 1;
        }
        valid &= position.is_finite()
            && position.x.abs() <= crate::navigation::HALF_SIZE
            && position.z.abs() <= crate::navigation::HALF_SIZE
            && (allows_local_steering
                || (grid.is_walkable(*position) && grid.has_clearance(*position)));
        for value in [
            *id,
            position.x.to_bits(),
            position.y.to_bits(),
            position.z.to_bits(),
        ] {
            checksum = (checksum ^ value as u64).wrapping_mul(0x100000001b3);
        }
    }
    let mut health: Vec<_> = world
        .query::<(&Unit, &Health)>()
        .iter(world)
        .map(|(unit, health)| (unit.0, health.current.to_bits()))
        .collect();
    health.sort_unstable();
    for (id, bits) in health {
        for value in [id as u64, bits as u64] {
            checksum = (checksum ^ value).wrapping_mul(0x100000001b3);
        }
    }
    let stats = world.resource::<NavigationStats>();
    run.planned = stats.planned;
    run.failed = stats.failed;
    run.checksum = checksum;
    match run.workload {
        Workload::Skirmish | Workload::AiTest => {
            run.arrived = positions.len();
            run.kills = expected.len().saturating_sub(positions.len());
            run.correctness_pass = valid
                && run.failed == 0
                && run.kills > 0
                && run.arrived + run.kills == expected.len();
        }
        Workload::Crowd | Workload::Guard => {
            run.arrived = positions.len();
            run.correctness_pass = valid && run.failed == 0 && positions.len() == expected.len();
            if run.workload == Workload::Guard {
                let mut guards = 0;
                let mut wards = 0;
                for order in world.query_filtered::<&UnitOrder, With<Unit>>().iter(world) {
                    match order {
                        UnitOrder::Guard { .. } => guards += 1,
                        UnitOrder::Patrol { .. } => wards += 1,
                        _ => {}
                    }
                }
                run.correctness_pass &= wards == 2 && guards + 2 == expected.len();
            }
        }
        Workload::Idle | Workload::Crossing => {
            run.correctness_pass = valid && run.failed == 0 && run.arrived == expected.len();
        }
    }
}

fn empty_run(
    per_team: usize,
    workload: Workload,
    repeat: usize,
    mode: &'static str,
    ticks: usize,
) -> Run {
    Run {
        per_team,
        workload,
        repeat,
        mode,
        ticks,
        samples: Vec::new(),
        order_ms: 0.0,
        planned: 0,
        failed: 0,
        arrived: 0,
        kills: 0,
        correctness_pass: false,
        timing_valid: true,
        checksum: 0,
    }
}

pub fn run_headless(config: &Config) -> Result<(), Box<dyn Error>> {
    #[cfg(not(feature = "profile-chrome"))]
    if config.profile_capture {
        return Err("profile capture requires Cargo feature profile-chrome".into());
    }

    let preset = if config.suite {
        config.preset.as_str()
    } else if config.profile_capture {
        "profile"
    } else {
        "custom"
    };
    let mut report = Report::new(config.output.clone(), preset)?;
    let cases = if config.suite {
        suite_cases(config)
    } else {
        vec![Case {
            workload: config.workload,
            per_team: config.per_team,
            ticks: config.ticks,
            repeats: 1,
        }]
    };
    let mut checksums = HashMap::new();
    for case in cases {
        for repeat in 1..=case.repeats {
            let scenario = if matches!(case.workload, Workload::Crowd | Workload::Skirmish) {
                Scenario::Skirmish {
                    per_team: case.per_team,
                }
            } else {
                Scenario::Benchmark {
                    per_team: case.per_team,
                }
            };
            let mut app = App::new();
            #[cfg(feature = "profile-chrome")]
            if config.profile_capture {
                app.add_plugins(bevy::log::LogPlugin::default());
            }
            app.add_plugins(MinimalPlugins)
                .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                    1.0 / 60.0,
                )))
                .insert_resource(scenario)
                .add_plugins((
                    NavigationPlugin,
                    UnitPlugin { visuals: false },
                    MovementPlugin,
                ));
            if case.workload == Workload::Crowd {
                app.add_plugins(SpatialPlugin);
            } else if matches!(case.workload, Workload::Skirmish | Workload::Guard) {
                app.add_plugins((SpatialPlugin, CombatPlugin))
                    .add_plugins(bevy::asset::AssetPlugin::default())
                    .init_asset::<Mesh>()
                    .init_asset::<StandardMaterial>();
            }
            app.finish();
            app.cleanup();
            for _ in 0..WARMUP_TICKS {
                app.update();
            }
            *app.world_mut().resource_mut::<NavigationStats>() = NavigationStats::default();
            let mut run = empty_run(case.per_team, case.workload, repeat, "headless", case.ticks);
            let expected = match case.workload {
                Workload::Crossing => {
                    let (expected, ms) = crossing_order(app.world_mut())
                        .map_err(|error| format!("crossing: {error}"))?;
                    run.order_ms = ms;
                    expected
                }
                Workload::Crowd => {
                    let (initial, ms) = strategic_order(app.world_mut(), false);
                    run.order_ms = ms;
                    initial
                }
                Workload::Skirmish | Workload::AiTest => {
                    let (initial, ms) = strategic_order(app.world_mut(), true);
                    run.order_ms = ms;
                    initial
                }
                Workload::Guard => {
                    let (initial, ms) = guard_order(app.world_mut())?;
                    run.order_ms = ms;
                    initial
                }
                Workload::Idle => positions(app.world_mut())
                    .into_iter()
                    .map(|(_, position, _)| position)
                    .collect(),
            };
            run.samples = Vec::with_capacity(case.ticks);
            let mut telemetry = collect_telemetry(app.world_mut());
            #[cfg(feature = "profile-chrome")]
            let _measurement_guard = config.profile_capture.then(|| {
                bevy::log::info_span!(
                    "benchmark_measurement",
                    workload = case.workload.as_str(),
                    per_team = case.per_team,
                    ticks = case.ticks
                )
                .entered()
            });
            for tick in 0..case.ticks {
                let start = Instant::now();
                app.update();
                let update_ms = start.elapsed().as_secs_f64() * 1000.0;
                if tick > 0 && tick.is_multiple_of(TELEMETRY_INTERVAL_TICKS) {
                    telemetry = collect_telemetry(app.world_mut());
                }
                run.samples
                    .push(sample(app.world(), tick, update_ms, 0.0, telemetry));
            }
            #[cfg(feature = "profile-chrome")]
            drop(_measurement_guard);
            finish(app.world_mut(), &expected, &mut run);
            let previous = checksums
                .entry((case.per_team, case.workload, case.ticks))
                .or_insert(run.checksum);
            run.correctness_pass &= *previous == run.checksum;
            let stats = Stats::new(run.samples.iter().map(|sample| sample.update_ms));
            println!(
                "{} vs {} {:8} repeat={} ticks={} mean={:.3}ms p95={:.3}ms p99={:.3}ms arrived={}/{} failed={} kills={} {}",
                case.per_team,
                case.per_team,
                case.workload.as_str(),
                repeat,
                case.ticks,
                stats.mean,
                stats.p95,
                stats.p99,
                run.arrived,
                case.per_team * 2,
                run.failed,
                run.kills,
                if run.correctness_pass { "PASS" } else { "FAIL" }
            );
            report.runs.push(run);
        }
    }
    let artifact = report.write()?;
    if config.record_history {
        let history = report.record_history(&artifact)?;
        println!("Benchmark history: {}", history.display());
    }
    println!("Results: {}", report.directory.canonicalize()?.display());
    if report.runs.iter().any(|run| !run.correctness_pass) {
        return Err("Benchmark correctness checks failed; see report.md".into());
    }
    Ok(())
}

#[derive(Resource)]
pub struct VisualRun {
    config: Config,
    report: Report,
    started: Option<f64>,
    tick_start: Instant,
    expected: Vec<Vec3>,
    telemetry: Telemetry,
    run: Run,
    completed: bool,
    occluded: bool,
}

impl VisualRun {
    pub fn label(&self) -> &'static str {
        if self.started.is_none() {
            "BENCHMARK: warming up"
        } else {
            "BENCHMARK: measuring"
        }
    }
}

pub fn add_graphical(app: &mut App, config: Config) -> Result<(), Box<dyn Error>> {
    let report = Report::new(config.output.clone(), "graphical")?;
    println!(
        "Graphical benchmark: {} vs {}, 3 s warmup + {} s measurement. Output: {}",
        config.per_team,
        config.per_team,
        config.seconds,
        report.directory.display()
    );
    let run = empty_run(config.per_team, config.workload, 1, "graphical", 0);
    app.insert_resource(VisualRun {
        config,
        report,
        started: None,
        tick_start: Instant::now(),
        expected: Vec::new(),
        telemetry: Telemetry::default(),
        run,
        completed: false,
        occluded: false,
    })
    .add_systems(First, |mut state: ResMut<VisualRun>| {
        state.tick_start = Instant::now()
    })
    .add_systems(PreUpdate, track_occlusion)
    .add_systems(Update, start_graphical.before(PlanPaths))
    .add_systems(Last, record_graphical);
    Ok(())
}

fn start_graphical(world: &mut World) {
    let now = world.resource::<Time<Real>>().elapsed_secs_f64();
    if now < 3.0 || world.resource::<VisualRun>().started.is_some() {
        return;
    }
    let workload = world.resource::<VisualRun>().config.workload;
    let result = match workload {
        Workload::Skirmish | Workload::AiTest => Ok(strategic_order(world, true)),
        Workload::Crowd => Ok(strategic_order(world, false)),
        Workload::Guard => guard_order(world),
        Workload::Crossing => crossing_order(world),
        Workload::Idle => Ok((
            positions(world)
                .into_iter()
                .map(|(_, position, _)| position)
                .collect(),
            0.0,
        )),
    };
    let (expected, ms) = match result {
        Ok(result) => result,
        Err(error) => {
            eprintln!("Cannot start benchmark: {error}");
            world.write_message(AppExit::error());
            return;
        }
    };
    *world.resource_mut::<NavigationStats>() = NavigationStats::default();
    let telemetry = collect_telemetry(world);
    let mut state = world.resource_mut::<VisualRun>();
    state.expected = expected;
    state.run.order_ms = ms;
    state.telemetry = telemetry;
    state.started = Some(now);
    #[cfg(feature = "profile-chrome")]
    if state.config.profile_capture {
        measurement_marker(workload);
    }
}

/// Boundary marker for the measurement window. Entered and exited in the
/// same scope (same thread): `EnteredSpan` is explicitly `!Send` and can
/// never live in a resource across frames. The post-processor bounds the
/// window by min/max over every `benchmark_measurement` span, so the single
/// headless span keeps working unchanged. No-op without the trace layer.
#[cfg(feature = "profile-chrome")]
fn measurement_marker(workload: Workload) {
    let _span =
        bevy::log::info_span!("benchmark_measurement", workload = workload.as_str()).entered();
}

fn track_occlusion(
    mut events: MessageReader<bevy::window::WindowOccluded>,
    mut state: ResMut<VisualRun>,
) {
    for event in events.read() {
        state.occluded = event.occluded;
        if event.occluded && state.started.is_some() {
            state.run.timing_valid = false;
        }
    }
}

fn record_graphical(world: &mut World) {
    world.resource_scope(|world, mut state: Mut<VisualRun>| {
        let Some(started) = state.started else {
            return;
        };
        if state.completed {
            return;
        }
        let time = world.resource::<Time<Real>>();
        let now = time.elapsed_secs_f64();
        let frame_ms = time.delta_secs_f64() * 1000.0;
        let update_ms = state.tick_start.elapsed().as_secs_f64() * 1000.0;
        let tick = state.run.samples.len();
        if tick > 0 && tick.is_multiple_of(TELEMETRY_INTERVAL_TICKS) {
            state.telemetry = collect_telemetry(world);
        }
        let telemetry = state.telemetry;
        state.run.samples.push(sample(
            world,
            tick,
            update_ms,
            frame_ms,
            telemetry,
        ));
        state.run.ticks = state.run.samples.len();
        let focused = world
            .query::<&Window>()
            .iter(world)
            .all(|window| window.focused);
        let keys = world
            .resource::<ButtonInput<KeyCode>>()
            .get_pressed()
            .next()
            .is_some();
        let mouse = world
            .resource::<ButtonInput<MouseButton>>()
            .get_pressed()
            .next()
            .is_some();
        let wheel = !world
            .resource::<Messages<bevy::input::mouse::MouseWheel>>()
            .is_empty();
        state.run.timing_valid &= focused && !state.occluded && !keys && !mouse && !wheel;
        if now - started >= state.config.seconds {
            state.completed = true;
            #[cfg(feature = "profile-chrome")]
            if state.config.profile_capture {
                measurement_marker(state.run.workload);
            }
            let mut run = std::mem::replace(
                &mut state.run,
                empty_run(0, Workload::Crossing, 0, "graphical", 0),
            );
            finish(world, &state.expected, &mut run);
            let passed = run.correctness_pass;
            println!(
                "Graphical benchmark: arrived={}/{} failed={} kills={} correctness={} timing_valid={}",
                run.arrived,
                run.per_team * 2,
                run.failed,
                run.kills,
                run.correctness_pass,
                run.timing_valid
            );
            state.report.runs.push(run);
            match state.report.write() {
                Ok(_) => {
                    println!("Results: {}", state.report.directory.display());
                    world.write_message(if passed {
                        AppExit::Success
                    } else {
                        AppExit::error()
                    });
                }
                Err(error) => {
                    eprintln!("Cannot write benchmark report: {error}");
                    world.write_message(AppExit::error());
                }
            }
        }
    });
}

pub fn write_history_report(output: Option<std::path::PathBuf>) -> Result<(), Box<dyn Error>> {
    let directory = report::write_history_report(output)?;
    println!("History report: {}", directory.canonicalize()?.display());
    Ok(())
}
