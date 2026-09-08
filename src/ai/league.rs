//! AI league: match headless AI-vs-AI fino al game-over + Elo + history.
//!
//! Livello L2 della AI suite: misura se i cervelli *vincono*, non solo se
//! funzionano (quello resta a `harness`/`director`). I numeri sono
//! descrittivi come i benchmark: solo perdita-vs-null e divergenza
//! checksum falliscono il run, il resto sono warning.
//!
//! Ispirazioni: ladder SSCAI/BWAPI (win/loss relativi + Elo), scrimmage
//! Battlecode (match deterministici rigiocabili + baseline stupide),
//! autoplay-agents di Game AI Pro (metriche temporali per tarare).

use crate::{
    benchmark::{HISTORY_ROOT, Metadata, cli::Config},
    combat::{CombatPlugin, Health},
    economy::{Economy, EconomyPlugin},
    fog::{FOG_COUNT, FogPlugin, VisibilityMap},
    game_over::decide_winner,
    movement::MovementPlugin,
    navigation::{
        CELL_SIZE, HALF_SIZE, MAP_OBSTACLES, NavGrid, NavigationPlugin, NavigationStats,
        generate_obstacles,
    },
    production::ProductionPlugin,
    scenario::Scenario,
    spatial::SpatialPlugin,
    structures::{Building, Construction, StructuresPlugin},
    units::{Commander, Team, Unit, UnitKind, UnitPlugin},
};
use bevy::{prelude::*, time::TimeUpdateStrategy};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use super::{
    AiConfig, AiMode, AiTeamConfig,
    director::{self},
    strategy::{self, Personality},
};

pub const LEAGUE_TICKS_CAP: usize = 36_000; // 10 minuti, come da piano
pub const LEAGUE_REPEATS: usize = 2;
pub const SAMPLE_EVERY: usize = 60;
pub const POLL_EVERY: usize = 15;
/// Campioni identici consecutivi prima del draw: 60 x 60 tick = 60s fermi.
pub const STALL_SAMPLES: usize = 60;
pub const ELO_BASE: f64 = 1200.0;
pub const ELO_K: f64 = 16.0;
/// Elo: soglia di allarme su crollo vs priors (solo warning).
pub const ELO_DROP_WARN: f64 = 100.0;
/// Mirror con scarto oltre 30pp dal 50%: sospetto bias di lato/mappa.
pub const MIRROR_BIAS_WARN: f64 = 0.30;
/// Winrate sotto il 40% vs rush-scripted: warning, non fail.
pub const RUSH_WINRATE_WARN: f64 = 0.40;
pub const AI_SUITE_SCHEMA: u32 = 1;
pub const HISTORY_SUBDIR: &str = "ai";

/// Sfidanti della league. `null` = team senza cervello (commander idle che
/// si difende da solo): perderci è regressione critica.
pub const CONTESTANTS: [&str; 5] = ["turtle", "rusher", "eco-only", "rush-scripted", "null"];

