//! AI RTS dinamica & future-proof: snapshot onesto (fog) -> utility strategy
//! data-driven -> executor con budget. Stessi attuatori del player
//! (`queue_*`, `spawn_building`, `Factory::enqueue`), quindi ogni run è sia
//! auto-test funzionale che avversario skirmish.
//!
//! Uso:
//! - Headless test: `--benchmark --headless --workload ai-test`
//!   (Playground reale con Commander, mappa seedata, fog attivo).
//! - Grafico 1v1 AI-vs-AI: `cargo run -- --ai both`
//!   (blu=rusher vs rosso=turtle di default, overlay per entrambi).
//! - Grafico vs AI: `cargo run -- --ai skirmish` (AI sul team 1, tu giochi il blu).
//! - Personalità: `--ai-personality turtle --ai-personality2 rusher`.

pub mod combat;
pub mod debug;
pub mod difficulty;
pub mod director;
pub mod executor;
pub mod harness;
pub mod league;
pub mod memory;
pub mod opponent;
pub mod planner;
pub mod scenarios;
pub mod scout;
pub mod snapshot;
pub mod strategy;
pub mod threat;
pub mod utility;

use crate::{
    combat::Health,
    movement::MovementSystems,
    orders::UnitOrder,
    production::Factory,
    structures::{Building, Construction},
    units::{Team, Unit, UnitKind},
};
use bevy::prelude::*;
use std::collections::BTreeMap;

pub use memory::EnemyMemory;
pub use snapshot::AiSnapshot;
pub use strategy::Personality;

/// Cadence: snapshot come il fog (4Hz), strategia lenta (1Hz) come da
/// GameAIPro (Utility a bassa frequenza, micro ad alta).
pub const SNAPSHOT_PERIOD: f32 = 0.25;

/// Match-relative snapshot cadence. It is a resource so `R` can reset both
/// the accumulator and the tick together with the rest of the AI state.
#[derive(Resource, Default, Debug)]
pub struct AiClock {
    pub(crate) accumulator: f32,
    pub(crate) tick: u64,
}
pub const STRATEGY_PERIOD: f32 = 1.0;
/// 0.0.20 — micro a 4Hz (solo Retreat/Focus/Hold/Screen), budget 2 ordini:
/// correzioni di rotta rapide senza churn del planner macro.
pub const MICRO_PERIOD: f32 = 0.25;
pub const MICRO_BUDGET: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AiMode {
    #[default]
    Off,
    /// Harness headless: assert funzionali + balance, deterministico.
    Test,
    /// Avversario in partita grafica (un team).
    Skirmish,
    /// Demo grafica 1v1: entrambi i team giocati dall'AI.
    Both,
}

/// Config di un singolo cervello (un team). `AiConfig` ne contiene una lista
/// ordinata per team: aggiungere un terzo team futuro = un push, zero rewrite.
#[derive(Clone, Debug, PartialEq)]
pub struct AiTeamConfig {
    pub team: u8,
    pub personality: Personality,
    pub max_orders_per_tick: usize,
    /// 0.0.21 — handicap puro sullo stesso cervello (solo freno, mai bonus).
    pub handicap: difficulty::Handicap,
}

impl AiTeamConfig {
    pub fn new(team: u8, personality: Personality) -> Self {
        Self {
            team,
            personality,
            max_orders_per_tick: 8,
            handicap: difficulty::Handicap::default(),
        }
    }

    pub fn with_handicap(
        team: u8,
        personality: Personality,
        handicap: difficulty::Handicap,
    ) -> Self {
        Self {
            team,
            personality,
            max_orders_per_tick: handicap.max_orders_per_tick.min(8),
            handicap,
        }
    }
}

#[derive(Resource, Clone, Debug, Default)]
pub struct AiConfig {
    pub teams: Vec<AiTeamConfig>,
    pub mode: AiMode,
}

impl AiConfig {
    pub fn single(team: u8, personality: Personality, mode: AiMode) -> Self {
        Self {
            teams: vec![AiTeamConfig::new(team, personality)],
            mode,
        }
    }

    pub fn versus(
        team0_personality: Personality,
        team1_personality: Personality,
        mode: AiMode,
    ) -> Self {
        Self {
            teams: vec![
                AiTeamConfig::new(0, team0_personality),
                AiTeamConfig::new(1, team1_personality),
            ],
            mode,
        }
    }

    /// Team ordinati per determinismo.
    pub fn sorted_teams(&self) -> Vec<AiTeamConfig> {
        let mut teams = self.teams.clone();
        teams.sort_by_key(|t| t.team);
        teams
    }

