//! Batteria scenari L1: micro-scenari seeded veloci, un cervello contro setup
//! fisso. Entra in test-suite.sh: fail = nonzero exit. Solo gate robuste
//! (determinismo, onestà fog, nav, milestone larghe calibrate sui run reali);
//! i numeri fini restano descrittivi per tarare 0.16+.

use crate::{
    benchmark::cli::Config,
    combat::Health,
    economy::balance::BuildingKind,
    orders::UnitOrder,
    scenario::Scenario,
    structures::{Building, spawn_building},
    units::{Team, Unit, UnitIds, UnitKind, spawn_combat_unit},
};
use bevy::prelude::*;
use std::collections::BTreeMap;

use super::{director::CheckResult, strategy::Personality};

/// Unità scriptata: spawn a un tick con ordine immediato.
#[derive(Clone, Debug)]
pub struct SpawnCmd {
    pub team: u8,
    pub kind: UnitKind,
    pub pos: Vec3,
    pub attack: bool,
}

/// Onde scriptate per tick, ordinate. Solo negli scenari che le usano.
#[derive(Resource, Default)]
pub struct SpawnSchedule {
    pub tick: u64,
    pub queue: Vec<(u64, SpawnCmd)>,
}

fn spawner(
    mut commands: Commands,
    mut clock: ResMut<SpawnSchedule>,
    mut ids: ResMut<UnitIds>,
    scenario: Res<Scenario>,
) {
    clock.tick += 1;
    let mut i = 0;
    while i < clock.queue.len() {
        if clock.queue[i].0 > clock.tick {
            i += 1;
            continue;
        }
        let (_, cmd) = clock.queue.remove(i);
        let id = ids.allocate();
        let e = spawn_combat_unit(&mut commands, id, Team(cmd.team), cmd.kind, cmd.pos);
        if cmd.attack {
            let dest = scenario.attack_target(cmd.team as usize);
            crate::orders::queue_attack_move(&mut commands.entity(e), dest);
        } else {
            commands.entity(e).insert(UnitOrder::HoldPosition);
        }
    }
}

pub struct ScenarioDef {
    pub id: &'static str,
    pub description: &'static str,
    pub ticks: usize,
    pub setup: fn(&mut App),
    #[allow(clippy::type_complexity)]
    pub evaluate:
        fn(&mut App, &[super::league::MatchSample]) -> (Vec<CheckResult>, BTreeMap<String, f64>),
}

fn check(name: &'static str, pass: bool, detail: String) -> CheckResult {
    CheckResult { name, pass, detail }
}

fn alive_units(world: &mut World, team: u8) -> usize {
    world
        .query_filtered::<(&Team, &Health), With<Unit>>()
        .iter(world)
        .filter(|(t, h)| t.0 == team && h.current > 0.0)
        .count()
}

fn alive_buildings(world: &mut World, team: u8) -> usize {
    world
        .query_filtered::<(&Team, &Health), With<Building>>()
        .iter(world)
        .filter(|(t, h)| t.0 == team && h.current > 0.0)
        .count()
}

fn base_hp_fraction(world: &mut World, team: u8) -> f64 {
    let mut cur = 0.0;
    let mut max = 0.0;
    for (t, h) in world
        .query_filtered::<(&Team, &Health), With<Building>>()
        .iter(world)
    {
        if t.0 == team {
            cur += h.current as f64;
            max += h.max as f64;
        }
    }
    if max <= 0.0 {
        1.0
    } else {
        (cur / max).clamp(0.0, 1.0)
    }
}

fn commander_alive(world: &mut World, team: u8) -> bool {
    world
        .query_filtered::<(&Team, &Health), (With<Unit>, With<crate::units::Commander>)>()
        .iter(world)
        .any(|(t, h)| t.0 == team && h.current > 0.0)
}

fn fog_honest(world: &mut World, team: u8) -> (bool, String) {
    let mut bad = 0;
    for (t, o) in world
        .query_filtered::<(&Team, &UnitOrder), With<Unit>>()
        .iter(world)
    {
        if t.0 != team {
            continue;
        }
        if let UnitOrder::Attack { target } = o {
            let pos = world.get::<Transform>(*target).map(|t| t.translation);
            let map = world.get_resource::<crate::fog::VisibilityMap>();
            if let (Some(p), Some(m)) = (pos, map)
                && !m.visible(team, p)
            {
                bad += 1;
            }
        }
    }
    (bad == 0, format!("blind attacks={bad}"))
}