/// Risolve un nome in cervello opzionale (`None` = null baseline, nessun AI).
/// Stretto: nomi ignoti sono errore, mai fallback silenzioso a turtle.
pub fn resolve_brain(name: &str) -> Result<Option<Personality>, String> {
    match name {
        "null" => Ok(None),
        "turtle" => Ok(Some(Personality::TURTLE)),
        "rusher" => Ok(Some(Personality::RUSHER)),
        "eco-only" => Ok(Some(Personality::ECO_ONLY)),
        "rush-scripted" => Ok(Some(Personality::RUSH_SCRIPTED)),
        other => Err(format!(
            "Unknown contestant '{other}'; use one of: {}",
            CONTESTANTS.join(", ")
        )),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchCase {
    pub blue: String,
    pub red: String,
    pub seed: u64,
}

impl MatchCase {
    pub fn id(&self) -> String {
        format!("{}-vs-{}-seed{}", self.blue, self.red, self.seed)
    }
}

/// Matrice quick: direzioni incrociate dei main + mirror (bias detector) +
/// copertura null/rush-scripted/eco-only. 8 pairing x 2 repeat.
pub fn quick_matrix() -> Vec<(String, String)> {
    [
        ("turtle", "rusher"),
        ("rusher", "turtle"),
        ("turtle", "turtle"),
        ("rusher", "rusher"),
        ("turtle", "null"),
        ("rusher", "null"),
        ("rusher", "rush-scripted"),
        ("rusher", "eco-only"),
    ]
    .into_iter()
    .map(|(b, r)| (b.to_owned(), r.to_owned()))
    .collect()
}

/// Matrice full: tutte le coppie ordinate dei cervelli (16, mirror inclusi)
/// + ogni cervello vs null in entrambe le direzioni (8). Seed e repeat fuori.
pub fn full_matrix() -> Vec<(String, String)> {
    let brains = ["turtle", "rusher", "eco-only", "rush-scripted"];
    let mut pairs = Vec::new();
    for b in brains {
        for r in brains {
            pairs.push((b.to_owned(), r.to_owned()));
        }
    }
    for b in brains {
        pairs.push((b.to_owned(), "null".to_owned()));
        pairs.push(("null".to_owned(), b.to_owned()));
    }
    pairs
}

pub fn match_seeds(full: bool) -> Vec<u64> {
    if full { vec![0, 1] } else { vec![0] }
}

/// App di match: stesso Playground + plugin dell'harness, ma con 0-2 cervelli
/// (null = team assente) e mappa seedata (0 = standard).
pub(crate) fn build_match_app(
    blue: Option<Personality>,
    red: Option<Personality>,
    seed: u64,
) -> App {
    let mut teams = Vec::new();
    if let Some(p) = blue {
        teams.push(AiTeamConfig::new(0, p));
    }
    if let Some(p) = red {
        teams.push(AiTeamConfig::new(1, p));
    }
    let grid = if seed == 0 {
        NavGrid::default()
    } else {
        NavGrid::new(
            HALF_SIZE,
            CELL_SIZE,
            generate_obstacles(seed, HALF_SIZE, MAP_OBSTACLES),
        )
    };
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Scenario::Playground)
        .insert_resource(AiConfig {
            teams,
            mode: AiMode::Test,
        })
        .insert_resource(grid)
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
    // Warmup della simulazione; il primo raster fog è immediato.
    for _ in 0..20 {
        app.update();
    }
    *app.world_mut().resource_mut::<NavigationStats>() = NavigationStats::default();
    app
}

fn alive_commanders(world: &mut World) -> [usize; 2] {
    let mut n = [0, 0];
    for (t, h) in world
        .query_filtered::<(&Team, &Health), (With<Unit>, With<Commander>)>()
        .iter(world)
    {
        if h.current > 0.0 && (t.0 as usize) < 2 {
            n[t.0 as usize] += 1;
        }
    }
    n
}

/// Firma di attività: checksum unità + edifici + avanzamento cantieri + HP.
/// Tutto fermo per [`STALL_SAMPLES`] campioni consecutivi = stallo (draw).
/// Solo letture: non tocca la simulazione.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActivitySig(u64, u64, u64, u64);

/// Puro: avanza il contatore di stallo. Ritorna `(still, stalled)`.
fn stall_step(still: usize, last: &mut Option<ActivitySig>, sig: ActivitySig) -> (usize, bool) {
    let still = if Some(sig) == *last { still + 1 } else { 0 };
    *last = Some(sig);
    (still, still >= STALL_SAMPLES)
}

fn activity_signature(world: &mut World) -> ActivitySig {
    let units = director::state_checksum(world);
    let mut entries: Vec<(u8, usize, u32, u32, u32, bool)> = world
        .query_filtered::<(
            &Team,
            &crate::economy::balance::BuildingKind,
            &Transform,
            &Health,
            Has<Construction>,
        ), With<Building>>()
        .iter(world)
        .filter(|(_, _, _, h, _)| h.current > 0.0)
        .map(|(t, k, p, h, s)| {
            (
                t.0,
                *k as usize,
                p.translation.x.to_bits(),
                p.translation.z.to_bits(),
                h.current.to_bits(),
                s,
            )
        })
        .collect();
    entries.sort();
    let mut buildings = 0xcbf29ce484222325_u64;
    for (t, k, x, z, hp, s) in entries {
        for value in [t as u64, k as u64, x as u64, z as u64, hp as u64, s as u64] {
            buildings = (buildings ^ value).wrapping_mul(0x100000001b3);
        }
    }
    let mut done = 0.0f64;
    let mut hp = 0.0f64;
    for site in world.query::<&Construction>().iter(world) {
        done += site.0.done;
    }
    for h in world.query::<&Health>().iter(world) {
        hp += h.current as f64;
    }
    ActivitySig(units, buildings, done.to_bits(), hp.to_bits())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TeamSample {
    pub army_value: f64,
    pub income: [f64; 2],
    pub stocks: [f64; 2],
    pub units: usize,
    pub buildings: usize,
    pub explored_pct: f64,
    pub orders: u64,
    pub blocked_factories: usize,
    pub queued_units: usize,
    /// 0.0.16 — threat shadow da `AiSnapshot` onesto (mai query dirette).
    /// Solo osservazione: `decide()` non la legge ancora.
    #[serde(default)]
    pub threat_mean: f64,
    #[serde(default)]
    pub threat_max: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MatchSample {
    pub tick: usize,
    pub blue: TeamSample,
    pub red: TeamSample,
}

pub(crate) fn sample_teams(world: &mut World, tick: usize) -> MatchSample {
    // Threat shadow onesta: solo dagli snapshot per-team (mai posizioni
    // nemiche dirette). Se mancano (scenari scriptati senza cervelli) = 0.
    let threat_of = |id: u8| -> (f64, f64) {
        world
            .get_resource::<super::AiSnapshots>()
            .and_then(|s| s.0.get(&id))
            .map(|snap| {
                let map = super::threat::build_threat(snap);
                (map.mean() as f64, map.max() as f64)
            })
            .unwrap_or((0.0, 0.0))
    };
    let (blue_threat_mean, blue_threat_max) = threat_of(0);
    let (red_threat_mean, red_threat_max) = threat_of(1);
    let mut team = |id: u8| {
        let mut army_value = 0.0;
        let mut units = 0;
        for (t, k, h) in world
            .query_filtered::<(&Team, &UnitKind, &Health), With<Unit>>()
            .iter(world)
        {
            if t.0 == id && h.current > 0.0 {
                units += 1;
                army_value += strategy::combat_power(*k, h.current) as f64;
            }
        }
        let buildings = world
            .query_filtered::<(&Team, &Health), With<Building>>()
            .iter(world)
            .filter(|(t, h)| t.0 == id && h.current > 0.0)
            .count();
        let (income, stocks) = world
            .resource::<Economy>()
            .0
            .get(&id)
            .map(|a| (a.income, a.stock))
            .unwrap_or(([0.0, 0.0], [0.0, 0.0]));
        let explored_pct = world
            .get_resource::<VisibilityMap>()
            .and_then(|m| m.0.get(&id))
            .map(|f| f.explored.iter().filter(|c| **c).count() as f64 / FOG_COUNT as f64)
            .unwrap_or(0.0);
        let orders = world
            .get_resource::<super::AiState>()
            .and_then(|s| s.per_team.get(&id))
            .map(|s| s.orders_issued + s.builds_done + s.enqueues_done)
            .unwrap_or(0);
        let (blocked_factories, queued_units) = world
            .query_filtered::<(&Team, &crate::production::Factory), With<Building>>()
            .iter(world)
            .filter(|(t, _)| t.0 == id)
            .fold((0, 0), |(b, q), (_, f)| {
                (b + usize::from(f.blocked), q + f.queue.len())
            });
        let (threat_mean, threat_max) = if id == 0 {
            (blue_threat_mean, blue_threat_max)
        } else {
            (red_threat_mean, red_threat_max)
        };
        TeamSample {
            army_value,
            income,
            stocks,
            units,
            buildings,
            explored_pct,
            orders,
            blocked_factories,
            queued_units,
            threat_mean,
            threat_max,
        }
    };
    MatchSample {
        tick,
        blue: team(0),
        red: team(1),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchGame {
    pub repeat: usize,
    pub winner: Option<u8>,
    pub draw: bool,
    pub reason: String,
    pub ticks: usize,
    pub checksum: u64,
    pub samples: Vec<MatchSample>,
    pub mean_ms: f64,
    pub first_blood_tick: Option<usize>,
    pub max_army: [f64; 2],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchCaseResult {
    pub case_id: String,
    pub blue: String,
    pub red: String,
    pub seed: u64,
    pub games: Vec<MatchGame>,
    pub deterministic: bool,
}

/// Gioca un match fino a game-over (early exit), stallo o cap.
pub fn run_match(case: &MatchCase, repeat: usize, ticks_cap: usize) -> MatchGame {
    let blue = resolve_brain(&case.blue).expect("league validates names first");
    let red = resolve_brain(&case.red).expect("league validates names first");
    let mut app = build_match_app(blue, red, case.seed);
    let init = sample_teams(app.world_mut(), 0);
    let mut samples = vec![init];
    let mut seen = false;
    let mut last_sig: Option<ActivitySig> = None;
    let mut still = 0usize;
    let mut ms_sum = 0.0;
    let mut ms_n = 0usize;
    let mut end: Option<(Option<u8>, bool, String)> = None;
    let mut final_tick = 0;
    for tick in 1..=ticks_cap {
        let now = Instant::now();
        app.update();
        ms_sum += now.elapsed().as_secs_f64() * 1000.0;
        ms_n += 1;
        if tick % POLL_EVERY == 0 {
            let n = alive_commanders(app.world_mut());
            if n[0] + n[1] > 0 {
                seen = true;
            }
            let res = decide_winner(n[0], n[1], seen);
            if res.over {
                let reason = if res.draw {
                    "draw-both-down".to_owned()
                } else {
                    "commander-down".to_owned()
                };
                end = Some((res.winner, res.draw, reason));
                final_tick = tick;
                break;
            }
        }
        if tick % SAMPLE_EVERY == 0 {
            samples.push(sample_teams(app.world_mut(), tick));
            let (next, stalled) =
                stall_step(still, &mut last_sig, activity_signature(app.world_mut()));
            still = next;
            if stalled {
                end = Some((None, true, "stall".to_owned()));
                final_tick = tick;
                break;
            }
        }
    }
    let (winner, draw, reason) = end.unwrap_or((None, true, "timeout".to_owned()));
    if final_tick == 0 {
        final_tick = ticks_cap;
    }
    // Campione finale: chiude first-blood/milestone anche se il game-over
    // cade tra due campionamenti.
    samples.push(sample_teams(app.world_mut(), final_tick));
    let checksum = director::state_checksum(app.world_mut());
    let mut first_blood_tick = None;
    let mut max_army = [0.0f64; 2];
    // First blood = prima kill: il conteggio scende sotto il massimo visto
    // (i conteggi iniziali sono solo i Commander, mai un buon riferimento).
    let mut max_units = [
        samples.first().map_or(0, |s| s.blue.units),
        samples.first().map_or(0, |s| s.red.units),
    ];
    for s in &samples {
        max_army[0] = max_army[0].max(s.blue.army_value);
        max_army[1] = max_army[1].max(s.red.army_value);
        max_units[0] = max_units[0].max(s.blue.units);
        max_units[1] = max_units[1].max(s.red.units);
        if first_blood_tick.is_none() && (s.blue.units < max_units[0] || s.red.units < max_units[1])
        {
            first_blood_tick = Some(s.tick);
        }
    }
    MatchGame {
        repeat,
        winner,
        draw,
        reason,
        ticks: final_tick,
        checksum,
        samples,
        mean_ms: ms_sum / ms_n.max(1) as f64,
        first_blood_tick,
        max_army,
    }
}

/// Elo standard: atteso logistico, K fisso, pareggio = 0.5.
pub fn expected_score(ra: f64, rb: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf((rb - ra) / 400.0))
}

/// Applica un risultato (dal lato blu) alle rating, a somma zero.
pub fn apply_elo(ratings: &mut BTreeMap<String, f64>, blue: &str, red: &str, score_blue: f64) {
    let ra = *ratings.get(blue).unwrap_or(&ELO_BASE);
    let rb = *ratings.get(red).unwrap_or(&ELO_BASE);
    let ea = expected_score(ra, rb);
    ratings.insert(blue.to_owned(), ra + ELO_K * (score_blue - ea));
    ratings.insert(
        red.to_owned(),
        rb + ELO_K * ((1.0 - score_blue) - (1.0 - ea)),
    );
}

fn score_of(game: &MatchGame) -> f64 {
    if game.draw {
        0.5
    } else if game.winner == Some(0) {
        1.0
    } else {
        0.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeagueReport {
    pub kind: String,
    pub suite_schema_version: u32,
    pub version: String,
    pub preset: String,
    pub metadata: Metadata,
    pub cases: Vec<MatchCaseResult>,
    pub winrate: BTreeMap<String, WinRate>,
    pub elo: BTreeMap<String, f64>,
    pub elo_delta_vs_priors: BTreeMap<String, f64>,
    pub milestones: BTreeMap<String, f64>,
    pub deterministic: bool,
    pub warnings: Vec<String>,
    pub summary_line: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WinRate {
    pub games: usize,
    pub blue_wins: usize,
    pub draws: usize,
    pub red_wins: usize,
}

/// Riga di avanzamento per-game (formato stabile: stesso testo del runner
/// sequenziale, così i log restano confrontabili tra run).
fn game_line(case: &MatchCase, game: &MatchGame) -> String {
    format!(
        "league {} repeat={} winner={} {} ticks={} first_blood={:?} max_army=[{:.0},{:.0}] checksum={:#x} mean={:.2}ms",
        case.id(),
        game.repeat,
        game.winner.map_or("draw".to_owned(), |t| if t == 0 {
            case.blue.clone()
        } else {
            case.red.clone()
        }),
        game.reason,
        game.ticks,
        game.first_blood_tick,
        game.max_army[0],
        game.max_army[1],
        game.checksum,
        game.mean_ms,
    )
}

fn history_dir(metadata: &Metadata) -> std::path::PathBuf {
    std::path::PathBuf::from(HISTORY_ROOT)
        .join(HISTORY_SUBDIR)
        .join(&metadata.machine_key)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn load_priors(dir: &std::path::Path) -> BTreeMap<String, f64> {
    std::fs::read_to_string(dir.join("ratings.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<BTreeMap<String, f64>>(&text).ok())
        .unwrap_or_default()
}

/// Esegue la suite: matrice x seed x repeat, report, guardrail, history.
pub fn run_suite(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let full = config.ai_suite == Some(crate::benchmark::cli::SuitePreset::Full);
    let preset = if full { "full" } else { "quick" };
    let pairings = if full { full_matrix() } else { quick_matrix() };
    let seeds = match_seeds(full);
    let ticks_cap = if config.ticks_overridden {
        config.ticks
    } else {
        LEAGUE_TICKS_CAP
    };
    // Nomi validati prima del primo match (mai fallback silenziosi).
    for (b, r) in &pairings {
        resolve_brain(b)?;
        resolve_brain(r)?;
        if b == "null" && r == "null" {
            return Err("null-vs-null match is meaningless".into());
        }
    }
    let metadata = Metadata::collect();
    let mut priors = load_priors(&history_dir(&metadata));
    let priors_snapshot = priors.clone();
    // Case in ordine deterministico (matrice x seed): l'ordine non cambia
    // mai, in sequenziale come in parallelo — winrate/Elo/history identici.
    let mut cases_in = Vec::new();
    for (blue, red) in &pairings {
        for &seed in &seeds {
            cases_in.push(MatchCase {
                blue: blue.clone(),
                red: red.clone(),
                seed,
            });
        }
    }
    for case in &cases_in {
        // Feedback immediato: ogni match dura minuti, le righe per-game
        // arrivano a fine case (in ordine). Senza `| tail`, lo stream è live.
        println!(
            "league {} start ({} games, cap {} ticks)",
            case.id(),
            LEAGUE_REPEATS,
            ticks_cap
        );
    }
    // Match indipendenti su worker thread: stessa simulazione (step fissi
    // 1/60s, niente wall-clock nel sim) → checksum bit-identici, solo il
    // wall-clock totale diviso per i core. `mean_ms` resta descrittivo
    // (rumoroso sotto carico, mai gate). Niente nuove dipendenze: solo
    // `std::thread::scope`.
    struct CaseOut {
        case: MatchCase,
        games: Vec<MatchGame>,
        deterministic: bool,
        lines: Vec<String>,
        null_loss: bool,
    }
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .max(1);
    let chunk = cases_in.len().div_ceil(workers).max(1);
    let mut ordered: Vec<CaseOut> = Vec::with_capacity(cases_in.len());
    std::thread::scope(|s| {
        let mut handles = Vec::new();
        for chunk_cases in cases_in.chunks(chunk) {
            handles.push(s.spawn(move || {
                let mut out = Vec::with_capacity(chunk_cases.len());
                for case in chunk_cases {
                    let mut games = Vec::with_capacity(LEAGUE_REPEATS);
                    let mut lines = Vec::with_capacity(LEAGUE_REPEATS);
                    for repeat in 0..LEAGUE_REPEATS {
                        let game = run_match(case, repeat + 1, ticks_cap);
                        lines.push(game_line(case, &game));
                        games.push(game);
                    }
                    let deterministic = games.windows(2).all(|w| w[0].checksum == w[1].checksum);
                    let null_loss = games.iter().any(|g| {
                        (case.blue == "null" && g.winner == Some(0))
                            || (case.red == "null" && g.winner == Some(1))
                    });
                    out.push(CaseOut {
                        case: case.clone(),
                        games,
                        deterministic,
                        lines,
                        null_loss,
                    });
                }
                out
            }));
        }
        for h in handles {
            ordered.extend(h.join().expect("league worker panicked"));
        }
    });
    let mut cases = Vec::with_capacity(ordered.len());
    let mut all_deterministic = true;
    let mut null_losses: Vec<String> = Vec::new();
    for o in ordered {
        for line in &o.lines {
            println!("{line}");
        }
        all_deterministic &= o.deterministic;
        if o.null_loss {
            null_losses.push(o.case.id());
        }
        cases.push(MatchCaseResult {
            case_id: o.case.id(),
            blue: o.case.blue.clone(),
            red: o.case.red.clone(),
            seed: o.case.seed,
            games: o.games,
            deterministic: o.deterministic,
        });
    }
    if !all_deterministic {
        return Err("League repeats diverged (nondeterminism)".into());
    }
    // Winrate per coppia ordinata + Elo in ordine deterministico.
    let mut winrate: BTreeMap<String, WinRate> = BTreeMap::new();
    let mut games_sorted: Vec<(String, String, f64)> = Vec::new();
    for case in &cases {
        let entry = winrate
            .entry(format!("{}-vs-{}", case.blue, case.red))
            .or_default();
        for game in &case.games {
            games_sorted.push((case.blue.clone(), case.red.clone(), score_of(game)));
            entry.games += 1;
            if game.draw {
                entry.draws += 1;
            } else if game.winner == Some(0) {
                entry.blue_wins += 1;
            } else {
                entry.red_wins += 1;
            }
        }
    }
    games_sorted.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.total_cmp(&b.2))
    });
    for (b, r, s) in &games_sorted {
        for name in [b, r] {
            priors.entry(name.clone()).or_insert(ELO_BASE);
        }
        apply_elo(&mut priors, b, r, *s);
    }
    let mut elo_delta = BTreeMap::new();
    for (name, rating) in &priors {
        let before = priors_snapshot.get(name).copied().unwrap_or(ELO_BASE);
        elo_delta.insert(name.clone(), rating - before);
    }
    // Milestones: mediane su tutti i game.
    let mut milestones = BTreeMap::new();
    let mut first_bloods: Vec<f64> = vec![];
    let mut max_armies: Vec<f64> = vec![];
    let mut durations: Vec<f64> = vec![];
    // 0.0.16 — copertura e threat shadow (solo osservazione).
    let mut explored: Vec<f64> = vec![];
    let mut threats: Vec<f64> = vec![];
    for case in &cases {
        for game in &case.games {
            if let Some(t) = game.first_blood_tick {
                first_bloods.push(t as f64);
            }
            max_armies.push(game.max_army[0].max(game.max_army[1]));
            durations.push(game.ticks as f64);
            if let Some(last) = game.samples.last() {
                explored.push(last.blue.explored_pct.max(last.red.explored_pct) * 100.0);
                threats.push(last.blue.threat_max.max(last.red.threat_max));
            }
        }
    }
    for (key, mut values) in [
        ("median_first_blood_tick", first_bloods),
        ("median_max_army", max_armies),
        ("median_match_ticks", durations),
        ("median_explored_pct", explored),
        ("median_threat_max", threats),
    ] {
        values.sort_by(f64::total_cmp);
        let median = if values.is_empty() {
            0.0
        } else {
            values[values.len() / 2]
        };
        milestones.insert(key.to_owned(), median);
    }
    // Guardrail morbidi (warning) + uno duro (perdere da null).
    let mut warnings = Vec::new();
    for (pair, wr) in &winrate {
        let total = wr.games.max(1) as f64;
        // Mirror bias detector.
        let sides: Vec<&str> = pair.split("-vs-").collect();
        if sides.len() == 2 && sides[0] == sides[1] {
            let blue_share = (wr.blue_wins as f64 + wr.draws as f64 * 0.5) / total;
            if (blue_share - 0.5).abs() > MIRROR_BIAS_WARN {
                warnings.push(format!(
                    "GUARDRAIL WARN: mirror {pair} blue share {blue_share:.2} (bias sospetto)"
                ));
            }
        }
    }
    // Winrate vs rush-scripted per cervello (serve almeno 2 game).
    for brain in ["turtle", "rusher", "eco-only"] {
        let mut w = 0usize;
        let mut n = 0usize;
        for case in &cases {
            for game in &case.games {
                if case.blue == brain && case.red == "rush-scripted" {
                    n += 1;
                    if game.winner == Some(0) {
                        w += 1;
                    }
                } else if case.blue == "rush-scripted" && case.red == brain {
                    n += 1;
                    if game.winner == Some(1) {
                        w += 1;
                    }
                }
            }
        }
        if n >= 2 {
            let rate = w as f64 / n as f64;
            if rate < RUSH_WINRATE_WARN {
                warnings.push(format!(
                    "GUARDRAIL WARN: {brain} winrate vs rush-scripted {rate:.2} < {RUSH_WINRATE_WARN:.2}"
                ));
            }
        }
    }
    for (name, delta) in &elo_delta {
        if priors_snapshot.contains_key(name) && *delta < -ELO_DROP_WARN {
            warnings.push(format!(
                "GUARDRAIL WARN: {name} Elo {delta:+.0} vs priors (crollo oltre {ELO_DROP_WARN:.0})"
            ));
        }
    }
    for w in &warnings {
        println!("{w}");
    }
    let total_games: usize = cases.iter().map(|c| c.games.len()).sum();
    let decisive = cases
        .iter()
        .flat_map(|c| &c.games)
        .filter(|g| !g.draw)
        .count();
    let draws = total_games - decisive;
    let summary_line = format!(
        "preset={preset} matches={total_games} decisive={decisive} draws={draws} deterministic={all_deterministic}",
    );
    let report = LeagueReport {
        kind: "ai-suite".into(),
        suite_schema_version: AI_SUITE_SCHEMA,
        version: env!("CARGO_PKG_VERSION").into(),
        preset: preset.into(),
        metadata: metadata.clone(),
        cases,
        winrate,
        elo: priors.clone(),
        elo_delta_vs_priors: elo_delta,
        milestones,
        deterministic: all_deterministic,
        warnings: warnings.clone(),
        summary_line: summary_line.clone(),
    };
    // Output: dir --output oppure benchmark-results/ai-<ms> (sempre scritta).
    let dir = match &config.output {
        Some(d) => {
            std::fs::create_dir_all(d)?;
            d.clone()
        }
        None => {
            let d = std::path::PathBuf::from(format!(
                "benchmark-results/ai-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|t| t.as_millis())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&d)?;
            d
        }
    };
    std::fs::write(
        dir.join("ai-match.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    write_markdown(&dir, &report)?;
    println!(
        "League report: {}/ai-match.json ({summary_line})",
        dir.display()
    );
    // Tabelle console: winrate + Elo.
    println!("\n== winrate (blue perspective) ==");
    for (pair, wr) in &report.winrate {
        println!(
            "  {pair}: {}g blue={} draw={} red={}",
            wr.games, wr.blue_wins, wr.draws, wr.red_wins
        );
    }
    println!("== elo ==");
    let mut elo_sorted: Vec<(&String, &f64)> = report.elo.iter().collect();
    elo_sorted.sort_by(|a, b| b.1.total_cmp(a.1));
    for (name, rating) in elo_sorted {
        let delta = report.elo_delta_vs_priors.get(name).copied().unwrap_or(0.0);
        println!("  {name}: {rating:.0} ({delta:+.0})");
    }
    if config.ai_record {
        record_history(&report)?;
    }
    if !null_losses.is_empty() {
        return Err(format!(
            "League regression: lost vs null baseline in {}",
            null_losses.join(", ")
        )
        .into());
    }
    Ok(())
}

fn write_markdown(
    dir: &std::path::Path,
    report: &LeagueReport,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::fmt::Write as _;
    let mut md = String::new();
    writeln!(
        md,
        "# AI suite v{} ({})\n\nPreset: **{}** · machine: `{}` · commit: `{}`{}\n\n{}",
        report.version,
        report.kind,
        report.preset,
        report.metadata.machine_key,
        short_sha(&report.metadata.git_commit),
        if report.metadata.git_dirty {
            " (dirty)"
        } else {
            ""
        },
        report.summary_line,
    )?;
    writeln!(
        md,
        "\n## Winrate (blue perspective)\n\n| Pairing | Games | Blue | Draw | Red |"
    )?;
    writeln!(md, "|---|---:|---:|---:|---:|")?;
    for (pair, wr) in &report.winrate {
        writeln!(
            md,
            "| {} | {} | {} | {} | {} |",
            pair, wr.games, wr.blue_wins, wr.draws, wr.red_wins
        )?;
    }
    writeln!(md, "\n## Elo\n\n| Contestant | Rating | Δ priors |")?;
    writeln!(md, "|---|---:|---:|")?;
    let mut elo_sorted: Vec<(&String, &f64)> = report.elo.iter().collect();
    elo_sorted.sort_by(|a, b| b.1.total_cmp(a.1));
    for (name, rating) in elo_sorted {
        let delta = report.elo_delta_vs_priors.get(name).copied().unwrap_or(0.0);
        writeln!(md, "| {name} | {rating:.0} | {delta:+.0} |")?;
    }
    writeln!(md, "\n## Milestones (medians)\n")?;
    for (k, v) in &report.milestones {
        writeln!(md, "- {k}: {v:.0}")?;
    }
    if !report.warnings.is_empty() {
        writeln!(md, "\n## Warnings\n")?;
        for w in &report.warnings {
            writeln!(md, "- {w}")?;
        }
    }
    writeln!(
        md,
        "\n## Matches\n\n| Case | Repeat | Winner | Reason | Ticks | First blood | Max army | Checksum | Mean ms |"
    )?;
    writeln!(md, "|---|---:|---|---|---:|---:|---|---|---|")?;
    for case in &report.cases {
        for game in &case.games {
            writeln!(
                md,
                "| {} | {} | {} | {} | {} | {} | {:.0}/{:.0} | {:#x} | {:.2} |",
                case.case_id,
                game.repeat,
                game.winner.map_or("draw".to_owned(), |t| if t == 0 {
                    case.blue.clone()
                } else {
                    case.red.clone()
                }),
                game.reason,
                game.ticks,
                game.first_blood_tick
                    .map_or("-".to_owned(), |t| t.to_string()),
                game.max_army[0],
                game.max_army[1],
                game.checksum,
                game.mean_ms,
            )?;
        }
    }
    std::fs::write(dir.join("ai-report.md"), md)?;
    Ok(())
}

fn short_sha(full: &str) -> String {
    full.chars().take(8).collect::<String>()
}

fn record_history(report: &LeagueReport) -> Result<(), Box<dyn std::error::Error>> {
    let dir = history_dir(&report.metadata);
    std::fs::create_dir_all(&dir)?;
    let sha = short_sha(&report.metadata.git_commit);
    let sha = if sha.len() < 8 {
        "nogit".to_owned()
    } else {
        sha
    };
    let name = format!(
        "ai-suite-{}-{}-{}-{}.json",
        report.version,
        sha,
        report.preset,
        now_ms()
    );
    std::fs::write(dir.join(&name), serde_json::to_string_pretty(&report)?)?;
    std::fs::write(
        dir.join("ratings.json"),
        serde_json::to_string_pretty(&report.elo)?,
    )?;
    // history.md rigenerato: una riga per run registrato.
    let mut rows: Vec<(String, String)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("ai-suite-") && n.ends_with(".json"))
            .collect();
        files.sort();
        for file in files {
            if let Ok(text) = std::fs::read_to_string(dir.join(&file))
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
            {
                let summary = value
                    .get("summary_line")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                rows.push((file, summary));
            }
        }
    }
    let mut md = String::from("# AI suite history\n\n| Run | Summary |\n|---|---|\n");
    use std::fmt::Write as _;
    for (file, summary) in rows {
        let _ = writeln!(md, "| {file} | {summary} |");
    }
    std::fs::write(dir.join("history.md"), md)?;
    println!("League history: {}", dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elo_math_is_zero_sum_and_calibrated() {
        // Pari + draw: tutto fermo.
        let mut ratings =
            BTreeMap::from([("turtle".to_owned(), 1200.0), ("rusher".to_owned(), 1200.0)]);
        apply_elo(&mut ratings, "turtle", "rusher", 0.5);
        assert_eq!(ratings["turtle"], 1200.0);
        assert_eq!(ratings["rusher"], 1200.0);
        // Vittoria blu a pari rating: +8/-8, somma costante.
        apply_elo(&mut ratings, "turtle", "rusher", 1.0);
        assert_eq!(ratings["turtle"], 1208.0);
        assert_eq!(ratings["rusher"], 1192.0);
        assert_eq!(ratings["turtle"] + ratings["rusher"], 2400.0);
        // Atteso logistico: favorito sopra, sfavorito sotto 0.5.
        assert!((expected_score(1200.0, 1200.0) - 0.5).abs() < 1e-12);
        assert!(expected_score(1400.0, 1200.0) > 0.5);
    }

    #[test]
    fn contestants_parse_strictly() {
        for name in CONTESTANTS {
            assert!(resolve_brain(name).is_ok(), "{name} deve risolversi");
        }
        assert!(resolve_brain("null").unwrap().is_none());
        assert!(resolve_brain("turtle").unwrap().is_some());
        assert!(resolve_brain("gandalf").is_err());
    }

    #[test]
    fn matrices_have_expected_coverage() {
        let quick = quick_matrix();
        assert_eq!(quick.len(), 8);
        assert!(quick.contains(&("turtle".to_owned(), "turtle".to_owned())));
        assert!(quick.contains(&("rusher".to_owned(), "rusher".to_owned())));
        assert!(quick.contains(&("turtle".to_owned(), "null".to_owned())));
        assert!(quick.contains(&("rusher".to_owned(), "rush-scripted".to_owned())));
        // Niente null-vs-null mai.
        for (b, r) in quick.iter().chain(full_matrix().iter()) {
            assert!(!(b == "null" && r == "null"));
        }
        let full = full_matrix();
        assert_eq!(full.len(), 24);
        assert!(full.contains(&("eco-only".to_owned(), "rush-scripted".to_owned())));
    }

    #[test]
    fn stall_detector_counts_frozen_samples() {
        let a = ActivitySig(1, 2, 3, 4);
        let b = ActivitySig(1, 2, 3, 5);
        let mut last = None;
        let mut still = 0;
        let mut stalled;
        for _ in 0..STALL_SAMPLES {
            (still, stalled) = stall_step(still, &mut last, a);
            assert!(!stalled);
        }
        // L'ultimo step identico fa scattare il draw.
        (still, stalled) = stall_step(still, &mut last, a);
        assert!(stalled);
        // Un cambiamento resetta il contatore.
        (still, stalled) = stall_step(still, &mut last, b);
        assert_eq!(still, 0);
        assert!(!stalled);
    }
}