    #[allow(dead_code)]
    pub fn controls(&self, team: u8) -> bool {
        self.teams.iter().any(|t| t.team == team)
    }
}

/// Snapshot per team (chiave = team id). Ogni cervello legge SOLO il suo.
#[derive(Resource, Clone, Debug, Default)]
pub struct AiSnapshots(pub BTreeMap<u8, AiSnapshot>);

#[derive(Clone, Debug, Default)]
pub struct AiTeamStats {
    pub orders_issued: u64,
    pub builds_done: u64,
    pub enqueues_done: u64,
    /// Punto 2 — telemetria onde/micro: ondate lanciate (macro 1Hz) e ordini
    /// micro (4Hz, budget 2). `orders_issued` resta il totale per compat.
    pub waves_launched: u64,
    pub micro_orders: u64,
}

#[derive(Resource, Default, Debug)]
pub struct AiState {
    pub acc: f32,
    pub per_team: BTreeMap<u8, AiTeamStats>,
    /// 0.0.20 — ultima ondata lanciata per team (catch-up: vedi `decide`).
    pub last_wave: BTreeMap<u8, u64>,
    /// Punto 1 — win_prob al lancio di ogni ondata *contata* (una per periodo).
    /// Serie corta (<= periodi del match): calibrazione courage dai dati veri.
    pub fire_win_probs: BTreeMap<u8, Vec<f32>>,
}

/// Punto 1 — avanza il contatore onde: conta solo il passaggio a un nuovo
/// periodo (`wave_id > last`), mai le riemissioni `force_attack` dentro lo
/// stesso periodo (gonfiavano il contatore a centinaia). Puro.
pub fn note_wave_fired(last_wave_id: u64, tick: u64) -> (u64, bool) {
    let id = strategy::wave_id(tick);
    // `max` difensivo: se il tick snapshot mai andasse indietro (reset),
    // l'ondata non resta soppressa per un intero periodo.
    (id.max(last_wave_id), id > last_wave_id)
}

impl AiState {
    pub fn stats_mut(&mut self, team: u8) -> &mut AiTeamStats {
        self.per_team.entry(team).or_default()
    }

    /// Somme su tutti i team (telemetria harness single-team resta valida).
    pub fn totals(&self) -> (u64, u64, u64) {
        let mut orders = 0;
        let mut builds = 0;
        let mut enqueues = 0;
        for stats in self.per_team.values() {
            orders += stats.orders_issued;
            builds += stats.builds_done;
            enqueues += stats.enqueues_done;
        }
        (orders, builds, enqueues)
    }

    // Compat harness: totali su tutti i team.
    #[allow(dead_code)]
    pub fn orders_issued(&self) -> u64 {
        self.totals().0
    }

    #[allow(dead_code)]
    pub fn builds_done(&self) -> u64 {
        self.totals().1
    }

    #[allow(dead_code)]
    pub fn enqueues_done(&self) -> u64 {
        self.totals().2
    }
}

pub struct AiPlugin;
impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AiConfig>()
            .init_resource::<AiSnapshots>()
            .init_resource::<AiState>()
            .init_resource::<EnemyMemory>()
            .init_resource::<AiClock>()
            .add_systems(
                Update,
                (snapshot::refresh_snapshots, ai_tick, micro_tick)
                    .chain()
                    .after(MovementSystems)
                    .after(crate::fog::FogSystems)
                    .run_if(ai_active),
            );
    }
}

fn ai_active(config: Res<AiConfig>) -> bool {
    config.mode != AiMode::Off && !config.teams.is_empty()
}