fn no_t2_without_labt2(world: &mut World, team: u8) -> (bool, String) {
    let labt2 = world
        .query_filtered::<(&Team, &BuildingKind, &Health), With<Building>>()
        .iter(world)
        .filter(|(t, k, h)| t.0 == team && **k == BuildingKind::LabT2 && h.current > 0.0)
        .count();
    let t2 = world
        .query_filtered::<(&Team, &UnitKind, &Health), With<Unit>>()
        .iter(world)
        .filter(|(t, k, h)| {
            t.0 == team
                && h.current > 0.0
                && matches!(**k, UnitKind::HeavyTank2 | UnitKind::Artillery2)
        })
        .count();
    (labt2 > 0 || t2 == 0, format!("labt2={labt2} t2units={t2}"))
}

fn nav_clean(world: &mut World) -> (bool, String) {
    let failed = world
        .resource::<crate::navigation::NavigationStats>()
        .failed;
    (failed == 0, format!("failed={failed}"))
}

// ---------------------------------------------------------------------------
// Scenari.
// ---------------------------------------------------------------------------

fn setup_empty(_app: &mut App) {}

fn eval_eco_race(
    app: &mut App,
    series: &[super::league::MatchSample],
) -> (Vec<CheckResult>, BTreeMap<String, f64>) {
    let world = app.world_mut();
    let mut checks = Vec::new();
    let income_at = |tick: usize| -> [f64; 2] {
        series
            .iter()
            .rev()
            .find(|s| s.tick <= tick)
            .map(|s| s.blue.income)
            .unwrap_or([0.0, 0.0])
    };
    let m = BTreeMap::from([
        ("income_metal_120s".to_owned(), income_at(7200)[0]),
        ("income_metal_180s".to_owned(), income_at(10800)[0]),
        (
            "units_end".to_owned(),
            series.last().map(|s| s.blue.units as f64).unwrap_or(0.0),
        ),
        (
            "buildings_end".to_owned(),
            series
                .last()
                .map(|s| s.blue.buildings as f64)
                .unwrap_or(0.0),
        ),
    ]);
    checks.push(check(
        "eco-factory-online",
        alive_buildings(world, 0) >= 1,
        format!("buildings={}", alive_buildings(world, 0)),
    ));
    checks.push(check(
        "eco-income-flows",
        m["income_metal_180s"] > 2.0,
        format!("metal@180s={:.1}/s", m["income_metal_180s"]),
    ));
    (checks, m)
}

fn setup_hold_base(app: &mut App) {
    // Torretta prebuilt a difesa + 3 ondate da est.
    let world = app.world_mut();
    let scenario = *world.resource::<Scenario>();
    let grid = world.resource::<crate::navigation::NavGrid>().clone();
    let base = scenario.center(0);
    let spot = grid.clear_point_for(base + Vec3::new(12.0, 0.0, 12.0), 2.0);
    spawn_building(
        &mut world.commands(),
        Team(0),
        BuildingKind::Turret,
        spot,
        true,
    );
    world.flush();
    let foe = scenario.center(1);
    let dir = (foe - base).normalize_or_zero();
    let mut queue = Vec::new();
    for (wave, at) in [600u64, 2400u64, 4200u64].into_iter().enumerate() {
        for i in 0..3 {
            let side = if i == 0 {
                Vec3::ZERO
            } else {
                Vec3::new(0.0, 0.0, if i == 1 { 8.0 } else { -8.0 })
            };
            queue.push((
                at,
                SpawnCmd {
                    team: 1,
                    kind: UnitKind::HeavyTank,
                    pos: base + dir * 150.0 + side,
                    attack: true,
                },
            ));
        }
        let _ = wave;
    }
    queue.sort_by_key(|(t, _)| *t);
    world.insert_resource(SpawnSchedule { tick: 0, queue });
    world.flush();
    app.add_systems(Update, spawner);
}

fn eval_hold_base(
    app: &mut App,
    _series: &[super::league::MatchSample],
) -> (Vec<CheckResult>, BTreeMap<String, f64>) {
    let world = app.world_mut();
    let hp = base_hp_fraction(world, 0);
    let cmd = commander_alive(world, 0);
    let m = BTreeMap::from([
        ("base_hp_pct".to_owned(), hp * 100.0),
        ("commander_alive".to_owned(), if cmd { 1.0 } else { 0.0 }),
    ]);
    let mut checks = vec![check(
        "hold-commander-alive",
        cmd,
        "commander must survive scripted waves".to_owned(),
    )];
    checks.push(check(
        "hold-base-standing",
        hp > 0.3,
        format!("base hp={:.0}%", hp * 100.0),
    ));
    (checks, m)
}

