//! Harness headless `ai-test`: Playground reale (Commander per team, mappa
//! seedata) + AI sul team rosso. Stesso stile di `economy/benchmark.rs`:
//! niente report-suite legacy, JSON proprio + assert funzionali e balance.

use crate::{
    benchmark::cli::Config,
    combat::CombatPlugin,
    economy::{Economy, EconomyPlugin},
    fog::FogPlugin,
    movement::MovementPlugin,
    navigation::{NavGrid, NavigationPlugin, NavigationStats},
    production::ProductionPlugin,
    scenario::Scenario,
    spatial::SpatialPlugin,
    structures::StructuresPlugin,
    units::{Team, Unit, UnitPlugin},
};
use bevy::{prelude::*, time::TimeUpdateStrategy};
use std::time::{Duration, Instant};

use super::{
    AiConfig, AiMode,
    director::{self, AiTestReport, CheckResult},
};

fn build_app(personality: &str, team: u8) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Scenario::Playground)
        .insert_resource(AiConfig::single(
            team,
            super::strategy::Personality::from_name(personality),
            AiMode::Test,
        ))
        .add_plugins((
            NavigationPlugin,
            SpatialPlugin,
            UnitPlugin { visuals: false },
            MovementPlugin,
            CombatPlugin,
            EconomyPlugin,
            StructuresPlugin { visuals: false },
            ProductionPlugin,
            FogPlugin { render: false },
            super::AiPlugin,
        ))
        .add_plugins(bevy::asset::AssetPlugin::default())
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>();
    app.finish();
    app.cleanup();
    // Attiva fog (i primi 0.25s sono open): come combat_fog_app nei test.
    for _ in 0..20 {
        app.update();
    }
    *app.world_mut().resource_mut::<NavigationStats>() = NavigationStats::default();
    app
}

fn collect_report(app: &mut App, team: u8, ticks: usize, _samples: Vec<f64>) -> AiTestReport {
    let world = app.world_mut();
    let mut functional = director::evaluate_functional(world, team, ticks);
    let balance = director::evaluate_balance(world, team);
    functional.extend(balance);
    let checksum = director::state_checksum(world);
    let units_team = world
        .query_filtered::<(&Team, &crate::combat::Health), With<Unit>>()
        .iter(world)
        .filter(|(t, h)| t.0 == team && h.current > 0.0)
        .count();
    let buildings_team = world
        .query_filtered::<&Team, With<crate::structures::Building>>()
        .iter(world)
        .filter(|t| t.0 == team)
        .count();
    let (stocks, income) = world
        .resource::<Economy>()
        .0
        .get(&team)
        .map(|a| (a.stock, a.income))
        .unwrap_or(([0.0, 0.0], [0.0, 0.0]));
    let nav_failed = world.resource::<NavigationStats>().failed;
    let ai_state = world.resource::<super::AiState>();
    let (orders_issued, builds_done, enqueues_done) = ai_state.totals();
    AiTestReport {
        checks: functional,
        ticks,
        checksum,
        units_team,
        buildings_team,
        stocks,
        income,
        orders_issued,
        builds_done,
        enqueues_done,
        nav_failed,
        kills: 0,
    }
}

pub fn run(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    // 0.0.21 — strict come league: personalità ignote sono errore, mai
    // fallback silenzioso a turtle (allinea from_name a resolve_brain).
    super::strategy::Personality::try_from_name(&config.ai_personality)
        .map_err(|e| format!("{e}; use one of: turtle, rusher, eco-only, rush-scripted"))?;
    let team = config.ai_team;
    // Catena completa Metal->Solar->Factory->Tank richiede ~80-100s sim:
    // 7200 tick Update (120s) di default. Se l'utente passa --ticks esplicito,
    // rispettalo (run brevi testano solo i primi stadi).
    let ticks = if config.ticks_overridden {
        config.ticks.max(60)
    } else {
        7200
    };
    // Due repeat per determinismo (stesso pattern economy benchmark).
    let mut reports: Vec<AiTestReport> = Vec::new();
    let mut all_samples: Vec<Vec<f64>> = Vec::new();
    for repeat in 0..config.repeats.clamp(2, 3) {
        let mut app = build_app(&config.ai_personality, team);
        let mut samples = Vec::with_capacity(ticks);
        for _ in 0..ticks {
            let now = Instant::now();
            app.update();
            samples.push(now.elapsed().as_secs_f64() * 1000.0);
        }
        let report = collect_report(&mut app, team, ticks, samples.clone());
        all_samples.push(samples);
        println!(
            "AI-test repeat={} personality={} ticks={} units={} buildings={} stocks=[{:.0},{:.0}] orders={} builds={} enqueues={} nav_failed={} checksum={:#x} {}",
            repeat + 1,
            config.ai_personality,
            ticks,
            report.units_team,
            report.buildings_team,
            report.stocks[0],
            report.stocks[1],
            report.orders_issued,
            report.builds_done,
            report.enqueues_done,
            report.nav_failed,
            report.checksum,
            if report.all_pass() { "PASS" } else { "FAIL" },
        );
        for c in &report.checks {
            println!(
                "  [{}] {} — {}",
                if c.pass { "ok" } else { "FAIL" },
                c.name,
                c.detail
            );
        }
        reports.push(report);
    }
    // Determinismo: checksum identici tra repeat.
    let deterministic = reports.windows(2).all(|w| w[0].checksum == w[1].checksum);
    if !deterministic {
        return Err("AI-test repeats diverged (nondeterminism)".into());
    }
    let all_pass = reports.iter().all(|r| r.all_pass());
    // Timing descrittivo.
    let flat: Vec<f64> = all_samples.into_iter().flatten().collect();
    let mean = flat.iter().sum::<f64>() / flat.len().max(1) as f64;
    let mut sorted = flat.clone();
    sorted.sort_by(f64::total_cmp);
    let p95 = sorted
        .get((sorted.len() as f64 * 0.95).ceil() as usize - 1)
        .copied()
        .unwrap_or(0.0);
    let first = &reports[0];
    let checks_json: Vec<_> = first
        .checks
        .iter()
        .map(|c: &CheckResult| {
            serde_json::json!({"name": c.name, "pass": c.pass, "detail": c.detail})
        })
        .collect();
    let report_json = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "workload": "ai-test",
        "scenario": "Playground (commander start, seeded map)",
        "personality": config.ai_personality,
        "team": team,
        "ticks": ticks,
        "repeats": reports.len(),
        "deterministic": deterministic,
        "all_pass": all_pass,
        "mean_ms": mean,
        "p95_ms": p95,
        "checksum": format!("{:#x}", first.checksum),
        "units_team": first.units_team,
        "buildings_team": first.buildings_team,
        "stocks": first.stocks,
        "income": first.income,
        "orders_issued": first.orders_issued,
        "builds_done": first.builds_done,
        "enqueues_done": first.enqueues_done,
        "nav_failed": first.nav_failed,
        "checks": checks_json,
    });
    if let Some(directory) = &config.output {
        std::fs::create_dir_all(directory)?;
        std::fs::write(
            directory.join("ai-test.json"),
            serde_json::to_string_pretty(&report_json)?,
        )?;
        println!("AI-test report: {}/ai-test.json", directory.display());
    } else {
        println!("AI-test summary: {report_json}");
    }
    // Grid resource usata (evita warning unused import NavGrid se il setup cambia).
    let _ = std::mem::size_of::<NavGrid>();
    if !all_pass {
        return Err("AI-test functional/balance checks failed".into());
    }
    Ok(())
}