/// Strategia (1Hz) + micro-esecuzione con budget, per OGNI team AI.
/// Legge SOLO il proprio AiSnapshot (mai query nemiche dirette):
/// l'onestà fog è strutturale. Loop in ordine di team = deterministico.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn ai_tick(
    mut commands: Commands,
    time: Res<Time>,
    config: Res<AiConfig>,
    snapshots: Res<AiSnapshots>,
    match_result: Option<Res<crate::game_over::MatchResult>>,
    scenario: Res<crate::scenario::Scenario>,
    grid: Res<crate::navigation::NavGrid>,
    mut state: ResMut<AiState>,
    mut debug: Option<ResMut<debug::AiDebugState>>,
    units: Query<(Entity, &Transform, &Team, &UnitKind, &UnitOrder, &Health), With<Unit>>,
    unit_bodies: Query<(&Transform, &crate::units::CollisionRadius), With<Unit>>,
    buildings: Query<
        (
            Entity,
            &Team,
            &crate::economy::balance::BuildingKind,
            &Transform,
            &Health,
        ),
        (With<Building>,),
    >,
    building_sites: Query<
        (
            Entity,
            &Team,
            &crate::economy::balance::BuildingKind,
            &Transform,
        ),
        (With<Building>, With<Construction>),
    >,
    builders: Query<(&Team, &Transform, &crate::units::Builder, &Health), With<Unit>>,
    mut factories: Query<
        (Entity, &Team, &Transform, &mut Factory),
        (With<Building>, Without<Construction>),
    >,
) {
    state.acc += time.delta_secs();
    if state.acc < STRATEGY_PERIOD {
        return;
    }
    state.acc = 0.0;
    // Partita finita: i cervelli tacciono (lo snapshot resta fresco).
    if match_result.is_some_and(|r| r.over) {
        return;
    }

    // Viste condivise (lette una volta, filtrate per team nel loop).
    let mut flat_buildings: Vec<(Team, crate::economy::balance::BuildingKind, Vec3, bool)> =
        Vec::new();
    for (e, team, kind, transform, _) in buildings.iter() {
        let site = building_sites.get(e).is_ok();
        flat_buildings.push((*team, *kind, transform.translation, site));
    }
    flat_buildings.sort_by_key(|(_, _, pos, _)| (pos.x.to_bits(), pos.z.to_bits()));

    let live_builders: Vec<(Team, Vec3, f32)> = builders
        .iter()
        .filter(|(_, _, _, hp)| hp.current > 0.0)
        .map(|(team, transform, builder, _)| (*team, transform.translation, builder.radius))
        .collect();

    let footprints: Vec<(Vec3, f32)> = unit_bodies
        .iter()
        .map(|(t, r)| (t.translation, r.0))
        .collect();

    let mut all_intent_labels: Vec<String> = Vec::new();

    for brain in config.sorted_teams() {
        let Some(snapshot) = snapshots.0.get(&brain.team) else {
            continue;
        };
        if snapshot.tick == 0 {
            continue;
        }

        // Rally default per factory complete senza rally (rinforzi autonomi).
        // Sempre validato+riparato: un rally murato bloccherebbe la factory
        // per sempre (vedi default_rally), come visto sul lato blu.
        {
            let target = scenario.attack_target(brain.team as usize);
            for (_, team, transform, mut factory) in factories.iter_mut() {
                if team.0 != brain.team || factory.rally.is_some() {
                    continue;
                }
                if let Some(rally) = strategy::default_rally(&grid, transform.translation, target) {
                    factory.rally = Some(rally);
                }
            }
        }

        // Code factory del team (entity + len + blocked + tier + code),
        // ordinate per determinismo.
        let mut factory_queues: Vec<strategy::FactoryView> = factories
            .iter()
            .filter(|(_, team, _, _)| team.0 == brain.team)
            .map(|(e, _, _, f)| strategy::FactoryView {
                entity: e,
                queue_len: f.queue.len(),
                blocked: f.blocked,
                tier: f.tier,
                queued: f.queue.iter().map(|job| job.kind).collect(),
            })
            .collect();
        factory_queues.sort_by_key(|v| v.entity.to_bits());

        // 0.0.21 — handicap periodo: macro più lenta (salta tick). Deterministico
        // su snapshot.tick (4Hz): macro_id = tick/4, corre ogni period_mult.
        // Catch-up onde invariato: il periodo perso resta dovuto.
        let macro_id = snapshot.tick / 4;
        let period = brain.handicap.strategy_period_mult.max(1.0) as u64;
        if period > 1 && macro_id % period != 0 {
            continue;
        }
        // 0.0.21 — handicap courage + APM sullo stesso cervello (solo freno).
        let mut pers = brain.personality;
        pers.courage = (pers.courage + brain.handicap.courage_malus).clamp(0.0, 1.2);
        let apm = brain
            .max_orders_per_tick
            .min(brain.handicap.max_orders_per_tick);
        let intents = strategy::decide(
            snapshot,
            &pers,
            *scenario,
            &factory_queues,
            state.last_wave.get(&brain.team).copied().unwrap_or(0),
        );
        all_intent_labels.extend(intents.iter().map(|i| format!("M{}:{i:?}", brain.team)));
        // 0.0.20 — catch-up onde: l'ondata lanciata consuma il periodo.
        // Punto 1 — conta solo il passaggio di periodo (mai le riemissioni).
        if intents
            .iter()
            .any(|i| matches!(i, strategy::AiIntent::AttackMoveGroup { .. }))
        {
            let prev = state.last_wave.get(&brain.team).copied().unwrap_or(0);
            let (next, counted) = note_wave_fired(prev, snapshot.tick);
            state.last_wave.insert(brain.team, next);
            if counted {
                state.stats_mut(brain.team).waves_launched += 1;
                state
                    .fire_win_probs
                    .entry(brain.team)
                    .or_default()
                    .push(strategy::win_prob_of(snapshot));
            }
        }
        if intents.is_empty() {
            continue;
        }

        // Truppe del team.
        let mut flat_units: Vec<(Entity, Vec3, UnitKind, UnitOrder)> = units
            .iter()
            .filter(|(_, _, team, _, _, hp)| team.0 == brain.team && hp.current > 0.0)
            .map(|(e, t, _, kind, order, _)| (e, t.translation, *kind, order.clone()))
            .collect();
        flat_units.sort_by_key(|(e, _, _, _)| e.to_bits());

        let (orders, builds, enqueue_targets) = executor::execute_movement_and_build(
            &mut commands,
            snapshot,
            &intents,
            brain.team,
            apm,
            &grid,
            *scenario,
            &flat_units,
            &flat_buildings,
            &live_builders,
            &footprints,
        );
        // Enqueue via query mutabile (ordinata per determinismo).
        let mut enqueues_done = 0;
        let mut sorted_targets = enqueue_targets;
        sorted_targets.sort_by_key(|(e, _)| e.to_bits());
        for (entity, kind) in sorted_targets {
            // Sicurezza: la factory deve appartenere allo stesso team.
            let owner_ok = factories
                .get(entity)
                .is_ok_and(|(_, team, _, _)| team.0 == brain.team);
            if !owner_ok {
                continue;
            }
            if let Ok((_, _, _, mut factory)) = factories.get_mut(entity)
                && factory.enqueue(kind)
            {
                enqueues_done += 1;
            }
        }
        let stats = state.stats_mut(brain.team);
        stats.orders_issued += orders as u64;
        stats.builds_done += builds as u64;
        stats.enqueues_done += enqueues_done as u64;
    }

    if let Some(debug) = debug.as_deref_mut() {
        debug.last_intents = all_intent_labels;
    }
}