fn eval_scout_blitz(
    app: &mut App,
    series: &[super::league::MatchSample],
) -> (Vec<CheckResult>, BTreeMap<String, f64>) {
    let world = app.world_mut();
    let explored = series
        .last()
        .map(|s| s.blue.explored_pct * 100.0)
        .unwrap_or(0.0);
    let m = BTreeMap::from([("explored_pct_120s".to_owned(), explored)]);
    let (honest, detail) = fog_honest(world, 0);
    let mut checks = vec![
        check("scout-fog-honest", honest, detail),
        check(
            "scout-explored",
            explored > 1.0,
            format!("explored={explored:.1}%"),
        ),
    ];
    let (clean, detail) = nav_clean(world);
    checks.push(check("scout-nav-clean", clean, detail));
    (checks, m)
}

fn setup_micro_duel(app: &mut App) {
    // 6v6 simmetrici fronte a fronte, ordini immediati, niente cervelli.
    let ids: Vec<u32> = (0..12)
        .map(|_| app.world_mut().resource_mut::<UnitIds>().allocate())
        .collect();
    {
        let world = app.world_mut();
        let mut cmds = world.commands();
        for side in [0u8, 1u8] {
            let sign = if side == 0 { -1.0 } else { 1.0 };
            for i in 0..6 {
                let id = ids[(side as usize) * 6 + i];
                let pos = Vec3::ZERO + Vec3::new(sign * 20.0, 0.8, (i as f32 - 2.5) * 3.0);
                let e = spawn_combat_unit(&mut cmds, id, Team(side), UnitKind::HeavyTank, pos);
                let dest = Vec3::ZERO - Vec3::new(sign * 20.0, 0.0, 0.0);
                crate::orders::queue_attack_move(&mut cmds.entity(e), dest);
            }
        }
    }
    app.world_mut().flush();
}

fn eval_micro_duel(
    app: &mut App,
    _series: &[super::league::MatchSample],
) -> (Vec<CheckResult>, BTreeMap<String, f64>) {
    let world = app.world_mut();
    let b0 = alive_units(world, 0);
    let b1 = alive_units(world, 1);
    let kills = 12usize.saturating_sub(b0 + b1);
    let m = BTreeMap::from([
        ("blue_survivors".to_owned(), b0 as f64),
        ("red_survivors".to_owned(), b1 as f64),
        ("kills".to_owned(), kills as f64),
    ]);
    let checks = vec![check(
        "duel-resolves",
        kills > 0,
        format!("kills={kills} survivors={b0}v{b1}"),
    )];
    (checks, m)
}

fn setup_siege(app: &mut App) {
    // Base blu: torretta + muretto; rossi: 6 heavy + 2 arty in AttackMove.
    let world = app.world_mut();
    let scenario = *world.resource::<Scenario>();
    let grid = world.resource::<crate::navigation::NavGrid>().clone();
    let base = scenario.center(0);
    let foe = scenario.center(1);
    let dir = (foe - base).normalize_or_zero();
    let tpos = grid.clear_point_for(base + Vec3::new(10.0, 0.0, 10.0), 2.0);
    spawn_building(
        &mut world.commands(),
        Team(0),
        BuildingKind::Turret,
        tpos,
        true,
    );
    let side = Vec3::new(-dir.z, 0.0, dir.x);
    for i in -1..=1 {
        let wp = grid.clear_point_for(tpos + dir * 10.0 + side * (i as f32 * 2.0), 1.0);
        spawn_building(&mut world.commands(), Team(0), BuildingKind::Wall, wp, true);
    }
    world.flush();
    // Forza d'assalto schierata a ~150m dalla base (una marcia da ~25s):
    // spawnarla all'angolo nemico (735m) non arriverebbe mai nel cap.
    let ids: Vec<u32> = (0..8)
        .map(|_| app.world_mut().resource_mut::<UnitIds>().allocate())
        .collect();
    {
        let world = app.world_mut();
        let mut cmds = world.commands();
        for (i, id) in ids.into_iter().enumerate() {
            let kind = if i < 6 {
                UnitKind::HeavyTank
            } else {
                UnitKind::Artillery
            };
            let pos = base + dir * 150.0 + side * ((i as f32 - 3.5) * 4.0);
            let e = spawn_combat_unit(&mut cmds, id, Team(1), kind, pos.with_y(0.8));
            crate::orders::queue_attack_move(&mut cmds.entity(e), base);
        }
    }
    app.world_mut().flush();
}

fn eval_siege(
    app: &mut App,
    _series: &[super::league::MatchSample],
) -> (Vec<CheckResult>, BTreeMap<String, f64>) {
    let world = app.world_mut();
    let turrets = world
        .query_filtered::<(&Team, &BuildingKind, &Health), With<Building>>()
        .iter(world)
        .filter(|(t, k, h)| t.0 == 0 && **k == BuildingKind::Turret && h.current > 0.0)
        .count();
    let attackers_left = alive_units(world, 1);
    let m = BTreeMap::from([
        ("turrets_left".to_owned(), turrets as f64),
        ("attackers_left".to_owned(), attackers_left as f64),
    ]);
    let checks = vec![check(
        "siege-turret-falls",
        turrets == 0,
        format!("turrets left={turrets} attackers left={attackers_left}"),
    )];
    (checks, m)
}

fn setup_lance_hold(app: &mut App) {
    // Scala gittate in scena: Lance blu (28m) vs 2 Rocket rossi (26m).
    // I rocket devono entrare nei 28m per sparare e la Lance li vede prima:
    // la banda 26<28 esiste apposta. Niente cervelli, solo ordini.
    let world = app.world_mut();
    let scenario = *world.resource::<Scenario>();
    let grid = world.resource::<crate::navigation::NavGrid>().clone();
    let base = scenario.center(0);
    let foe = scenario.center(1);
    let dir = (foe - base).normalize_or_zero();
    let tpos = grid.clear_point_for(base + Vec3::new(10.0, 0.0, 10.0), 2.0);
    spawn_building(
        &mut world.commands(),
        Team(0),
        BuildingKind::Lance,
        tpos,
        true,
    );
    world.flush();
    let ids: Vec<u32> = (0..2)
        .map(|_| app.world_mut().resource_mut::<UnitIds>().allocate())
        .collect();
    {
        let world = app.world_mut();
        let mut cmds = world.commands();
        for (i, id) in ids.into_iter().enumerate() {
            let pos = base + dir * 150.0 + Vec3::new(0.0, 0.0, (i as f32 - 0.5) * 6.0);
            let e = spawn_combat_unit(
                &mut cmds,
                id,
                Team(1),
                UnitKind::RocketTank,
                pos.with_y(0.8),
            );
            crate::orders::queue_attack_move(&mut cmds.entity(e), base);
        }
    }
    app.world_mut().flush();
}

fn eval_lance_hold(
    app: &mut App,
    _series: &[super::league::MatchSample],
) -> (Vec<CheckResult>, BTreeMap<String, f64>) {
    let world = app.world_mut();
    let lances = world
        .query_filtered::<(&Team, &BuildingKind, &Health), With<Building>>()
        .iter(world)
        .filter(|(t, k, h)| t.0 == 0 && **k == BuildingKind::Lance && h.current > 0.0)
        .count();
    let attackers_left = alive_units(world, 1);
    let m = BTreeMap::from([
        ("lances_left".to_owned(), lances as f64),
        ("attackers_left".to_owned(), attackers_left as f64),
    ]);
    // La Lance vede prima (28 vs 26) e regge: deve restare in piedi e aver
    // morso gli assalitori (2 rocket non bastano a seppellirla).
    let checks = vec![
        check("lance-holds", lances == 1, format!("lances left={lances}")),
        check(
            "lance-bites-back",
            attackers_left < 2,
            format!("attackers left={attackers_left}"),
        ),
    ];
    (checks, m)
}