/// 0.0.20 — micro a 4Hz: SOLO Retreat/FocusFire/HoldAtMaxRange/Screen
/// (`strategy::decide_micro`), budget 2. La macro a 1Hz resta padrona di
/// Build/Enqueue/ondate/Scout: niente doppio ordine, niente churn (isteresi
/// in executor). Legge gli stessi snapshot onesti (4Hz): mai query nemiche
/// dirette. Loop in ordine di team = deterministico.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn micro_tick(
    mut commands: Commands,
    time: Res<Time>,
    mut acc: Local<f32>,
    config: Res<AiConfig>,
    snapshots: Res<AiSnapshots>,
    match_result: Option<Res<crate::game_over::MatchResult>>,
    scenario: Res<crate::scenario::Scenario>,
    grid: Res<crate::navigation::NavGrid>,
    mut state: ResMut<AiState>,
    mut debug: Option<ResMut<debug::AiDebugState>>,
    units: Query<(Entity, &Transform, &Team, &UnitKind, &UnitOrder, &Health), With<Unit>>,
    buildings: Query<
        (
            Entity,
            &Team,
            &crate::economy::balance::BuildingKind,
            &Transform,
            &Health,
        ),
        (With<Building>,),
    >,
    building_sites: Query<
        (
            Entity,
            &Team,
            &crate::economy::balance::BuildingKind,
            &Transform,
        ),
        (With<Building>, With<Construction>),
    >,
) {
    *acc += time.delta_secs();
    if *acc < MICRO_PERIOD {
        return;
    }
    *acc = 0.0;
    if match_result.is_some_and(|r| r.over) {
        return;
    }

    let mut flat_buildings: Vec<(Team, crate::economy::balance::BuildingKind, Vec3, bool)> =
        Vec::new();
    for (e, team, kind, transform, _) in buildings.iter() {
        let site = building_sites.get(e).is_ok();
        flat_buildings.push((*team, *kind, transform.translation, site));
    }
    flat_buildings.sort_by_key(|(_, _, pos, _)| (pos.x.to_bits(), pos.z.to_bits()));

    // Il micro non costruisce: niente builders/footprints/factories.
    let no_builders: Vec<(Team, Vec3, f32)> = Vec::new();
    let no_footprints: Vec<(Vec3, f32)> = Vec::new();

    let mut micro_labels: Vec<String> = Vec::new();
    for brain in config.sorted_teams() {
        let Some(snapshot) = snapshots.0.get(&brain.team) else {
            continue;
        };
        if snapshot.tick == 0 {
            continue;
        }
        let intents = strategy::decide_micro(snapshot, &brain.personality, *scenario);
        micro_labels.extend(intents.iter().map(|i| format!("m{}:{i:?}", brain.team)));
        if intents.is_empty() {
            continue;
        }
        let mut flat_units: Vec<(Entity, Vec3, UnitKind, UnitOrder)> = units
            .iter()
            .filter(|(_, _, team, _, _, hp)| team.0 == brain.team && hp.current > 0.0)
            .map(|(e, t, _, kind, order, _)| (e, t.translation, *kind, order.clone()))
            .collect();
        flat_units.sort_by_key(|(e, _, _, _)| e.to_bits());

        let (orders, _, _) = executor::execute_movement_and_build(
            &mut commands,
            snapshot,
            &intents,
            brain.team,
            MICRO_BUDGET,
            &grid,
            *scenario,
            &flat_units,
            &flat_buildings,
            &no_builders,
            &no_footprints,
        );
        state.stats_mut(brain.team).orders_issued += orders as u64;
        state.stats_mut(brain.team).micro_orders += orders as u64;
    }

    if let Some(debug) = debug.as_deref_mut() {
        debug.last_intents.extend(micro_labels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        combat::CombatPlugin,
        economy::EconomyPlugin,
        fog::FogPlugin,
        movement::MovementPlugin,
        navigation::{NavigationPlugin, NavigationStats},
        production::ProductionPlugin,
        scenario::Scenario,
        spatial::SpatialPlugin,
        structures::{Building, StructuresPlugin},
        units::{Unit, UnitPlugin},
    };
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    /// Punto 1 — il contatore onde scatta solo al passaggio di periodo:
    /// riemissioni `force_attack` dentro lo stesso periodo non contano.
    #[test]
    fn wave_counter_counts_period_transitions_only() {
        // Periodo 300 tick snapshot: 0..299 = id 0, 300..599 = id 1.
        assert_eq!(note_wave_fired(0, 0), (0, false));
        assert_eq!(note_wave_fired(0, 300), (1, true));
        // Riemissione nello stesso periodo: aggiorna ma non conta.
        assert_eq!(note_wave_fired(1, 301), (1, false));
        assert_eq!(note_wave_fired(1, 599), (1, false));
        assert_eq!(note_wave_fired(1, 600), (2, true));
        // Tick indietro (reset): mai soppressione, mai doppio conteggio.
        assert_eq!(note_wave_fired(2, 10), (2, false));
    }

    /// Smoke test 1v1: entrambi i Commander costruiscono senza panico e senza
    /// barare (snapshot isolati per team). Veloce: 600 tick bastano per i siti.
    #[test]
    fn dual_ai_both_commanders_expand_without_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                1.0 / 60.0,
            )))
            .insert_resource(Scenario::Playground)
            .insert_resource(AiConfig::versus(
                Personality::RUSHER,
                Personality::TURTLE,
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
                AiPlugin,
            ))
            .add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>();
        app.finish();
        app.cleanup();
        app.update();
        for snapshot in app.world().resource::<AiSnapshots>().0.values() {
            assert!(
                snapshot.visible_enemies.is_empty(),
                "the first AI snapshot must respect fog"
            );
            assert!(
                snapshot.memory.is_empty(),
                "bootstrap must not memorize enemies outside sight"
            );
        }
        for _ in 1..600 {
            app.update();
        }
        let world = app.world_mut();
        for team in [0u8, 1u8] {
            let buildings = world
                .query_filtered::<&Team, With<Building>>()
                .iter(world)
                .filter(|t| t.0 == team)
                .count();
            assert!(
                buildings >= 1,
                "team {team} dovrebbe aver avviato almeno un edificio (ne ha {buildings})"
            );
            let units = world
                .query_filtered::<(&Team, &crate::combat::Health), With<Unit>>()
                .iter(world)
                .filter(|(t, h)| t.0 == team && h.current > 0.0)
                .count();
            assert!(units >= 1, "team {team} dovrebbe avere il Commander vivo");
        }
        // Snapshot isolati: entrambi i team hanno la propria vista.
        let snaps = world.resource::<AiSnapshots>();
        assert_eq!(snaps.0.len(), 2);
        assert!(snaps.0.contains_key(&0) && snaps.0.contains_key(&1));
        // Determinismo snapshot: team ordinati.
        let config = world.resource::<AiConfig>();
        let teams = config.sorted_teams();
        assert_eq!(vec![teams[0].team, teams[1].team], vec![0, 1]);
        let _ = world.resource::<NavigationStats>();
    }
}