fn eval_no_cheat(
    app: &mut App,
    _series: &[super::league::MatchSample],
) -> (Vec<CheckResult>, BTreeMap<String, f64>) {
    let world = app.world_mut();
    let mut checks = Vec::new();
    for team in [0u8, 1u8] {
        let (honest, detail) = fog_honest(world, team);
        checks.push(check(
            if team == 0 {
                "nocheat-blue-honest"
            } else {
                "nocheat-red-honest"
            },
            honest,
            detail,
        ));
        let (ok, detail) = no_t2_without_labt2(world, team);
        checks.push(check(
            if team == 0 {
                "nocheat-blue-tier"
            } else {
                "nocheat-red-tier"
            },
            ok,
            detail,
        ));
    }
    let (clean, detail) = nav_clean(world);
    checks.push(check("nocheat-nav-clean", clean, detail));
    (checks, BTreeMap::new())
}

fn setup_counter_comp(app: &mut App) {
    // 0.0.21 — counter-comp: 4 Heavy blu vs 6 Light rossi, ordini speculari.
    // La tabella counter (Heavy 1.15 vs Light, Light 0.9 vs Heavy) deve dare
    // vantaggio blu a parità di micro nullo. Scriptato puro, niente cervelli.
    let ids: Vec<u32> = (0..10)
        .map(|_| app.world_mut().resource_mut::<UnitIds>().allocate())
        .collect();
    {
        let world = app.world_mut();
        let mut cmds = world.commands();
        for (i, id) in ids[..4].iter().enumerate() {
            let pos = Vec3::ZERO + Vec3::new(-20.0, 0.8, (i as f32 - 1.5) * 3.0);
            let e = spawn_combat_unit(&mut cmds, *id, Team(0), UnitKind::HeavyTank, pos);
            crate::orders::queue_attack_move(
                &mut cmds.entity(e),
                Vec3::ZERO + Vec3::new(20.0, 0.0, 0.0),
            );
        }
        for (i, id) in ids[4..].iter().enumerate() {
            let pos = Vec3::ZERO + Vec3::new(20.0, 0.8, (i as f32 - 2.5) * 3.0);
            let e = spawn_combat_unit(&mut cmds, *id, Team(1), UnitKind::LightTank, pos);
            crate::orders::queue_attack_move(
                &mut cmds.entity(e),
                Vec3::ZERO + Vec3::new(-20.0, 0.0, 0.0),
            );
        }
    }
    app.world_mut().flush();
}

fn eval_counter_comp(
    app: &mut App,
    _series: &[super::league::MatchSample],
) -> (Vec<CheckResult>, BTreeMap<String, f64>) {
    let world = app.world_mut();
    let b0 = alive_units(world, 0);
    let b1 = alive_units(world, 1);
    let kills = 10usize.saturating_sub(b0 + b1);
    let m = BTreeMap::from([
        ("blue_survivors".to_owned(), b0 as f64),
        ("red_survivors".to_owned(), b1 as f64),
        ("kills".to_owned(), kills as f64),
    ]);
    // Tabella counter motivata (dato, non logica): Heavy batte Light.
    let table_ok =
        (crate::economy::balance::counter_mult(UnitKind::HeavyTank, UnitKind::LightTank) - 1.15)
            .abs()
            < 1e-6
            && (crate::economy::balance::counter_mult(UnitKind::LightTank, UnitKind::HeavyTank)
                - 0.9)
                .abs()
                < 1e-6;
    let mut checks = vec![
        check(
            "counter-table-shaped",
            table_ok,
            "Heavy 1.15 vs Light, Light 0.9 vs Heavy".to_owned(),
        ),
        check(
            "counter-duel-resolves",
            kills > 0,
            format!("kills={kills} survivors={b0}v{b1}"),
        ),
    ];
    // Il counter deve dare vantaggio blu (più sopravvissuti o almeno pari con
    // kill): gate morbida ma direzionale.
    checks.push(check(
        "counter-blue-holds-edge",
        b0 >= b1,
        format!("blue={b0} red={b1}"),
    ));
    (checks, m)
}

fn setup_commander_snipe(app: &mut App) {
    // 0.0.21 — commander-snipe: turtle blu + torretta prebuilt vs 4 Heavy
    // rossi in AttackMove sulla base (caccia al capitale). Il Commander blu
    // deve sopravvivere (capitale protetto 0.0.20: retreat + screen).
    let world = app.world_mut();
    let scenario = *world.resource::<Scenario>();
    let grid = world.resource::<crate::navigation::NavGrid>().clone();
    let base = scenario.center(0);
    let foe = scenario.center(1);
    let dir = (foe - base).normalize_or_zero();
    let tpos = grid.clear_point_for(base + Vec3::new(10.0, 0.0, 10.0), 2.0);
    spawn_building(
        &mut world.commands(),
        Team(0),
        BuildingKind::Turret,
        tpos,
        true,
    );
    world.flush();
    let ids: Vec<u32> = (0..4)
        .map(|_| app.world_mut().resource_mut::<UnitIds>().allocate())
        .collect();
    {
        let world = app.world_mut();
        let mut cmds = world.commands();
        let side = Vec3::new(-dir.z, 0.0, dir.x);
        for (i, id) in ids.into_iter().enumerate() {
            let pos = base + dir * 150.0 + side * ((i as f32 - 1.5) * 4.0);
            let e = spawn_combat_unit(&mut cmds, id, Team(1), UnitKind::HeavyTank, pos.with_y(0.8));
            crate::orders::queue_attack_move(&mut cmds.entity(e), base);
        }
    }
    app.world_mut().flush();
}

fn eval_commander_snipe(
    app: &mut App,
    _series: &[super::league::MatchSample],
) -> (Vec<CheckResult>, BTreeMap<String, f64>) {
    let world = app.world_mut();
    let cmd = commander_alive(world, 0);
    let hp = base_hp_fraction(world, 0);
    let (honest, honest_detail) = fog_honest(world, 0);
    let (clean, clean_detail) = nav_clean(world);
    let m = BTreeMap::from([
        ("base_hp_pct".to_owned(), hp * 100.0),
        ("commander_alive".to_owned(), if cmd { 1.0 } else { 0.0 }),
    ]);
    let checks = vec![
        check(
            "snipe-commander-alive",
            cmd,
            "commander must survive focused rush".to_owned(),
        ),
        check("snipe-fog-honest", honest, honest_detail),
        check("snipe-nav-clean", clean, clean_detail),
    ];
    (checks, m)
}

pub fn all_scenarios() -> Vec<ScenarioDef> {
    vec![
        ScenarioDef {
            id: "eco-race",
            description: "turtle vs null 180s: eco e milestone",
            ticks: 10_800,
            setup: setup_empty,
            evaluate: eval_eco_race,
        },
        ScenarioDef {
            id: "hold-the-base",
            description: "turtle + torretta vs 3 ondate scriptate",
            ticks: 6000,
            setup: setup_hold_base,
            evaluate: eval_hold_base,
        },
        ScenarioDef {
            id: "scout-blitz",
            description: "rusher vs null 120s: % esplorato",
            ticks: 7200,
            setup: setup_empty,
            evaluate: eval_scout_blitz,
        },
        ScenarioDef {
            id: "micro-duel",
            description: "6v6 heavy simmetrici, ordini scriptati",
            ticks: 3600,
            setup: setup_micro_duel,
            evaluate: eval_micro_duel,
        },
        ScenarioDef {
            id: "siege",
            description: "8 rossi vs torretta+muretto blu",
            ticks: 5400,
            setup: setup_siege,
            evaluate: eval_siege,
        },
        ScenarioDef {
            id: "no-cheat",
            description: "turtle-vs-rusher 1800 tick: fog + tier onesti",
            ticks: 1800,
            setup: setup_empty,
            evaluate: eval_no_cheat,
        },
        ScenarioDef {
            id: "counter-comp",
            description: "4 heavy blu vs 6 light rossi: vantaggio counter",
            ticks: 3600,
            setup: setup_counter_comp,
            evaluate: eval_counter_comp,
        },
        ScenarioDef {
            id: "commander-snipe",
            description: "turtle+torretta vs 4 heavy snipe: capitale vivo",
            ticks: 3600,
            setup: setup_commander_snipe,
            evaluate: eval_commander_snipe,
        },
        ScenarioDef {
            id: "lance-hold",
            description: "Lance blu (28m) vs 2 rocket rossi (26m): banda scala",
            ticks: 3600,
            setup: setup_lance_hold,
            evaluate: eval_lance_hold,
        },
    ]
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ScenarioSummary {
    pub id: String,
    pub ticks: usize,
    pub checks: Vec<CheckResultSummary>,
    pub metrics: BTreeMap<String, f64>,
    pub checksum: u64,
    pub deterministic: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CheckResultSummary {
    pub name: String,
    pub pass: bool,
    pub detail: String,
}

/// Quale cervello gioca ogni scenario (None = nessuno, scriptato).
fn brains_for(id: &str) -> (Option<Personality>, Option<Personality>) {
    use super::strategy::Personality as P;
    match id {
        "eco-race" => (Some(P::TURTLE), None),
        "hold-the-base" => (Some(P::TURTLE), None),
        "scout-blitz" => (Some(P::RUSHER), None),
        "micro-duel" => (None, None),
        "siege" => (None, None),
        "no-cheat" => (Some(P::TURTLE), Some(P::RUSHER)),
        "counter-comp" => (None, None),
        "lance-hold" => (None, None),
        "commander-snipe" => (Some(P::TURTLE), None),
        _ => (None, None),
    }
}

/// Batteria L1: ogni scenario x2 repeat (determinismo), serie ogni 600 tick,
/// report ai-scenarios.json. Exit nonzero se un gate cade.
pub fn run_battery(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let mut summaries = Vec::new();
    let mut all_pass = true;
    for def in all_scenarios() {
        println!("scenario {} — {}", def.id, def.description);
        let (blue, red) = brains_for(def.id);
        let mut checksums = Vec::new();
        let mut last = None;
        for repeat in 0..2 {
            let mut app = super::league::build_match_app(blue, red, 0);
            (def.setup)(&mut app);
            let mut series = vec![super::league::sample_teams(app.world_mut(), 0)];
            for tick in 1..=def.ticks {
                app.update();
                if tick % 600 == 0 {
                    series.push(super::league::sample_teams(app.world_mut(), tick));
                }
            }
            let checksum = super::director::state_checksum(app.world_mut());
            let (checks, metrics) = (def.evaluate)(&mut app, &series);
            let pass = checks.iter().all(|c| c.pass);
            all_pass &= pass;
            println!(
                "scenario {} repeat={} ticks={} checksum={:#x} {}",
                def.id,
                repeat + 1,
                def.ticks,
                checksum,
                if pass { "PASS" } else { "FAIL" },
            );
            for c in &checks {
                println!(
                    "  [{}] {} — {}",
                    if c.pass { "ok" } else { "FAIL" },
                    c.name,
                    c.detail
                );
            }
            for (k, v) in &metrics {
                println!("    metric {k}={v:.2}");
            }
            checksums.push(checksum);
            last = Some((checks, metrics, checksum));
        }
        let (checks, metrics, checksum) = last.unwrap();
        let deterministic = checksums.windows(2).all(|w| w[0] == w[1]);
        all_pass &= deterministic;
        summaries.push(ScenarioSummary {
            id: def.id.to_owned(),
            ticks: def.ticks,
            checks: checks
                .into_iter()
                .map(|c| CheckResultSummary {
                    name: c.name.to_owned(),
                    pass: c.pass,
                    detail: c.detail,
                })
                .collect(),
            metrics,
            checksum,
            deterministic,
        });
    }
    let report = serde_json::json!({
        "kind": "ai-scenarios",
        "version": env!("CARGO_PKG_VERSION"),
        "scenarios": summaries,
        "all_pass": all_pass,
    });
    let dir = match &config.output {
        Some(d) => {
            std::fs::create_dir_all(d)?;
            d.clone()
        }
        None => {
            let d = std::path::PathBuf::from("benchmark-results/ai-scenarios");
            std::fs::create_dir_all(&d)?;
            d
        }
    };
    std::fs::write(
        dir.join("ai-scenarios.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!("Scenarios report: {}/ai-scenarios.json", dir.display());
    if !all_pass {
        return Err("AI scenario battery failed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_has_nine_unique_scenarios() {
        let defs = all_scenarios();
        assert_eq!(defs.len(), 9);
        let mut ids: Vec<&str> = defs.iter().map(|d| d.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 9);
        for def in &defs {
            assert!(def.ticks >= 1800, "{} troppo corto", def.id);
            assert!(!def.description.is_empty());
        }
    }

    #[test]
    fn every_scenario_has_a_brain_or_a_script() {
        // micro-duel, siege e counter-comp sono scriptati puri (niente
        // cervelli), gli altri hanno almeno un cervello che gioca.
        for def in all_scenarios() {
            let (blue, red) = brains_for(def.id);
            if def.id == "micro-duel"
                || def.id == "siege"
                || def.id == "counter-comp"
                || def.id == "lance-hold"
            {
                assert!(blue.is_none() && red.is_none(), "{}", def.id);
            } else {
                assert!(blue.is_some() || red.is_some(), "{}", def.id);
            }
        }
    }
}
