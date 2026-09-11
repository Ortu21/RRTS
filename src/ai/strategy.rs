//! Utility strategy + tactics (pure, testable).
//!
//! Decisioni data-driven: ogni opzione ha scorer 0..1, la personalità pesa
//! le soglie. Aggiungere unità/edifici futuri = nuove righe in tabella +
//! pesi in [`Personality`], mai `if kind == X` nel core.

use crate::{
    economy::balance::{BuildingKind, MAX_QUEUE},
    orders::UnitOrder,
    scenario::Scenario,
    structures::{
        building_obstacle, factory_spawn_ok, placement_rule, site_approach, snap_to_grid,
        valid_ground,
    },
    units::UnitKind,
};
use bevy::prelude::*;

use super::{memory::MEMORY_FRESH_TICKS, snapshot::AiSnapshot};

/// Personalità data-driven (oggi const, domani `.ron` senza toccare il core).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Personality {
    pub name: &'static str,
    /// Quante truppe prima di considerare l'attacco.
    pub army_threshold: usize,
    /// 0.0.17 — coraggio: attacca se la win_prob predetta (Lanchester,
    /// vista live + ricordi pesati per età) supera questa soglia.
    pub courage: f32,
    /// Costruisci Engineer prima del secondo Tank (turtle eco).
    pub engineer_first: bool,
    /// Secondo Solar prima di attaccare.
    pub second_solar: bool,
    /// Mix produttivo T1 (kind, peso): la coda insegue queste proporzioni.
    pub mix: [(UnitKind, u32); 3],
    /// Tech reattivi T1 (kind, peso): costruiti SOLO contro comp nemiche che
    /// counterano davvero (edge > soglia, vedi `tech_pick`) — a nemico ignoto
    /// edge 1.0 ovunque e si torna al mix normale. Pesi 0 = mai (baseline).
    /// L'AI scopre così il roster nuovo senza cambiare cervello.
    pub tech_mix: [(UnitKind, u32); 2],
    /// Tetto condiviso tech vivi+accodati (0 = mai, solo main).
    pub max_tech: usize,
    /// Torrette difensive massime (0 = mai, solo turtle).
    pub max_turrets: usize,
    /// Lance turret massime (0 = mai, solo turtle late con eco solida).
    pub max_lance: usize,
    /// 0.0.18 — muri difensivi massimi (0 = mai, solo turtle: schermo
    /// davanti alla prima torretta verso la minaccia).
    pub max_walls: usize,
    /// 0.0.15 — scaling: massimi per tipo (oltre il bootstrap).
    pub max_metals: usize,
    pub max_solars: usize,
    pub max_factories: usize,
    /// 0.0.15 — ingegneri massimi vivi+accodati (code non intasate).
    pub max_engineers: usize,
    /// 0.0.13 — ritirata sotto questa frazione HP (0 = mai).
    pub retreat_hp_frac: f32,
    /// 0.0.17 — focus fire a priorità minaccia quando dominante.
    pub focus_fire: bool,
    /// Tick snapshot (4Hz) oltre il quale si attacca comunque (scripted
    /// baseline). `u64::MAX` = mai: solo potenza/soglie decidono.
    pub attack_at_tick: u64,
    /// 0.0.19 — peso minaccia nella frontiera scout
    /// (`novelty − minaccia_norm×peso` in `scout::frontier_target`).
    /// Turtle cauto (1.2) devia di più, rusher audace (0.5) esplora dritto.
    pub scout_threat_weight: f32,
    /// 0.0.20 — commit del capitale: il Commander marcia nell'ondata solo se
    /// la win_prob predetta supera questa soglia (deterministico: soglia, non
    /// dado). 2.0 = mai (baseline prevedibili). Il capitale non si rischia
    /// per ondate ordinarie: muore lui = game over.
    pub commander_commit_prob: f32,
    /// 0.0.22 — pesi utility full (Graham/Lewis/DA:I): ogni scorer macro
    /// (`build/enqueue/scout/defense` in `utility.rs`) è moltiplicato per il
    /// suo peso e gated a `UTILITY_GATE`. Default 1.0 = comportamento
    /// bit-identico (solo gli zeri veri saltano). Tuning futuro senza toccare
    /// il core.
    pub w_build: f32,
    pub w_enqueue: f32,
    pub w_scout: f32,
    pub w_defense: f32,
}

impl Personality {
    pub const TURTLE: Self = Self {
        name: "turtle",
        army_threshold: 4,
        courage: 0.65,
        engineer_first: true,
        second_solar: true,
        mix: [
            (UnitKind::HeavyTank, 5),
            (UnitKind::LightTank, 2),
            (UnitKind::Artillery, 3),
        ],
        tech_mix: [(UnitKind::MortarTank, 1), (UnitKind::MgTank, 1)],
        max_tech: 2,
        max_turrets: 2,
        max_lance: 1,
        max_walls: 3,
        max_metals: 3,
        max_solars: 3,
        max_factories: 2,
        max_engineers: 2,
        retreat_hp_frac: 0.35,
        focus_fire: true,
        attack_at_tick: u64::MAX,
        scout_threat_weight: 1.2,
        commander_commit_prob: 0.95,
        w_build: 1.0,
        w_enqueue: 1.0,
        w_scout: 1.0,
        w_defense: 1.0,
    };
    pub const RUSHER: Self = Self {
        name: "rusher",
        army_threshold: 2,
        courage: 0.55,
        engineer_first: false,
        second_solar: false,
        mix: [
            (UnitKind::HeavyTank, 6),
            (UnitKind::LightTank, 3),
            (UnitKind::Artillery, 1),
        ],
        tech_mix: [(UnitKind::MgTank, 1), (UnitKind::MortarTank, 1)],
        max_tech: 2,
        max_turrets: 0,
        max_lance: 0,
        max_walls: 0,
        max_metals: 2,
        max_solars: 2,
        max_factories: 1,
        max_engineers: 1,
        retreat_hp_frac: 0.25,
        focus_fire: true,
        attack_at_tick: u64::MAX,
        scout_threat_weight: 0.5,
        commander_commit_prob: 0.9,
        w_build: 1.0,
        w_enqueue: 1.0,
        w_scout: 1.0,
        w_defense: 1.0,
    };
    /// Baseline eco-only per la league: costruisce economia fino ai cap, non
    /// attacca mai (soglia impossibile), nessun micro aggressivo. Sacco da
    /// boxe: perderci è regressione critica.
    pub const ECO_ONLY: Self = Self {
        name: "eco-only",
        army_threshold: usize::MAX,
        courage: 1.1,
        engineer_first: false,
        second_solar: true,
        mix: [
            (UnitKind::Engineer, 2),
            (UnitKind::Scout, 1),
            (UnitKind::HeavyTank, 0),
        ],
        tech_mix: [(UnitKind::Scout, 0), (UnitKind::Scout, 0)],
        max_tech: 0,
        max_turrets: 0,
        max_lance: 0,
        max_walls: 0,
        max_metals: 3,
        max_solars: 3,
        max_factories: 2,
        max_engineers: 3,
        retreat_hp_frac: 0.0,
        focus_fire: false,
        attack_at_tick: u64::MAX,
        scout_threat_weight: 1.0,
        commander_commit_prob: 2.0,
        w_build: 1.0,
        w_enqueue: 1.0,
        w_scout: 1.0,
        w_defense: 1.0,
    };
    /// Baseline rush-scripted per la league: bootstrap + solo HeavyTank,
    /// ondata a tempo fisso comunque vada, niente ritirate né focus.
    /// Pugile prevedibile per misurare la difesa.
    pub const RUSH_SCRIPTED: Self = Self {
        name: "rush-scripted",
        army_threshold: 2,
        courage: 0.55,
        engineer_first: false,
        second_solar: false,
        mix: [
            (UnitKind::HeavyTank, 1),
            (UnitKind::LightTank, 0),
            (UnitKind::Artillery, 0),
        ],
        tech_mix: [(UnitKind::Scout, 0), (UnitKind::Scout, 0)],
        max_tech: 0,
        max_turrets: 0,
        max_lance: 0,
        max_walls: 0,
        max_metals: 2,
        max_solars: 2,
        max_factories: 1,
        max_engineers: 0,
        retreat_hp_frac: 0.0,
        focus_fire: false,
        attack_at_tick: 480, // 120s: l'ondata parte a tempo, comunque vada
        scout_threat_weight: 0.5,
        commander_commit_prob: 2.0,
        w_build: 1.0,
        w_enqueue: 1.0,
        w_scout: 1.0,
        w_defense: 1.0,
    };

    pub fn from_name(name: &str) -> Self {
        Self::try_from_name(name).unwrap_or(Self::TURTLE)
    }

    /// 0.0.21 — strict come `league::resolve_brain`: nomi ignoti sono errore,
    /// mai fallback silenzioso. `from_name()` resta compat (fallback turtle).
    pub fn try_from_name(name: &str) -> Result<Self, String> {
        match name {
            "turtle" => Ok(Self::TURTLE),
            "rusher" => Ok(Self::RUSHER),
            "eco-only" => Ok(Self::ECO_ONLY),
            "rush-scripted" => Ok(Self::RUSH_SCRIPTED),
            other => Err(format!(
                "Unknown personality '{other}'; use one of: turtle, rusher, eco-only, rush-scripted"
            )),
        }
    }

    /// 0.0.21 — carica da `.ron` (`personalities/*.ron`, `ron` transitiva via
    /// bevy, vedi `cargo tree -i ron`). Il `name` viene leakato a `&'static`
    /// (4 caricamenti, mai hot-reload). Errore = stringa, mai panic.
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let def: PersonalityDef = ron::de::from_str(text).map_err(|e| e.to_string())?;
        Ok(Self {
            name: Box::leak(def.name.into_boxed_str()),
            army_threshold: def.army_threshold,
            courage: def.courage,
            engineer_first: def.engineer_first,
            second_solar: def.second_solar,
            mix: def.mix,
            tech_mix: def.tech_mix,
            max_tech: def.max_tech,
            max_turrets: def.max_turrets,
            max_lance: def.max_lance,
            max_walls: def.max_walls,
            max_metals: def.max_metals,
            max_solars: def.max_solars,
            max_factories: def.max_factories,
            max_engineers: def.max_engineers,
            retreat_hp_frac: def.retreat_hp_frac,
            focus_fire: def.focus_fire,
            attack_at_tick: def.attack_at_tick,
            scout_threat_weight: def.scout_threat_weight,
            commander_commit_prob: def.commander_commit_prob,
            w_build: def.w_build,
            w_enqueue: def.w_enqueue,
            w_scout: def.w_scout,
            w_defense: def.w_defense,
        })
    }
}

/// 0.0.21 — forma serializzabile di `Personality` per `.ron` (nome owned,
/// resto identico). `Personality` resta `Copy` con `&'static str` per il sim;
/// la conversione fa `Box::leak` una volta per caricamento.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersonalityDef {
    pub name: String,
    pub army_threshold: usize,
    pub courage: f32,
    pub engineer_first: bool,
    pub second_solar: bool,
    pub mix: [(UnitKind, u32); 3],
    pub tech_mix: [(UnitKind, u32); 2],
    pub max_tech: usize,
    pub max_turrets: usize,
    pub max_lance: usize,
    pub max_walls: usize,
    pub max_metals: usize,
    pub max_solars: usize,
    pub max_factories: usize,
    pub max_engineers: usize,
    pub retreat_hp_frac: f32,
    pub focus_fire: bool,
    pub attack_at_tick: u64,
    pub scout_threat_weight: f32,
    pub commander_commit_prob: f32,
    pub w_build: f32,
    pub w_enqueue: f32,
    pub w_scout: f32,
    pub w_defense: f32,
}

impl From<&Personality> for PersonalityDef {
    fn from(p: &Personality) -> Self {
        Self {
            name: p.name.to_owned(),
            army_threshold: p.army_threshold,
            courage: p.courage,
            engineer_first: p.engineer_first,
            second_solar: p.second_solar,
            mix: p.mix,
            tech_mix: p.tech_mix,
            max_tech: p.max_tech,
            max_turrets: p.max_turrets,
            max_lance: p.max_lance,
            max_walls: p.max_walls,
            max_metals: p.max_metals,
            max_solars: p.max_solars,
            max_factories: p.max_factories,
            max_engineers: p.max_engineers,
            retreat_hp_frac: p.retreat_hp_frac,
            focus_fire: p.focus_fire,
            attack_at_tick: p.attack_at_tick,
            scout_threat_weight: p.scout_threat_weight,
            commander_commit_prob: p.commander_commit_prob,
            w_build: p.w_build,
            w_enqueue: p.w_enqueue,
            w_scout: p.w_scout,
            w_defense: p.w_defense,
        }
    }
}

/// Potenza combattimento stile Lanchester: hp * dps. Engineer disarmato = 0.
/// 0.0.17: singola fonte = `combat::effective_dps` (stessa formula di prima).
pub fn combat_power(kind: UnitKind, health: f32) -> f32 {
    if health <= 0.0 {
        return 0.0;
    }
    health * super::combat::effective_dps(kind)
}

/// Potenza armata viva (telemetria/debug). 0.0.17: `decide()` usa
/// `estimate_forces` + `predict_outcome`; resta API per director/league.
#[allow(dead_code)]
pub fn army_power(snapshot: &AiSnapshot) -> f32 {
    snapshot
        .my_units
        .iter()
        .map(|u| combat_power(u.kind, u.health))
        .sum()
}

/// Potenza nemica visibile (telemetria/debug). Vedi `army_power`.
#[allow(dead_code)]
pub fn visible_enemy_power(snapshot: &AiSnapshot) -> f32 {
    snapshot
        .visible_enemies
        .iter()
        .map(|e| combat_power(e.kind, e.health))
        .sum()
}

#[derive(Clone, Debug, PartialEq)]
pub enum AiIntent {
    Build(BuildingKind),
    Enqueue {
        factory: Entity,
        kind: UnitKind,
    },
    /// 0.0.20 — ondata: QUESTE unità (mai Commander salvo commit, mai scout,
    /// mai arty ferite, mai builder sul sito) in AttackMove sulla meta.
    /// Sostituisce `AttackMoveAll` (che mandava anche il capitale).
    AttackMoveGroup {
        units: Vec<Entity>,
        destination: Vec3,
    },
    Scout {
        destination: Vec3,
    },
    /// 0.0.13 — queste unità (hp bassi) ripiegano con Move (spara in marcia,
    /// mai chase suicida). 0.0.20: l'executor le porta in copertura torrette
    /// (fallback base). Micro 4Hz.
    Retreat {
        units: Vec<Entity>,
    },
    /// 0.0.13 — i più vicini convergono su questo nemico (solo dominante).
    /// Micro 4Hz.
    FocusFire {
        target: Entity,
    },
    /// 0.0.20 — batteria: QUESTE arty sane tengono la massima gittata sul
    /// nemico (Move allo stand-off, acquisizione automatica all'arrivo).
    /// Micro 4Hz, solo senza ondata in corso per unità.
    HoldAtMaxRange {
        units: Vec<Entity>,
        position: Vec3,
    },
    /// 0.0.20 — schermo: QUESTA linea copre le arty / la base (AttackMove in
    /// posizione, ingaggia a contatto). Micro 4Hz, solo senza ondata in corso
    /// per unità.
    Screen {
        units: Vec<Entity>,
        position: Vec3,
    },
    /// Kiting: (unità, meta) per le batterie pressate — il nemico più vicino
    /// è dentro la linea e la batteria arretra sparando (Move spara in marcia)
    /// verso lo stand-off. Micro 4Hz, precede Hold per le unità pressate.
    Kite {
        moves: Vec<(Entity, Vec3)>,
    },
}

/// 0.0.20 — periodo ondate in tick snapshot (4Hz): 300 = 75s. Rivalutazione
/// con `predict_outcome()`: se bassa, l'ondata resta a casa (niente rinforzi
/// suicidi). Multiplo di 4 per costruzione: la strategia a 1Hz vede ogni
/// quarto tick, i tick d'ondata (0, 300, 600, ...) sono sempre osservati
/// (test `waves_tick_alignment`).
pub const WAVE_PERIOD_TICKS: u64 = 300;
/// 0.0.20 — arty sotto questa frazione HP non marciano (inefficaci e fragili:
/// il micro le ritira). Sopra marciano e tengono la gittata.
pub const WOUNDED_ARTY_FRAC: f32 = 0.5;
/// 0.0.20 — raggio base minacciata: nemico visibile o hotspot entro questa
/// distanza da casa = difesa (richiamo: l'ondata non parte, il micro scherma
/// a casa).
pub const BASE_THREAT_RADIUS: f32 = 120.0;
/// 0.0.20 — split linea/batteria per gittata cannone (da tabella, mai branch
/// per-kind): sotto sono linea (screen), sopra batteria (hold). Heavy2 21.9
/// resta linea, Arty 30.0 batteria.
pub const BATTERY_MIN_RANGE: f32 = 25.0;
/// Kiting: dentro questa frazione della propria gittata dal nemico più vicino
/// la batteria arretra sparando invece di tenere la posizione (che verrebbe
/// caricata). Sopra resta in Hold. Isteresi implicita: marciando all'indietro
/// la distanza cresce e il kite si spegne da solo.
pub const KITE_LINE_FRAC: f32 = 0.75;
/// Snipe: col Commander nemico in vista basta questa win_prob (sotto la
/// soglia focus) per designarlo — il capitale vale il rischio che la truppa
/// non vale. Mai sotto: niente suicidi per un'uccisione di prestigio.
pub const SNIPE_MIN_WIN_PROB: f32 = 0.6;

/// Ricordi freschi con potenza stimata, pesata per età (stesso decay della
/// threat map) e scontata (posizioni non verificate). 0.0.17: sostituisce lo
/// sconto fisso *0.5 — stessa formula di `threat::build_threat`, mai divergono.
/// 0.0.17: `decide()` usa `estimate_forces` (stessa matematica per-unità);
/// resta API per test di coerenza e telemetria.
#[allow(dead_code)]
pub fn remembered_enemy_power(snapshot: &AiSnapshot) -> f32 {
    use super::threat::{THREAT_MEMORY_DISCOUNT, age_decay};
    snapshot
        .memory
        .iter()
        .filter(|m| m.age_ticks <= MEMORY_FRESH_TICKS && !m.building)
        .filter_map(|m| {
            m.kind.map(|kind| {
                combat_power(kind, m.hp) * age_decay(m.age_ticks) * THREAT_MEMORY_DISCOUNT
            })
        })
        .sum()
}

/// Ha occhi freschi sul nemico (vista live o ricordi recenti)?
pub fn has_fresh_eyes(snapshot: &AiSnapshot) -> bool {
    !snapshot.visible_enemies.is_empty()
        || snapshot
            .memory
            .iter()
            .any(|m| !m.building && m.age_ticks <= MEMORY_FRESH_TICKS)
}

/// Meta scouting 0.0.19: frontiera (`scout::frontier_target`:
/// conferma del ricordo fresco + `novelty − minaccia_norm×peso` su celle
/// `explored==false`, max 2 scout, waypoint threat-aware in executor).
/// Sostituisce il ciclo 0.0.14 su 4 punti fissi (che mandava gli scout in
/// roccia sui flank ±60m: il rosso `scout-nav-clean` pre-esistente).
/// Wrapper per compat (test + chiamanti legacy): `decide()` chiama
/// `frontier_target` col peso vero. Allow per clippy `-D warnings` fuori test.
/// Uso diretto nei test; oggi solo test oltre a quelli: allow per clippy.
#[allow(dead_code)]
pub fn scout_destination(snapshot: &AiSnapshot, scenario: Scenario) -> Vec3 {
    let threat = super::threat::build_threat(snapshot);
    // La personalità qui non è nota al chiamante legacy: usa il peso medio
    // dei main (0.85). `decide()` chiama `frontier_target` col peso vero.
    let fallback = Personality {
        scout_threat_weight: 0.85,
        ..Personality::RUSHER
    };
    super::scout::frontier_target(snapshot, scenario, &threat, &fallback)
}

/// Meta attacco: inseguimento onesto dei ricordi freschi se il nemico è
/// sparito dalla vista, altrimenti base nemica *ricordata* (0.0.19: edifici
/// visti guidano anche sotto fog — Walsh), altrimenti target statico.
/// 0.0.16: riusa `AiSnapshot::remembered_centroid` (stessa matematica di
/// `memory::remembered_centroid` su `AiMemory`, niente cambi di comportamento).
pub fn attack_destination(snapshot: &AiSnapshot, scenario: Scenario) -> Vec3 {
    if snapshot.visible_enemies.is_empty()
        && let Some(centroid) = snapshot.remembered_centroid(MEMORY_FRESH_TICKS)
        && centroid.is_finite()
    {
        return centroid;
    }
    // 0.0.19 — base nemica ricordata (edifici freschi) prima del target
    // statico: marciare su posizioni verificate invece che su stime.
    if let Some(base) = snapshot.remembered_building_centroid(MEMORY_FRESH_TICKS)
        && base.is_finite()
    {
        return base;
    }
    scenario.attack_target(snapshot.team as usize)
}

/// Tick snapshot (4Hz) minimo per il LabT2: 360 = 90s di partita. Prima il
/// T1 deve chiudersi (placeholder della futura logica a domanda 0.0.15).
pub const LABT2_MIN_TICK: u64 = 360;
/// 0.0.18 — cancello eco LabT2: serve vera economia (2 Metal + 2 Solar di
/// income), non solo il tick. Sotto soglia si espande l'eco T1.
pub const LABT2_MIN_METAL_INCOME: f64 = 10.0;
pub const LABT2_MIN_ENERGY_INCOME: f64 = 24.0;

/// Mix produttivo T2 (kind, peso): i LabT2 inseguono queste proporzioni.
pub const T2_MIX: [(UnitKind, u32); 2] = [(UnitKind::HeavyTank2, 2), (UnitKind::Artillery2, 1)];

/// Vista minima di una factory per la strategia: coda, blocco porte e tier.
#[derive(Clone, Debug)]
pub struct FactoryView {
    pub entity: Entity,
    pub queue_len: usize,
    pub blocked: bool,
    pub tier: u8,
    pub queued: Vec<UnitKind>,
}

/// Unità vive + accodate di un kind (la coda conta: evita di riaccodare lo
/// stesso kind ogni tick mentre la produzione è in corso).
fn count_kind(snapshot: &AiSnapshot, queued: &[UnitKind], kind: UnitKind) -> usize {
    snapshot.my_units.iter().filter(|u| u.kind == kind).count()
        + queued.iter().filter(|k| **k == kind).count()
}

/// 0.0.17 — focus fire solo con vittoria predetta decisiva (dominanza).
/// Sostituisce la vecchia soglia `my > foe*2` con la stessa semantica sulla
/// win_prob di Lanchester.
pub const FOCUS_MIN_WIN_PROB: f32 = 0.8;

/// Kind col rapporto di copertura più basso (conteggio/peso_effettivo);
/// pari → primo. 0.0.17: il peso è già moltiplicato per l'edge counter vs
/// comp nemica (1.0 se nemico ignoto = vecchio comportamento).
/// Deterministico: a parità vince l'ordine di tabella.
fn pick_deficit(cands: &[(UnitKind, f32)], count: impl Fn(UnitKind) -> usize) -> UnitKind {
    let mut best = cands[0].0;
    let mut best_ratio = f32::MAX;
    for (kind, weight) in cands {
        let ratio = count(*kind) as f32 / weight.max(0.05);
        if ratio < best_ratio {
            best_ratio = ratio;
            best = *kind;
        }
    }
    best
}

/// Mix nemico stimato (kind, hp): visibili + ricordi freschi con kind.
/// Aggregato per kind, ordinato per indice (deterministico). Vuoto = ignoto.
fn foe_mix(snapshot: &AiSnapshot) -> Vec<(UnitKind, f32)> {
    use std::collections::BTreeMap;
    let mut acc: BTreeMap<usize, (UnitKind, f32)> = BTreeMap::new();
    for e in &snapshot.visible_enemies {
        acc.entry(e.kind.index())
            .and_modify(|(_, hp)| *hp += e.health)
            .or_insert((e.kind, e.health));
    }
    for m in snapshot.remembered_troop_memory(MEMORY_FRESH_TICKS) {
        if let Some(kind) = m.kind {
            acc.entry(kind.index())
                .and_modify(|(_, hp)| *hp += m.hp)
                .or_insert((kind, m.hp));
        }
    }
    acc.into_values().collect()
}

/// Edge counter medio di `kind` contro il mix nemico (hp-share). 1.0 se ignoto.
fn counter_edge(kind: UnitKind, foe: &[(UnitKind, f32)]) -> f32 {
    use crate::economy::balance::counter_mult;
    let total: f32 = foe.iter().map(|(_, hp)| hp).sum();
    if total <= 0.0 {
        return 1.0;
    }
    foe.iter()
        .map(|(fk, hp)| counter_mult(kind, *fk) * hp)
        .sum::<f32>()
        / total
}

/// Soglia edge per tech pick reattivo: solo counter veri (>1.0 con margine),
/// mai rumore da tabella neutra. Puro.
pub const TECH_EDGE_MIN: f32 = 1.05;

/// Tech pick reattivo: tra i tech con peso>0, quello con edge counter massimo
/// contro la comp nemica — se supera la soglia e c'è cap condiviso.
/// A nemico ignoto (`foe` vuoto) edge 1.0 ovunque → None = mix normale.
/// Puro e deterministico (pareggi → ordine tabella).
pub fn tech_pick(
    tech_mix: &[(UnitKind, u32)],
    foe: &[(UnitKind, f32)],
    opponent: super::opponent::OpponentKind,
    tech_count: usize,
    max_tech: usize,
) -> Option<UnitKind> {
    if foe.is_empty() || tech_count >= max_tech {
        return None;
    }
    let mut best: Option<(UnitKind, f32, u32)> = None;
    for (kind, w) in tech_mix.iter().filter(|(_, w)| *w > 0) {
        let edge = counter_edge(*kind, foe) * super::opponent::mix_bias(*kind, opponent);
        if edge <= TECH_EDGE_MIN {
            continue;
        }
        // Vince l'edge maggiore; a pari edge il peso tabella (priorità della
        // personalità: rusher preferisce Mg, turtle i mortai); poi tabella.
        let better = match best {
            None => true,
            Some((_, be, bw)) => (edge, *w)
                .partial_cmp(&(be, bw))
                .is_some_and(|o| o == std::cmp::Ordering::Greater),
        };
        if better {
            best = Some((*kind, edge, *w));
        }
    }
    best.map(|(k, _, _)| k)
}
/// Pesi mix già corretti per counter e opponent-bias: (kind, peso*edge*bias).
/// Puro. 0.0.19: il bias da tabella `opponent::mix_bias` (±0.1) sposta il mix
/// verso i counter della classe avversaria; Unknown = 1.0 = vecchio comportamento.
fn weighted_mix(
    mix: &[(UnitKind, u32)],
    foe: &[(UnitKind, f32)],
    opponent: super::opponent::OpponentKind,
) -> Vec<(UnitKind, f32)> {
    mix.iter()
        .map(|(kind, w)| {
            (
                *kind,
                *w as f32 * counter_edge(*kind, foe) * super::opponent::mix_bias(*kind, opponent),
            )
        })
        .collect()
}

/// Coppia di forze (kind, hp) per il predittore: (mia, nemica).
type ForcePair = (Vec<(UnitKind, f32)>, Vec<(UnitKind, f32)>);

/// Stima forze per il predittore: armata viva (kind,hp) + nemici visibili +
/// ricordi freschi con hp scontati per età (come `remembered_enemy_power`).
fn estimate_forces(snapshot: &AiSnapshot) -> ForcePair {
    use super::threat::{THREAT_MEMORY_DISCOUNT, age_decay};
    let my_list: Vec<(UnitKind, f32)> =
        snapshot.army().iter().map(|u| (u.kind, u.health)).collect();
    let mut foe_list: Vec<(UnitKind, f32)> = snapshot
        .visible_enemies
        .iter()
        .map(|e| (e.kind, e.health))
        .collect();
    for m in snapshot.remembered_troop_memory(MEMORY_FRESH_TICKS) {
        if let Some(kind) = m.kind {
            foe_list.push((kind, m.hp * age_decay(m.age_ticks) * THREAT_MEMORY_DISCOUNT));
        }
    }
    (my_list, foe_list)
}

/// Punto 1 — win_prob Lanchester dello snapshot (stessa `estimate_forces` di
/// `decide`/`decide_micro`): telemetria e calibrazione courage dai dati veri.
/// Fase A step 2: range-aware come `decide`. Pura e deterministica.
pub fn win_prob_of(snapshot: &AiSnapshot) -> f32 {
    let (my_list, foe_list) = estimate_forces(snapshot);
    super::combat::predict_outcome_at_range(&my_list, &foe_list, engagement_range(snapshot))
}

/// Kind armato proprio più numeroso (primario) per il focus: pareggi → indice
/// minore. Fallback HeavyTank se nessun armato (deterministico comunque).
fn my_primary_kind(snapshot: &AiSnapshot) -> UnitKind {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<usize, (UnitKind, usize)> = BTreeMap::new();
    for u in snapshot.army() {
        counts
            .entry(u.kind.index())
            .and_modify(|(_, n)| *n += 1)
            .or_insert((u.kind, 1));
    }
    counts
        .into_values()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.index().cmp(&a.0.index())))
        .map(|(kind, _)| kind)
        .unwrap_or(UnitKind::HeavyTank)
}

/// Commander nemico in vista da snipare: hp minori, poi determinismo.
/// `None` senza Commander visibili. Puro; la soglia win_prob resta al
/// chiamante (`SNIPE_MIN_WIN_PROB`).
pub fn snipe_target(snapshot: &AiSnapshot) -> Option<Entity> {
    snapshot
        .visible_enemies
        .iter()
        .filter(|e| e.kind == UnitKind::Commander)
        .min_by(|a, b| {
            a.health
                .total_cmp(&b.health)
                .then_with(|| a.entity.to_bits().cmp(&b.entity.to_bits()))
        })
        .map(|e| e.entity)
}

/// 0.0.20 — numero d'ondata (periodo 75s): tick snapshot (4Hz) / 300.
/// Monotono per costruzione (test `waves_monotonic`). Puro.
/// Uso diretto in telemetria/test; oggi solo test: allow per clippy.
#[allow(dead_code)]
pub fn wave_id(tick: u64) -> u64 {
    tick / WAVE_PERIOD_TICKS
}

/// 0.0.20 — tick di lancio ondata (inizio periodo). La strategia a 1Hz vede
/// ogni quarto tick e il periodo è multiplo di 4: i lanci non si perdono mai
/// (test `waves_tick_alignment`). Puro.
pub fn is_wave_tick(tick: u64) -> bool {
    tick.is_multiple_of(WAVE_PERIOD_TICKS)
}

/// 0.0.20 — ruolo batteria (tiene la gittata) vs linea (fa schermo): dalla
/// gittata cannone in tabella, mai branch per-kind. Puro.
pub fn is_arty_role(kind: UnitKind) -> bool {
    crate::units::archetype(kind).range >= BATTERY_MIN_RANGE
}

/// 0.0.20 — gruppo d'ondata: linea sana (niente scout, niente Commander, niente
/// arty ferite, mai builder sul sito), ordinato per bits. Il Commander si
/// aggiunge SOLO se `win_prob > commander_commit_prob` E la linea non è vuota
/// E la base NON è minacciata (il capitale non si gioca nelle mischie
/// difensive: lì ripiega col micro, vedi `decide_micro`). Puro e deterministico.
pub fn wave_group(
    snapshot: &AiSnapshot,
    personality: &Personality,
    win_prob: f32,
    base_threatened: bool,
) -> Vec<Entity> {
    let mut group: Vec<Entity> = snapshot
        .my_units
        .iter()
        .filter(|u| {
            crate::units::archetype(u.kind).armed
                && u.kind != UnitKind::Scout
                && u.kind != UnitKind::Commander
                && !(is_arty_role(u.kind)
                    && u.max_health > 0.0
                    && u.health / u.max_health < WOUNDED_ARTY_FRAC)
                && !matches!(u.order, UnitOrder::Build { .. })
                && u.health > 0.0
        })
        .map(|u| u.entity)
        .collect();
    group.sort_by_key(|e| e.to_bits());
    // Commit del capitale: solo con vittoria quasi certa, scorta presente e
    // base libera (mai nelle mischie difensive: il torneo mostra il Commander
    // perso proprio lì anche oltre soglia 0.95).
    if !group.is_empty() && !base_threatened && win_prob > personality.commander_commit_prob {
        let mut capitals: Vec<Entity> = snapshot
            .my_units
            .iter()
            .filter(|u| {
                u.kind == UnitKind::Commander
                    && u.health > 0.0
                    && !matches!(u.order, UnitOrder::Build { .. })
            })
            .map(|u| u.entity)
            .collect();
        group.append(&mut capitals);
        group.sort_by_key(|e| e.to_bits());
    }
    group
}

/// 0.0.20 — base minacciata: nemico visibile o hotspot threat entro il raggio
/// da casa. Con la base sotto pressione l'ondata non parte (richiamo
/// difensivo: il micro scherma a casa). Solo snapshot onesto. Puro.
/// Costruisce la threat e delega (i sistemi con cache usano `..._with_map`).
pub fn base_under_threat(snapshot: &AiSnapshot, scenario: Scenario) -> bool {
    let threat = super::threat::build_threat(snapshot);
    base_under_threat_with_map(snapshot, scenario, &threat)
}

/// 0.0.23 — come `base_under_threat`, con mappa già pronta dalla `ThreatCache`.
/// Stessa matematica: a mappa uguale, stesso booleano. Puro.
pub fn base_under_threat_with_map(
    snapshot: &AiSnapshot,
    scenario: Scenario,
    threat: &super::threat::ThreatMap,
) -> bool {
    let home = scenario.center(snapshot.team as usize);
    if snapshot
        .visible_enemies
        .iter()
        .any(|e| e.pos.xz().distance(home.xz()) < BASE_THREAT_RADIUS)
    {
        return true;
    }
    threat
        .hotspot()
        .is_some_and(|h| h.xz().distance(home.xz()) < BASE_THREAT_RADIUS)
}

/// Fase A step 2 — distanza d'ingaggio (mio baricentro armato → baricentro
/// minaccia: visibile, altrimenti ricordi freschi). Sconosciuta = 0.0
/// (mischia immediata = vecchia matematica esatta). Pura.
pub fn engagement_range(snapshot: &AiSnapshot) -> f32 {
    let mut sum = Vec3::ZERO;
    let mut n = 0u32;
    for u in snapshot.army() {
        sum += u.pos;
        n += 1;
    }
    if n == 0 {
        return 0.0;
    }
    let mine = sum / n as f32;
    if let Some(vis) = visible_centroid(snapshot) {
        return mine.distance(vis);
    }
    if let Some(rem) = snapshot.remembered_centroid(MEMORY_FRESH_TICKS) {
        return mine.distance(rem);
    }
    0.0
}

/// 0.0.20 — baricentro nemici visibili (punto minaccia per screen/hold).
/// Ordine snapshot (bits) = somma deterministica. `None` senza contatti live
/// (niente posizionamento sui fantasmi). Puro.
pub fn visible_centroid(snapshot: &AiSnapshot) -> Option<Vec3> {
    if snapshot.visible_enemies.is_empty() {
        return None;
    }
    let mut sum = Vec3::ZERO;
    for e in &snapshot.visible_enemies {
        sum += e.pos;
    }
    Some(sum / snapshot.visible_enemies.len() as f32)
}

/// 0.0.20 — ancora di ritirata: torretta completa propria più vicina a casa
/// (copertura cannoni), fallback casa senza torrette. `turrets` = posizioni
/// torrette complete del team ( executor: da edifici vivi). Puro.
pub fn retreat_anchor(turrets: &[Vec3], home: Vec3) -> Vec3 {
    turrets
        .iter()
        .min_by(|a, b| {
            a.distance_squared(home)
                .total_cmp(&b.distance_squared(home))
                .then_with(|| a.x.to_bits().cmp(&b.x.to_bits()))
                .then_with(|| a.z.to_bits().cmp(&b.z.to_bits()))
        })
        .copied()
        .unwrap_or(home)
}

/// 0.0.20 — posizione schermo: 30% da guardia a minaccia (linea davanti alle
/// batterie / alla base). Pura.
pub fn screen_position(guard: Vec3, threat: Vec3) -> Vec3 {
    guard.lerp(threat, 0.3).with_y(0.0)
}

/// Kite: punto di arretramento per unità pressata — proiezione a `range − 4`
/// dalla minaccia lungo la direttrice, SEMPRE (a differenza di `hold_position`,
/// che tiene il terreno se già in gittata: lì va bene, qui serve creare
/// distanza). Degeneri (sopra il nemico): passo +X deterministico. Pura.
pub fn kite_position(unit: Vec3, threat: Vec3, range: f32) -> Vec3 {
    let mut dir = unit - threat;
    dir.y = 0.0;
    let dir = if dir.length_squared() < 1e-6 {
        Vec3::X
    } else {
        dir.normalize()
    };
    (threat + dir * (range - 4.0)).with_y(0.0)
}

/// 0.0.20 — posizione batteria: sul lato armata della minaccia, a
/// `range − 4` (dentro gittata con margine). Già in gittata (distanza
/// ancora-minaccia <= `range − 4`): tiene il terreno (niente avanzate
/// inutili dentro la gittata). Pura.
pub fn hold_position(anchor: Vec3, threat: Vec3, range: f32) -> Vec3 {
    let mut dir = anchor - threat;
    dir.y = 0.0;
    if dir.length_squared() < 1.0 {
        return anchor.with_y(0.0);
    }
    let dist = dir.length();
    if dist <= range - 4.0 {
        return anchor.with_y(0.0);
    }
    (threat + dir / dist * (range - 4.0)).with_y(0.0)
}

/// Utility scoring: ritorna intenti ordinati per priorità. Puro e deterministico.
/// `last_wave_id` = ultima ondata lanciata (da `AiState`, thread di `ai_tick`):
/// l'ondata parte al tick d'ondata oppure al primo tick pronto del periodo
/// (catch-up: se la massa non era pronta al confine, parte appena pronta
/// invece di aspettare 75s — e se la cadenza salta un tick d'ondata, parte al
/// successivo). Puro: lo stato resta fuori, qui solo confronto.
/// Costruisce la threat (1 build) e delega a `decide_with_threat`: i sistemi
/// con cache chiamano quella direttamente (zero rebuild).
pub fn decide(
    snapshot: &AiSnapshot,
    personality: &Personality,
    scenario: Scenario,
    factories: &[FactoryView],
    last_wave_id: u64,
) -> Vec<AiIntent> {
    let threat = super::threat::build_threat(snapshot);
    decide_with_threat(
        snapshot,
        personality,
        scenario,
        factories,
        last_wave_id,
        &threat,
    )
}

/// 0.0.23 — come `decide`, ma con threat map già pronta (dalla `ThreatCache`
/// di `ai_tick`: 1 build per team per strategy-tick invece di ~5). Stessa
/// matematica di `decide`: a mappa uguale, intenti bit-identici (test
/// `decide_with_threat_matches_decide`). Puro e deterministico.
pub fn decide_with_threat(
    snapshot: &AiSnapshot,
    personality: &Personality,
    scenario: Scenario,
    factories: &[FactoryView],
    last_wave_id: u64,
    threat: &super::threat::ThreatMap,
) -> Vec<AiIntent> {
    let mut intents = Vec::new();

    // 1. Macro a loop chiuso: bootstrap + scaling a domanda.
    // Legge stock/income/demand invece di costruire "a memoria".
    // G1 — Metal solo su spot liberi (propri + nemici visibili chiudono):
    // senza spot si passa oltre (solare/altro), mai code intasate.
    let mut metal_pos: Vec<Vec3> = snapshot
        .my_buildings
        .iter()
        .filter(|b| b.kind == BuildingKind::Metal)
        .map(|b| b.pos)
        .collect();
    metal_pos.extend(
        snapshot
            .visible_enemy_buildings
            .iter()
            .filter(|b| b.kind == BuildingKind::Metal)
            .map(|b| b.pos),
    );
    let metal_free = crate::structures::count_free(&snapshot.deposits, &metal_pos);
    // BAR-style: un intento Build per builder libero (Idle/Hold) — ognuno
    // porta avanti il suo cantiere. Il tetto emerge dai builder, non da regole:
    // a inizio partita è 1 sito come prima, con l'Engineer diventano 2.
    let idle_builders = snapshot
        .my_units
        .iter()
        .filter(|u| {
            u.kind.is_builder()
                && u.health > 0.0
                && matches!(u.order, UnitOrder::Idle | UnitOrder::HoldPosition)
        })
        .count();
    if idle_builders > 0 {
        let metal = snapshot.count_building(BuildingKind::Metal);
        let solar = snapshot.count_building(BuildingKind::Solar);
        let factory = snapshot.count_building(BuildingKind::Factory);
        // 0.0.18 — collo di bottiglia da planner su costi reali (stock,
        // income, demand + costi tabella): sostituisce l'euristica
        // domanda > offerta × 1.1 a pari casi singoli, sceglie meglio se
        // entrambe le risorse mancano (vince il deficit relativo maggiore).
        let plan = super::planner::plan_build(snapshot.stock, snapshot.income, snapshot.demand);
        let metal_starved = plan.bottleneck == Some(BuildingKind::Metal);
        let energy_starved = plan.bottleneck == Some(BuildingKind::Solar);
        // Debito Punto 5 — TTA gate: se il collo di bottiglia non è abbordabile
        // entro l'orizzonte (tta INF: income zero o oltre 60s), aspetta eco
        // invece di accodare un Build che resta fermo. Il bootstrap resta
        // incondizionato (senza primo Metal/Solar/Factory non c'è income).
        let affordable = plan.time_to_afford_secs.is_finite();
        // La specie prioritaria si decide una volta sola; poi un intento per
        // builder libero (stessa specie in parallelo = eco parallela, come un
        // player che sdoppia i builder). L'executor assegna builder distinti e
        // salta gli spot già presi nello stesso tick.
        let kind = if metal == 0 && metal_free > 0 {
            Some(BuildingKind::Metal)
        } else if solar == 0 {
            Some(BuildingKind::Solar)
        } else if factory == 0 {
            Some(BuildingKind::Factory)
        } else if personality.second_solar && solar < 2 {
            Some(BuildingKind::Solar)
        } else if metal_starved && affordable && metal < personality.max_metals && metal_free > 0 {
            Some(BuildingKind::Metal)
        } else if energy_starved && affordable && solar < personality.max_solars {
            Some(BuildingKind::Solar)
        } else if factory < personality.max_factories && metal >= 2 {
            // Seconda lab solo a eco metal avviata (2 Metal): raddoppia il
            // throughput, ma va nutrita.
            Some(BuildingKind::Factory)
        } else if snapshot.complete_building(BuildingKind::LabT2) == 0
            && snapshot.complete_building(BuildingKind::Factory) > 0
            && snapshot.tick >= LABT2_MIN_TICK
            && snapshot.income[0] >= LABT2_MIN_METAL_INCOME
            && snapshot.income[1] >= LABT2_MIN_ENERGY_INCOME
        {
            Some(BuildingKind::LabT2)
        } else {
            None
        };
        if let Some(kind) = kind {
            // 0.0.22 — gate utility (Graham/Lewis): bootstrap = 1.0, scaling =
            // bottleneck × abbordabilità. A pesi default passa sempre quando
            // il vecchio codice emetteva (solo gli zeri veri saltano).
            let bootstrap_needed = metal == 0 || solar == 0 || factory == 0;
            let bottleneck_or_programmed = metal_starved
                || energy_starved
                || kind == BuildingKind::Factory
                || kind == BuildingKind::LabT2
                || (personality.second_solar && kind == BuildingKind::Solar);
            let u = super::utility::build_urgency(
                bootstrap_needed,
                bottleneck_or_programmed,
                affordable || bootstrap_needed,
                true,
                if bootstrap_needed {
                    0.0
                } else {
                    plan.time_to_afford_secs
                },
            );
            if u * personality.w_build > super::utility::UTILITY_GATE {
                for _ in 0..idle_builders {
                    intents.push(AiIntent::Build(kind));
                }
            }
        }
    }
    // 0.0.22 — minaccia base una volta per tick (riusata da difesa utility
    // e tattica ondate): threat map passata dal chiamante (cache 0.0.23).
    let threatened_early = base_under_threat_with_map(snapshot, scenario, threat);
    // Difesa statica: torrette vicino alla base (l'executor cerca lo spot in
    // spirale dal centro). Conta anche i siti: niente doppie richieste.
    // 0.0.22 — gated da `defense_urgency × w_defense` (default passa sempre
    // quando il vecchio codice emetteva).
    if personality.max_turrets > 0
        && snapshot.complete_building(BuildingKind::Factory) > 0
        && snapshot.count_building(BuildingKind::Turret) < personality.max_turrets
        && super::utility::defense_urgency(true, threatened_early) * personality.w_defense
            > super::utility::UTILITY_GATE
    {
        intents.push(AiIntent::Build(BuildingKind::Turret));
    }
    // Lance turret (scala gittate): seconda risposta statica, solo turtle late
    // con eco solida (stesso cancello del LabT2: niente torrette da 220/220 a
    // economia zoppa). Conta anche i siti: niente doppie richieste.
    if personality.max_lance > 0
        && snapshot.complete_building(BuildingKind::Turret) > 0
        && snapshot.count_building(BuildingKind::Lance) < personality.max_lance
        && snapshot.income[0] >= LABT2_MIN_METAL_INCOME
        && snapshot.income[1] >= LABT2_MIN_ENERGY_INCOME
        && super::utility::defense_urgency(true, threatened_early) * personality.w_defense
            > super::utility::UTILITY_GATE
    {
        intents.push(AiIntent::Build(BuildingKind::Lance));
    }
    // 0.0.18 — muri: schermo davanti alla prima torretta completa (l'executor
    // cerca gli slot verso la minaccia, senza murare le factory). Solo turtle,
    // contati con i siti: niente doppie richieste.
    if personality.max_walls > 0
        && snapshot.complete_building(BuildingKind::Turret) > 0
        && snapshot.count_building(BuildingKind::Wall) < personality.max_walls
        && super::utility::defense_urgency(true, threatened_early) * personality.w_defense
            > super::utility::UTILITY_GATE
    {
        intents.push(AiIntent::Build(BuildingKind::Wall));
    }

    // 0.0.17 — comp nemica stimata una volta per tick: guida i pesi mix
    // (counter) e il predittore Lanchester. Vuota = nemico ignoto.
    let foe = foe_mix(snapshot);
    // 0.0.19 — opponent modeling leggero: classifica da conteggi freschi +
    // timing primo contatto + edifici visti → shift mix/courage ±0.1 da
    // tabella. Unknown = neutro = vecchio comportamento.
    let opponent = super::opponent::classify(snapshot);
    let courage = super::opponent::adjust_courage(personality.courage, opponent);

    // 2. Produzione: una enqueue per factory libera (N lab = N code in
    // parallelo). Le bloccate si saltano: accodare lì brucia solo eco.
    // I conteggi includono vivi + accodati così le code non si intasano di
    // Engineer/Scout mentre il primo esce ancora dalla factory.
    let mut queued_all: Vec<UnitKind> = factories
        .iter()
        .flat_map(|f| f.queued.iter().copied())
        .collect();
    for view in factories {
        if view.queue_len >= MAX_QUEUE || view.blocked {
            continue;
        }
        let engineers = count_kind(snapshot, &queued_all, UnitKind::Engineer);
        let scouts = count_kind(snapshot, &queued_all, UnitKind::Scout);
        let kind = if personality.engineer_first
            && engineers < personality.max_engineers
            && snapshot.complete_building(BuildingKind::Factory) > 0
        {
            UnitKind::Engineer
        } else if scouts < super::scout::MAX_SCOUTS
            && !has_fresh_eyes(snapshot)
            && snapshot.complete_building(BuildingKind::Factory) > 0
            && UnitKind::PRODUCIBLE.contains(&UnitKind::Scout)
        {
            // 0.0.14 — occhi prima di muscoli: Scout esploratori (0.0.19: max
            // 2, frontiera novelty−minaccia invece di un singolo punto fisso).
            UnitKind::Scout
        } else if view.tier >= 2 {
            // LabT2: mix pesante T2 pesato per counter + bias opponent. Il gate
            // di `enqueue` lo ribadisce, ma qui non si emette mai un T2 verso
            // una T1.
            pick_deficit(&weighted_mix(&T2_MIX, &foe, opponent), |k| {
                count_kind(snapshot, &queued_all, k)
            })
        } else {
            // Tech reattivo prima del mix: se la comp nemica è nota e un tech
            // la countera davvero (edge > soglia) con cap libero, si costruisce
            // lui (Mg vs sciami Light, mortai vs corazze). A nemico ignoto o
            // cap pieno: mix normale, comportamento invariato.
            let tech_count: usize = personality
                .tech_mix
                .iter()
                .filter(|(_, w)| *w > 0)
                .map(|(k, _)| count_kind(snapshot, &queued_all, *k))
                .sum();
            if let Some(tech) = tech_pick(
                &personality.tech_mix,
                &foe,
                opponent,
                tech_count,
                personality.max_tech,
            ) {
                tech
            } else {
                // Mix T1 per deficit di copertura pesato per counter e opponent
                // (vivi + accodati). Pesi 0 = mai (baseline eco): mix vuoto →
                // nessuna enqueue. Nemico ignoto → edge/bias 1.0 = vecchio
                // comportamento.
                let mix: Vec<(UnitKind, u32)> = personality
                    .mix
                    .iter()
                    .copied()
                    .filter(|(_, w)| *w > 0)
                    .collect();
                if mix.is_empty() {
                    continue;
                }
                pick_deficit(&weighted_mix(&mix, &foe, opponent), |k| {
                    count_kind(snapshot, &queued_all, k)
                })
            }
        };
        // Accoda solo se producibile (mai Commander).
        // 0.0.22 — gate utility: capacità libera × peso (default passa sempre
        // quando il vecchio codice emetteva: code libere = urgenza > gate).
        if UnitKind::PRODUCIBLE.contains(&kind) {
            let u = super::utility::enqueue_urgency(view.queue_len, MAX_QUEUE, view.blocked);
            if u * personality.w_enqueue > super::utility::UTILITY_GATE {
                queued_all.push(kind);
                intents.push(AiIntent::Enqueue {
                    factory: view.entity,
                    kind,
                });
            } else {
                // Capacità zero: la coda conta comunque per i deficit (evita
                // di riproporre lo stesso kind ogni tick).
                queued_all.push(kind);
            }
        }
    }

    // 3. Tattica utility (Punto 2): `opportunity = win_prob × readiness ×
    // safety` (prodotto fuzzy-AND, vedi `utility.rs`) confrontata con il
    // `courage` effettivo (base + shift opponent ±0.1). Sostituisce il cancello
    // booleano 0.0.17 e il ramo cieco separato (alla cieca soglia +2 in curva).
    // Baseline scripted a tempo fisso come prima (bypassano il cancello
    // d'ondata, mai quello di difesa). Il richiamo difensivo non è
    // più un muro: minacciati ma dominanti si contrattacca (`safety → 1`).
    let army_count = snapshot.army().len();
    let (my_list, foe_list) = estimate_forces(snapshot);
    // Fase A step 2 — doppia stima: range-aware per marciare verso il nemico
    // (chi picchia da fuori emerge dal sim), mischia per il contrattacco in
    // casa (lì conta la supremazia di forze, non i 10s di orizzonte: da 300m
    // il sim non vede l'ingaggio ma marciare resta giusto se dominanti).
    let win_prob =
        super::combat::predict_outcome_at_range(&my_list, &foe_list, engagement_range(snapshot));
    let threatened = threatened_early;
    // Alla cieca (mai visto il nemico: `win_prob` è 1.0 a vuoto) serve la massa
    // critica di prima: soglia effettiva +2, in curva invece che in ramo morto.
    // Saturating: soglie "mai" (usize::MAX delle baseline) non vanno in overflow.
    let blind = snapshot.visible_enemies.is_empty() && !has_fresh_eyes(snapshot);
    let threshold_eff = personality
        .army_threshold
        .saturating_add(if blind { 2 } else { 0 });
    let opportunity =
        super::utility::attack_opportunity(win_prob, army_count, threshold_eff, threatened);
    let power_ok = opportunity > courage;
    let force_attack = army_count > 0 && snapshot.tick >= personality.attack_at_tick;
    let on_wave = is_wave_tick(snapshot.tick);
    // Catch-up: prima prontezza di ogni periodo (vedi doc di `decide`).
    let wave_due = on_wave || wave_id(snapshot.tick) > last_wave_id;
    // Minacciati: niente ondata programmata verso la base nemica (richiamo
    // difensivo), MA se dominanti in mischia si contrattacca SULLA minaccia
    // in casa (centroide visibile: l'AttackMove ingaggia strada facendo).
    // Le baseline scripted restano richiamate sempre (niente micro difensivo
    // a coprirle).
    let push_threatened = threatened
        && wave_due
        && visible_centroid(snapshot).is_some()
        && super::utility::attack_opportunity(
            super::combat::predict_outcome(&my_list, &foe_list),
            army_count,
            threshold_eff,
            true,
        ) > courage;
    let scheduled = power_ok && wave_due && !threatened;
    let scripted = force_attack && !threatened;
    if scheduled || scripted || push_threatened {
        let destination = if push_threatened {
            visible_centroid(snapshot).unwrap_or_else(|| attack_destination(snapshot, scenario))
        } else {
            attack_destination(snapshot, scenario)
        };
        let group = wave_group(snapshot, personality, win_prob, threatened);
        if !group.is_empty() {
            intents.push(AiIntent::AttackMoveGroup {
                units: group,
                destination,
            });
        }
    }
    // 0.0.19 — pattuglia frontiera (fix ramo morto: prima `army_count == 0`,
    // impossibile col Commander armato vivo, teneva lo scout in base).
    // Ogni scout libero (Idle/Hold) ha sempre una meta + ogni scout in rotta
    // verso una meta diventata CALDA (threat > soglia: lo attende un esercito
    // avvistato dopo l'ordine — devia invece di immolarsi, visto dal vivo:
    // lo striker moriva nei cannoni a 10m dalla base rossa).
    // Mete = conferma del contatto perso a vista live vuota, altrimenti top-N
    // frontiera (`novelty − minaccia_norm×peso`, ventaglio su assi distinti e
    // corridoi in corso per max 2 scout). Indipendente dall'attacco: gli
    // scout sono occhi, non linea — mentre l'esercito marcia, loro mappano
    // (explored) e riacquisiscono (occhi per il courage, che attacca a soglia
    // invece che a massa cieca: first-blood).
    {
        // 0.0.23 — threat dalla cache del chiamante (zero rebuild qui).
        let free_scouts = snapshot
            .my_units
            .iter()
            .filter(|u| {
                u.kind == UnitKind::Scout
                    && (matches!(u.order, UnitOrder::Idle | UnitOrder::HoldPosition)
                        || matches!(u.order, UnitOrder::Move { destination }
                        if threat.query(destination)
                            > super::scout::SCOUT_THREAT_THRESHOLD))
            })
            .count();
        // 0.0.22 — gate utility (occhi + novelty): alla cieca = alto, con
        // occhi + mappa piena = basso ma > 0 (default passa sempre quando ci
        // sono scout liberi, come prima).
        let u = super::utility::scout_urgency(
            free_scouts,
            has_fresh_eyes(snapshot),
            snapshot.explored_pct,
        );
        if u * personality.w_scout <= super::utility::UTILITY_GATE {
            return intents;
        }
        let n = free_scouts.min(super::scout::MAX_SCOUTS);
        if n > 0 {
            for destination in
                super::scout::frontier_targets(snapshot, scenario, threat, personality, n)
            {
                intents.push(AiIntent::Scout { destination });
            }
        }
    }

    // 4. Micro 0.0.13/0.0.20 — SOLO Retreat/Focus a 1Hz qui per compat? No:
    // dal 0.0.20 il micro vive in `decide_micro()` a 4Hz (`micro_tick`).
    // `decide()` resta macro (Build/Enqueue/ondate/Scout): niente doppio
    // ordine alle stesse unità nello stesso tick.

    intents
}

/// 0.0.20 — micro a 4Hz: SOLO Retreat/FocusFire/HoldAtMaxRange/Screen, in
/// quest'ordine di priorità (il budget APM taglia dalla coda). Puro e
/// deterministico. Screen/Hold solo per unità ferme (Idle/Hold): chi marcia
/// in un'ondata non viene deviato (niente churn marcia↔schermo a 4Hz); chi
/// arriva a destinazione torna Idle e si riposiziona. Retreat/Focus valgono
/// sempre (urgenza e dominanza scavalcano la marcia).
pub fn decide_micro(
    snapshot: &AiSnapshot,
    personality: &Personality,
    scenario: Scenario,
) -> Vec<AiIntent> {
    let _ = scenario;
    let mut intents = Vec::new();

    // Ritirata: armati sotto soglia HP + arty ferite (<50%: inefficaci, si
    // preservano). Mai i builder taskati sul sito. Il Commander ferito
    // ripiega come gli altri (capitale!). L'executor porta in copertura
    // torrette (fallback base).
    if personality.retreat_hp_frac > 0.0 {
        let mut low: Vec<Entity> = snapshot
            .my_units
            .iter()
            .filter(|u| {
                crate::units::archetype(u.kind).armed
                    && u.max_health > 0.0
                    && (u.health / u.max_health < personality.retreat_hp_frac
                        || (is_arty_role(u.kind) && u.health / u.max_health < WOUNDED_ARTY_FRAC))
                    && !matches!(u.order, UnitOrder::Build { .. })
            })
            .map(|u| u.entity)
            .collect();
        low.sort_by_key(|e| e.to_bits());
        if !low.is_empty() {
            intents.push(AiIntent::Retreat { units: low });
        }
    }

    // Focus fire a priorità minaccia (dps × counter contro il kind primario
    // proprio): solo quando dominante (win_prob oltre soglia), mai
    // inseguimenti suicidi. Fase A step 2: stima range-aware sul contatto
    // live (a contatto ≈ mischia, ma con artiglierie la gittata conta).
    // Snipe: Commander nemico in vista = designato a soglia ridotta (vale il
    // rischio); precede il focus normale.
    let (my_list, foe_list) = estimate_forces(snapshot);
    let win_prob =
        super::combat::predict_outcome_at_range(&my_list, &foe_list, engagement_range(snapshot));
    if personality.focus_fire && !snapshot.visible_enemies.is_empty() {
        if win_prob > SNIPE_MIN_WIN_PROB
            && let Some(snipe) = snipe_target(snapshot)
        {
            intents.push(AiIntent::FocusFire { target: snipe });
        } else if win_prob > FOCUS_MIN_WIN_PROB {
            let primary = my_primary_kind(snapshot);
            let target = snapshot
                .visible_enemies
                .iter()
                .max_by(|a, b| {
                    super::combat::target_priority(a.kind, primary)
                        .total_cmp(&super::combat::target_priority(b.kind, primary))
                        .then_with(|| b.health.total_cmp(&a.health))
                        .then_with(|| a.entity.to_bits().cmp(&b.entity.to_bits()))
                })
                .map(|e| e.entity);
            if let Some(target) = target {
                intents.push(AiIntent::FocusFire { target });
            }
        }
    }

    // Schermo + batteria: solo a contatto live e solo per unità ferme
    // (niente deviazioni dell'ondata in marcia), TRANNE il kite: le batterie
    // pressate arretrano anche in marcia (mai gli Attack del focus: la
    // pressione del focus resta). La batteria tiene la
    // gittata sul lato armata; lo schermo sta davanti alla batteria
    // (senza batteria: picchetto avanzato al 30% da guardia a minaccia).
    if let Some(threat) = visible_centroid(snapshot) {
        // Kite: per ogni batteria (ferma o in marcia, mai in focus/build),
        // se il nemico più vicino è dentro il 75% della gittata, meta di
        // arretramento sparando lungo la direttrice unità→nemico. Direzione
        // per-unità (non centroide): tiene anche contro fiancheggiatori.
        // Chi kita esce dalla lista Hold (niente doppio ordine).
        let mut kited: Vec<Entity> = Vec::new();
        let mut kites: Vec<(Entity, Vec3)> = Vec::new();
        for u in snapshot.my_units.iter().filter(|u| {
            crate::units::archetype(u.kind).armed
                && is_arty_role(u.kind)
                && u.kind != UnitKind::Scout
                && u.kind != UnitKind::Commander
                && u.max_health > 0.0
                && u.health / u.max_health >= WOUNDED_ARTY_FRAC
                && matches!(
                    u.order,
                    UnitOrder::Idle | UnitOrder::HoldPosition | UnitOrder::Move { .. }
                )
                && !matches!(u.order, UnitOrder::Build { .. })
        }) {
            let range = crate::units::archetype(u.kind).range;
            let mut near: Option<(f32, u64, Vec3)> = None;
            for e in &snapshot.visible_enemies {
                let d = u.pos.xz().distance_squared(e.pos.xz());
                let better = match near {
                    None => true,
                    Some((bd, bb, _)) => {
                        use std::cmp::Ordering as O;
                        match d.total_cmp(&bd) {
                            O::Less => true,
                            O::Greater => false,
                            O::Equal => e.entity.to_bits() < bb,
                        }
                    }
                };
                if better {
                    near = Some((d, e.entity.to_bits(), e.pos));
                }
            }
            if let Some((d_sq, _, npos)) = near
                && d_sq < (range * KITE_LINE_FRAC) * (range * KITE_LINE_FRAC)
            {
                kited.push(u.entity);
                kites.push((u.entity, kite_position(u.pos, npos, range)));
            }
        }
        kited.sort_by_key(|e| e.to_bits());
        kites.sort_by_key(|(e, _)| e.to_bits());
        if !kites.is_empty() {
            intents.push(AiIntent::Kite { moves: kites });
        }
        let mut battery: Vec<(Entity, f32)> = snapshot
            .my_units
            .iter()
            .filter(|u| {
                crate::units::archetype(u.kind).armed
                    && is_arty_role(u.kind)
                    && u.kind != UnitKind::Scout
                    && u.kind != UnitKind::Commander
                    && u.max_health > 0.0
                    && u.health / u.max_health >= WOUNDED_ARTY_FRAC
                    && matches!(u.order, UnitOrder::Idle | UnitOrder::HoldPosition)
                    && !matches!(u.order, UnitOrder::Build { .. })
                    && !kited.contains(&u.entity)
            })
            .map(|u| (u.entity, crate::units::archetype(u.kind).range))
            .collect();
        battery.sort_by_key(|(e, _)| e.to_bits());
        let hold_spot = if battery.is_empty() {
            None
        } else {
            let range = battery.iter().map(|(_, r)| *r).fold(0.0f32, f32::max);
            let units: Vec<Entity> = battery.iter().map(|(e, _)| *e).collect();
            let anchor = centroid_of(snapshot, &units).unwrap_or(threat);
            Some(hold_position(anchor, threat, range))
        };
        let mut melee: Vec<Entity> = snapshot
            .my_units
            .iter()
            .filter(|u| {
                crate::units::archetype(u.kind).armed
                    && !is_arty_role(u.kind)
                    && u.kind != UnitKind::Scout
                    && u.kind != UnitKind::Commander
                    && u.health > 0.0
                    && matches!(u.order, UnitOrder::Idle | UnitOrder::HoldPosition)
                    && !matches!(u.order, UnitOrder::Build { .. })
            })
            .map(|u| u.entity)
            .collect();
        melee.sort_by_key(|e| e.to_bits());
        if !melee.is_empty() {
            let guard = centroid_of(snapshot, &melee).unwrap_or(threat);
            let position = match hold_spot {
                Some(hold) => hold.lerp(threat, 0.35).with_y(0.0),
                None => screen_position(guard, threat),
            };
            intents.push(AiIntent::Screen {
                units: melee,
                position,
            });
        }
        if let Some(position) = hold_spot {
            let units: Vec<Entity> = battery.into_iter().map(|(e, _)| e).collect();
            intents.push(AiIntent::HoldAtMaxRange { units, position });
        }
    }

    intents
}

/// Baricentro delle unità elencate (dalle posizioni snapshot). `None` se
/// vuote o sparite. Puro.
fn centroid_of(snapshot: &AiSnapshot, entities: &[Entity]) -> Option<Vec3> {
    let mut sum = Vec3::ZERO;
    let mut n = 0u32;
    for u in &snapshot.my_units {
        if entities.contains(&u.entity) {
            sum += u.pos;
            n += 1;
        }
    }
    (n > 0).then(|| sum / n as f32)
}

/// Rally default riparato: 18m verso il fronte, ma mai dentro roccia o senza
/// clearance. Un rally murato blocca la factory PER SEMPRE (`free_exit`
/// richiede path porta->rally per tutte le porte e la AI salta le factory
/// bloccate): meglio nessun rally (fallback apron) che uno murato.
pub fn default_rally(grid: &crate::navigation::NavGrid, from: Vec3, target: Vec3) -> Option<Vec3> {
    let dir = (target - from).normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }
    let repaired = grid.clear_point_for(from + dir * 18.0, 0.5);
    (grid.is_walkable(repaired) && grid.has_clearance_for(repaired, 0.5)).then_some(repaired)
}

/// 0.0.18 — slot muro davanti alla torretta verso la minaccia: 3
/// caselle perpendicolari a 10m, snappate alla build grid (schermo che
/// rallenta, non sigillo). Puro e deterministico.
pub fn wall_slots(turret_pos: Vec3, threat_dir: Vec3) -> Vec<Vec3> {
    let mut dir = threat_dir;
    dir.y = 0.0;
    let dir = if dir.length_squared() > 1e-6 {
        dir.normalize()
    } else {
        Vec3::X
    };
    let side = Vec3::new(-dir.z, 0.0, dir.x);
    [-1.0, 0.0, 1.0]
        .into_iter()
        .map(|s| {
            let p = turret_pos + dir * 10.0 + side * (s * 2.0);
            snap_to_grid(p.with_y(0.0))
        })
        .collect()
}

/// 0.0.18 — muro mirato: prima torretta completa propria + slot
/// verso la minaccia che (a) stanno su `valid_ground`, (b) restano
/// raggiungibili dal builder, (c) non murano NESSUNA factory propria
/// (porte verificate sulla grid col muro aggiunto). Fallback: nessuno spot
/// (il chiamante salta il tick, mai muri a caso).
#[allow(clippy::too_many_arguments)]
pub fn find_wall_spot(
    grid: &crate::navigation::NavGrid,
    team: u8,
    buildings: &[(crate::units::Team, BuildingKind, Vec3, bool)],
    builder_pos: Vec3,
    units: &[(Vec3, f32)],
    threat_pos: Vec3,
    home: Vec3,
) -> Option<Vec3> {
    // Prima torretta completa (xz minima = deterministico).
    let turret = buildings
        .iter()
        .filter(|(t, k, _, site)| t.0 == team && *k == BuildingKind::Turret && !site)
        .map(|(_, _, p, _)| *p)
        .min_by(|a, b| a.x.total_cmp(&b.x).then_with(|| a.z.total_cmp(&b.z)))?;
    let mut dir = threat_pos - turret;
    dir.y = 0.0;
    let dir = if dir.length_squared() > 1.0 {
        dir.normalize()
    } else {
        (home - turret).normalize_or_zero()
    };
    for slot in wall_slots(turret, dir) {
        if valid_ground(grid, BuildingKind::Wall, slot, units).is_err() {
            continue;
        }
        if !approach_ok(grid, BuildingKind::Wall, slot, builder_pos) {
            continue;
        }
        // Porte factory proprie ancora libere col muro aggiunto.
        let probe = grid.cloned_with_obstacle(building_obstacle(BuildingKind::Wall, slot));
        let mut seals = false;
        for (t, k, p, _) in buildings {
            if t.0 != team {
                continue;
            }
            if factory_spawn_ok(&probe, *k, *p).is_err() {
                seals = true;
                break;
            }
        }
        if seals {
            continue;
        }
        return Some(slot);
    }
    None
}
/// Stand-off reale + path builder (bordo footprint, non centro) sulla grid
/// CON il futuro edificio. Condiviso da spirale e spot Metal: stessa garanzia
/// anti-tasche per entrambi (vedi doc di `find_build_spot`).
fn approach_ok(
    grid: &crate::navigation::NavGrid,
    kind: BuildingKind,
    point: Vec3,
    builder_pos: Vec3,
) -> bool {
    // Raggio scafo conservativo (commander 1.4) così vale per tutti.
    // Validato sulla grid CON il futuro edificio: il suo stesso ostacolo può
    // sigillare l'approach (lab grandi in basi dense). Body-aware: lo scafo
    // grande deve poter eseguire il percorso, non solo il margine scout.
    let probe = grid.cloned_with_obstacle(building_obstacle(kind, point));
    let approach = site_approach(&probe, point, builder_pos, kind.stats().half, 1.4);
    probe.find_path_for(builder_pos, approach, 1.4).is_some()
}

/// G1 — spot Metal: libero più vicino al builder (parità → mult maggiore,
/// poi xz), con le stesse garanzie della spirale (`valid_ground` + approach).
/// Occupati = Metal vivi di qualsiasi team (conteso = chiuso). Mai spirale:
/// la regola è hard, senza spot niente Build (il chiamante salta il tick).
pub fn find_metal_spot(
    grid: &crate::navigation::NavGrid,
    deposits: &[crate::structures::MetalDeposit],
    metals: &[Vec3],
    builder_pos: Vec3,
    units: &[(Vec3, f32)],
) -> Option<Vec3> {
    let mut remaining = crate::structures::free_deposits(deposits, metals);
    while let Some(dep) = crate::structures::nearest_free(&remaining, &[], builder_pos) {
        remaining.retain(|d| d.pos != dep.pos);
        if valid_ground(grid, BuildingKind::Metal, dep.pos, units).is_err() {
            continue;
        }
        if approach_ok(grid, BuildingKind::Metal, dep.pos, builder_pos) {
            return Some(dep.pos);
        }
    }
    None
}

/// Ricerca deterministica dello spot edificabile: spirale dal centro base
/// (0.0.18: per le torrette, prima spirale sull'anchor hotspot se dato).
/// Ritorna il primo punto con `valid_ground` + `placement_rule` + path dal
/// builder — e per le Factory anche una porta d'uscita libera, così le truppe
/// in coda spawnano sempre (niente lab murati vivi).
/// Il path è validato verso lo stand-off sul bordo (`site_approach`), cioè
/// dove il builder andrà davvero: validare il centro darebbe falsi positivi
/// (il centro è libero finché l'edificio non esiste) e tasche irraggiungibili
/// manderebbero l'ordine Build in loop MoveTarget/fail a ogni frame.
#[allow(clippy::too_many_arguments)]
pub fn find_build_spot(
    grid: &crate::navigation::NavGrid,
    team: u8,
    kind: BuildingKind,
    scenario: Scenario,
    builder_pos: Vec3,
    buildings: &[(crate::units::Team, BuildingKind, Vec3, bool)],
    builders: &[(crate::units::Team, Vec3, f32)],
    units: &[(Vec3, f32)],
    anchor: Option<Vec3>,
) -> Option<Vec3> {
    use std::f32::consts::PI;
    let base = scenario.center(team as usize);
    // 0.0.18 — torrette: prima spirale sull'anchor (hotspot minaccia),
    // poi sulla base come fallback. Altri edifici sempre dalla base.
    let mut centers = vec![base];
    if kind == BuildingKind::Turret
        && let Some(a) = anchor
        && a.distance_squared(base) > 1.0
    {
        centers.insert(0, a);
    }
    // Raggi crescenti deterministici dalla base: prima vicino (difendibile),
    // poi espansione. Angoli fissi 16 per anello = ordine stabile.
    for base in centers {
        for radius in [10.0, 14.0, 18.0, 24.0, 32.0, 42.0, 56.0] {
            for step in 0..16 {
                let angle = step as f32 / 16.0 * 2.0 * PI;
                let ideal = base + Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius);
                let point = snap_to_grid(ideal.with_y(0.0));
                if valid_ground(grid, kind, point, units).is_err() {
                    continue;
                }
                // Impronte esistenti (incluse quelle aperte in questo stesso
                // tick, fuse via `probe` dall'executor): mai due edifici
                // sovrapposti, mai due cantieri sullo stesso spot.
                let clash = buildings.iter().any(|(_, bk, bp, _)| {
                    let bh = bk.stats().half;
                    let kh = kind.stats().half;
                    (bp.x - point.x).abs() < bh.x + kh.x && (bp.z - point.z).abs() < bh.y + kh.y
                });
                if clash {
                    continue;
                }
                if factory_spawn_ok(grid, kind, point).is_err() {
                    continue; // porte murate: la spirale cerca un punto libero
                }
                if placement_rule(crate::units::Team(team), point, buildings, builders).is_err() {
                    continue; // nessun builder vivo: inutile cercare oltre
                }
                // Stand-off reale del builder (bordo footprint, non centro):
                // raggio scafo conservativo (commander 1.4) così vale per tutti.
                if !approach_ok(grid, kind, point, builder_pos) {
                    continue;
                }
                return Some(point);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::UnitOrder;
    use std::collections::BTreeMap;

    fn home_deposit() -> crate::structures::MetalDeposit {
        // G1: mondo con un deposito libero (i test che vogliono lo spot
        // occupato lo coprono con edifici nemici/propri sopra).
        crate::structures::MetalDeposit {
            pos: Vec3::new(30.0, 0.0, 0.0),
            mult: 1.0,
        }
    }

    fn empty_snapshot(team: u8) -> AiSnapshot {
        AiSnapshot {
            team,
            deposits: vec![home_deposit()],
            ..Default::default()
        }
    }

    #[test]
    fn power_weights_tank_over_scout_and_ignores_engineer() {
        let tank = combat_power(UnitKind::HeavyTank, 120.0);
        let scout = combat_power(UnitKind::Scout, 60.0);
        assert!(tank > scout);
        assert_eq!(combat_power(UnitKind::Engineer, 70.0), 0.0);
        assert_eq!(combat_power(UnitKind::HeavyTank, 0.0), 0.0);
    }

    #[test]
    fn default_rally_never_points_into_rock() {
        use crate::navigation::{CELL_SIZE, HALF_SIZE, NavGrid, Obstacle};
        // Muro largo 48m tra factory e fronte: il rally grezzo ci finirebbe dentro.
        let grid = NavGrid::new(
            HALF_SIZE,
            CELL_SIZE,
            vec![Obstacle {
                center: Vec2::new(0.0, 0.0),
                half_size: Vec2::new(24.0, 4.0),
            }],
        );
        let from = Vec3::new(-40.0, 0.0, 0.0);
        let target = Vec3::new(100.0, 0.0, 0.0);
        let rally = default_rally(&grid, from, target).expect("rally riparato");
        assert!(grid.is_walkable(rally) && grid.has_clearance_for(rally, 0.5));
        // Punta ancora verso il fronte (non dietro).
        assert!(rally.x > from.x);
        // Senza direzione (factory sul target): niente rally, fallback apron.
        assert_eq!(default_rally(&grid, target, target), None);
        // Campo aperto: il default passa liscio a ~18m.
        let open = NavGrid::new(HALF_SIZE, CELL_SIZE, vec![]);
        let rally = default_rally(&open, from, target).unwrap();
        assert!((rally.x - (from.x + 18.0)).abs() < 3.0);
    }

    #[test]
    fn build_order_metal_solar_factory_in_sequence() {
        let mut snap = empty_snapshot(1);
        idle_commander(&mut snap);
        // Commander + Engineer liberi: bootstrap parallelo, stessa specie.
        idle_engineer(&mut snap);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert_eq!(
            intents,
            vec![
                AiIntent::Build(BuildingKind::Metal),
                AiIntent::Build(BuildingKind::Metal)
            ]
        );
    }

    #[test]
    fn rusher_attacks_earlier_than_turtle() {
        use crate::units::archetype;
        // 2 tank full vs 1 tank nemico visto.
        let mut snap = empty_snapshot(1);
        for i in 0..2 {
            snap.my_units.push(super::super::snapshot::AiUnit {
                entity: Entity::from_bits(100 + i),
                pos: Vec3::ZERO,
                kind: UnitKind::HeavyTank,
                order: UnitOrder::Idle,
                health: archetype(UnitKind::HeavyTank).max_health,
                max_health: archetype(UnitKind::HeavyTank).max_health,
            });
        }
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: archetype(UnitKind::HeavyTank).max_health,
        });
        // Completa eco per non generare Build che oscurano il test.
        for (i, k) in [
            BuildingKind::Metal,
            BuildingKind::Solar,
            BuildingKind::Factory,
        ]
        .into_iter()
        .enumerate()
        {
            snap.my_buildings.push(super::super::snapshot::AiBuilding {
                entity: Entity::from_bits(500 + i as u64),
                kind: k,
                pos: Vec3::ZERO,
                under_construction: false,
                health: 100.0,
            });
        }
        // Second solar per turtle ancora mancante: rimuovi l'intento build
        // aggiungendo il secondo solar così resta solo la decisione attacco.
        snap.my_buildings.push(super::super::snapshot::AiBuilding {
            entity: Entity::from_bits(600),
            kind: BuildingKind::Solar,
            pos: Vec3::ZERO,
            under_construction: false,
            health: 100.0,
        });
        let _ = BTreeMap::<u8, ()>::new();
        let rush = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        let turtle = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            rush.iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. }))
        );
        // Turtle con 2 tank sotto soglia 4: non attacca.
        assert!(
            !turtle
                .iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. }))
        );
    }

    #[test]
    fn never_enqueues_commander() {
        let p = Personality {
            mix: [
                (UnitKind::Commander, 1),
                (UnitKind::HeavyTank, 1),
                (UnitKind::LightTank, 1),
            ],
            ..Personality::RUSHER
        };
        let snap = empty_snapshot(1);
        let intents = decide(
            &snap,
            &p,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        // Commander non è PRODUCIBLE: nessun Enqueue generato.
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Enqueue { .. }))
        );
    }

    fn fac(
        bits: u64,
        queue_len: usize,
        blocked: bool,
        tier: u8,
        queued: &[UnitKind],
    ) -> FactoryView {
        FactoryView {
            entity: Entity::from_bits(bits),
            queue_len,
            blocked,
            tier,
            queued: queued.to_vec(),
        }
    }

    fn enqueue_kind(intents: &[AiIntent]) -> Option<UnitKind> {
        intents.iter().find_map(|i| match i {
            AiIntent::Enqueue { kind, .. } => Some(*kind),
            _ => None,
        })
    }

    #[test]
    fn mix_chases_target_shares() {
        // 0.0.19 — due Scout vivi (cap max, niente ramo scout): a conteggi
        // vuoti vince il peso maggiore, poi il deficit guida le scelte.
        let scouts = &[
            (UnitKind::Scout, 60.0, UnitOrder::Idle),
            (UnitKind::Scout, 60.0, UnitOrder::Idle),
        ];
        let snap = armed_snapshot(1, scouts);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::HeavyTank));
        // 6 heavy saturano il 60%: tocca al LightTank (0/3).
        let mut heavies: Vec<(UnitKind, f32, UnitOrder)> =
            scouts.iter().map(|(k, h, o)| (*k, *h, o.clone())).collect();
        let max = crate::units::archetype(UnitKind::HeavyTank).max_health;
        for _ in 0..6 {
            heavies.push((UnitKind::HeavyTank, max, UnitOrder::Idle));
        }
        let snap = armed_snapshot(1, &heavies);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::LightTank));
    }

    #[test]
    fn tech_pick_reacts_to_comp_not_noise() {
        use super::super::opponent::OpponentKind;
        let max_l = crate::units::archetype(UnitKind::LightTank).max_health;
        let max_h = crate::units::archetype(UnitKind::HeavyTank).max_health;
        let lights: Vec<(UnitKind, f32)> = vec![(UnitKind::LightTank, max_l); 3];
        let heavies: Vec<(UnitKind, f32)> = vec![(UnitKind::HeavyTank, max_h); 2];
        // Sciame Light → MgTank (1.15); corazze → MortarTank (1.1).
        assert_eq!(
            tech_pick(
                &Personality::TURTLE.tech_mix,
                &lights,
                OpponentKind::Unknown,
                0,
                2
            ),
            Some(UnitKind::MgTank)
        );
        assert_eq!(
            tech_pick(
                &Personality::TURTLE.tech_mix,
                &heavies,
                OpponentKind::Unknown,
                0,
                2
            ),
            Some(UnitKind::MortarTank)
        );
        // Ignoto / cap pieno / pesi 0 → None = mix normale.
        assert_eq!(
            tech_pick(
                &Personality::TURTLE.tech_mix,
                &[],
                OpponentKind::Unknown,
                0,
                2
            ),
            None
        );
        assert_eq!(
            tech_pick(
                &Personality::TURTLE.tech_mix,
                &lights,
                OpponentKind::Unknown,
                2,
                2
            ),
            None
        );
        assert_eq!(
            tech_pick(
                &Personality::ECO_ONLY.tech_mix,
                &lights,
                OpponentKind::Unknown,
                0,
                0
            ),
            None
        );
    }

    #[test]
    fn turtle_builds_mg_vs_light_swarm() {
        use crate::units::archetype;
        // 2 engineer (cap) + 2 scout (cap, niente ramo occhi) + 3 light visti
        // + factory T1 libera → tech reattivo MgTank, non Heavy.
        let max_e = archetype(UnitKind::Engineer).max_health;
        let max_s = archetype(UnitKind::Scout).max_health;
        let max_l = archetype(UnitKind::LightTank).max_health;
        let kinds = &[
            (UnitKind::Engineer, max_e, UnitOrder::Idle),
            (UnitKind::Engineer, max_e, UnitOrder::Idle),
            (UnitKind::Scout, max_s, UnitOrder::Idle),
            (UnitKind::Scout, max_s, UnitOrder::Idle),
        ];
        let mut snap = armed_snapshot(1, kinds);
        for i in 0..3 {
            snap.visible_enemies.push(super::super::snapshot::AiEnemy {
                entity: Entity::from_bits(900 + i),
                pos: Vec3::new(10.0, 0.0, 0.0),
                kind: UnitKind::LightTank,
                health: max_l,
            });
        }
        let intents = decide(
            &snap,
            &Personality::TURTLE,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::MgTank));
        // Stesso ma 2 Mg vivi (cap tech): torna il mix normale (Heavy).
        let mut capped_kinds: Vec<(UnitKind, f32, UnitOrder)> =
            kinds.iter().map(|(k, h, o)| (*k, *h, o.clone())).collect();
        let max_mg = archetype(UnitKind::MgTank).max_health;
        capped_kinds.push((UnitKind::MgTank, max_mg, UnitOrder::Idle));
        capped_kinds.push((UnitKind::MgTank, max_mg, UnitOrder::Idle));
        let mut capped = armed_snapshot(1, &capped_kinds);
        for i in 0..3 {
            capped
                .visible_enemies
                .push(super::super::snapshot::AiEnemy {
                    entity: Entity::from_bits(910 + i),
                    pos: Vec3::new(10.0, 0.0, 0.0),
                    kind: UnitKind::LightTank,
                    health: max_l,
                });
        }
        let intents = decide(
            &capped,
            &Personality::TURTLE,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::HeavyTank));
    }

    #[test]
    fn blind_single_scout_builds_second_up_to_cap() {
        // 0.0.19 — max 2 scout (Welsh): con uno Scout vivo ma ancora alla
        // cieca, la factory accoda il secondo; con due, passa ai muscoli.
        let one = &[(UnitKind::Scout, 60.0, UnitOrder::Idle)];
        let snap = armed_snapshot(1, one);
        assert!(!has_fresh_eyes(&snap));
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::Scout));
        let two = &[
            (UnitKind::Scout, 60.0, UnitOrder::Idle),
            (UnitKind::Scout, 60.0, UnitOrder::Idle),
        ];
        let snap = armed_snapshot(1, two);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::HeavyTank));
    }

    #[test]
    fn t2_units_only_from_labt2() {
        use crate::units::archetype;
        let mut snap = armed_snapshot(1, &[(UnitKind::Scout, 60.0, UnitOrder::Idle)]);
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: archetype(UnitKind::HeavyTank).max_health,
        });
        // Factory T1: solo mix T1, mai T2 anche con occhi.
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        let kind = enqueue_kind(&intents).unwrap();
        assert_eq!(archetype(kind).tier, 1);
        // LabT2: mix pesante T2, prima scelta HeavyTank2.
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(2, 0, false, 2, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::HeavyTank2));
    }

    #[test]
    fn labt2_needs_time_and_factory() {
        let mut snap = armed_snapshot(1, &[]);
        idle_commander(&mut snap);
        // 0.0.18 — il LabT2 vuole eco vera (2 Metal + 2 Solar di income),
        // non solo il tick: la soglia temporale da sola non basta più.
        snap.income = [10.0, 24.0];
        snap.tick = 0;
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::LabT2)))
        );
        snap.tick = LABT2_MIN_TICK;
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::LabT2)));
    }

    #[test]
    fn turret_defense_is_turtle_only_and_capped() {
        let snap = armed_snapshot(1, &[]);
        let turtle = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(turtle.contains(&AiIntent::Build(BuildingKind::Turret)));
        let rush = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            !rush
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Turret)))
        );
        // Cap raggiunto (2 torrette): basta richiederne.
        let mut capped = armed_snapshot(1, &[]);
        for i in 0..2 {
            capped
                .my_buildings
                .push(super::super::snapshot::AiBuilding {
                    entity: Entity::from_bits(700 + i),
                    kind: BuildingKind::Turret,
                    pos: Vec3::ZERO,
                    under_construction: false,
                    health: 100.0,
                });
        }
        let intents = decide(&capped, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Turret)))
        );
    }

    fn armed_snapshot(team: u8, kinds_hp: &[(UnitKind, f32, UnitOrder)]) -> AiSnapshot {
        use crate::units::archetype;
        let mut snap = empty_snapshot(team);
        for (i, (kind, hp, order)) in kinds_hp.iter().enumerate() {
            let max = archetype(*kind).max_health;
            snap.my_units.push(super::super::snapshot::AiUnit {
                entity: Entity::from_bits(100 + i as u64),
                pos: Vec3::ZERO,
                kind: *kind,
                order: order.clone(),
                health: *hp,
                max_health: max,
            });
        }
        // Eco completa: niente intenti Build a disturbare i test micro.
        for (i, k) in [
            BuildingKind::Metal,
            BuildingKind::Solar,
            BuildingKind::Factory,
            BuildingKind::Solar,
        ]
        .into_iter()
        .enumerate()
        {
            snap.my_buildings.push(super::super::snapshot::AiBuilding {
                entity: Entity::from_bits(500 + i as u64),
                kind: k,
                pos: Vec3::ZERO,
                under_construction: false,
                health: 100.0,
            });
        }
        snap
    }

    #[test]
    fn retreat_triggers_below_threshold_but_not_on_site() {
        use crate::units::archetype;
        let max = archetype(UnitKind::HeavyTank).max_health;
        // Tank al 20% (< 0.25 rusher e < 0.35 turtle): ritirata per entrambi.
        let snap = armed_snapshot(1, &[(UnitKind::HeavyTank, max * 0.2, UnitOrder::Idle)]);
        for p in [Personality::TURTLE, Personality::RUSHER] {
            let intents = decide_micro(&snap, &p, Scenario::Playground);
            assert!(
                intents
                    .iter()
                    .any(|i| matches!(i, AiIntent::Retreat { .. })),
                "{p:?} dovrebbe ritirare il tank al 20%"
            );
        }
        // Tank sano: nessuna ritirata.
        let healthy = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
        let intents = decide_micro(&healthy, &Personality::TURTLE, Scenario::Playground);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Retreat { .. }))
        );
        // Builder ferito ma taskato sul sito: mai mollare il cantiere.
        let site = Entity::from_bits(777);
        let tasked = armed_snapshot(
            1,
            &[(
                UnitKind::Commander,
                archetype(UnitKind::Commander).max_health * 0.1,
                UnitOrder::Build { site },
            )],
        );
        let intents = decide_micro(&tasked, &Personality::TURTLE, Scenario::Playground);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Retreat { .. }))
        );
    }

    #[test]
    fn focus_fire_picks_weakest_only_when_dominant() {
        use crate::units::archetype;
        let max = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
            ],
        );
        // Due nemici: il più debole (bits alti) va designato.
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(901),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max * 0.9,
        });
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(902),
            pos: Vec3::new(12.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max * 0.3,
        });
        let intents = decide_micro(&snap, &Personality::RUSHER, Scenario::Playground);
        assert_eq!(
            intents.iter().find_map(|i| match i {
                AiIntent::FocusFire { target } => Some(*target),
                _ => None,
            }),
            Some(Entity::from_bits(902))
        );
        // Potenza pari (1 tank vs 1 tank): niente focus fire suicida.
        let mut even = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
        even.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(903),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max,
        });
        let intents = decide_micro(&even, &Personality::RUSHER, Scenario::Playground);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::FocusFire { .. }))
        );
    }

    #[test]
    fn courage_blocks_suicide_but_allows_dominance() {
        use crate::units::archetype;
        let max = archetype(UnitKind::HeavyTank).max_health;
        let enemy = |bits: u64| super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(bits),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max,
        };
        // 1 vs 4 a occhi aperti: win_prob ~0, niente ondata suicida.
        let mut weak = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
        for i in 0..4 {
            weak.visible_enemies.push(enemy(900 + i));
        }
        // Eco completa ma niente torrette: turtle emetterebbe Build(Turret),
        // il rusher no — l'assert resta solo sull'attacco.
        let intents = decide(&weak, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. }))
        );
        // 4 vs 1: win_prob ~1, l'ondata parte.
        let mut strong = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
            ],
        );
        strong.visible_enemies.push(enemy(910));
        let intents = decide(&strong, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            intents
                .iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. }))
        );
    }

    #[test]
    fn counter_weights_shift_production_to_lights_vs_artillery() {
        use crate::units::archetype;
        // 6H/3L/1A vivi: a pesi puri è triplo pareggio (1.0) e vincerebbe il
        // primo (Heavy); col counter Light-vs-Arty 1.25 tocca ai Light.
        let max_h = archetype(UnitKind::HeavyTank).max_health;
        let max_l = archetype(UnitKind::LightTank).max_health;
        let max_a = archetype(UnitKind::Artillery).max_health;
        let mut kinds: Vec<(UnitKind, f32, UnitOrder)> = Vec::new();
        for _ in 0..6 {
            kinds.push((UnitKind::HeavyTank, max_h, UnitOrder::Idle));
        }
        for _ in 0..3 {
            kinds.push((UnitKind::LightTank, max_l, UnitOrder::Idle));
        }
        kinds.push((UnitKind::Artillery, max_a, UnitOrder::Idle));
        let mut snap = armed_snapshot(1, &kinds);
        for i in 0..3 {
            snap.visible_enemies.push(super::super::snapshot::AiEnemy {
                entity: Entity::from_bits(920 + i),
                pos: Vec3::new(10.0 + i as f32, 0.0, 0.0),
                kind: UnitKind::Artillery,
                health: max_a,
            });
        }
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::LightTank));
    }

    #[test]
    fn focus_fire_prefers_guns_over_screen() {
        use crate::units::archetype;
        // Dominanza + screen di Light davanti all'Arty a pieni hp: la vecchia
        // logica (hp minori) designerebbe il Light, la priorità minaccia
        // (dps × counter vs Heavy primario) designa l'Arty.
        let max = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
            ],
        );
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(901),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::LightTank,
            health: archetype(UnitKind::LightTank).max_health,
        });
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(902),
            pos: Vec3::new(12.0, 0.0, 0.0),
            kind: UnitKind::Artillery,
            health: archetype(UnitKind::Artillery).max_health,
        });
        let intents = decide_micro(&snap, &Personality::RUSHER, Scenario::Playground);
        assert_eq!(
            intents.iter().find_map(|i| match i {
                AiIntent::FocusFire { target } => Some(*target),
                _ => None,
            }),
            Some(Entity::from_bits(902))
        );
    }

    #[test]
    fn remembered_power_matches_threat_cell_on_fresh_contact() {
        use super::super::snapshot::AiMemory;
        // Stessa formula nei due moduli (potenza × decay × sconto): un singolo
        // ricordo fresco età 0 deve coincidere col max della threat map.
        let mut snap = empty_snapshot(1);
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: Vec3::new(50.0, 0.0, -30.0),
            age_ticks: 0,
            kind: Some(UnitKind::HeavyTank),
            hp: 100.0,
            building: false,
        });
        let power = remembered_enemy_power(&snap);
        let map = super::super::threat::build_threat(&snap);
        assert!(power > 0.0);
        assert!((power - map.max()).abs() < 0.001);
    }

    #[test]
    fn force_and_mix_deduplicate_live_memory_by_entity() {
        use super::super::snapshot::{AiEnemy, AiMemory};
        let live = Entity::from_bits(501);
        let remembered = Entity::from_bits(502);
        let mut snap = empty_snapshot(1);
        snap.visible_enemies.push(AiEnemy {
            entity: live,
            pos: Vec3::ZERO,
            kind: UnitKind::HeavyTank,
            health: 170.0,
        });
        snap.memory.extend([
            AiMemory {
                entity_bits: Some(live.to_bits()),
                pos: Vec3::ZERO,
                age_ticks: 0,
                kind: Some(UnitKind::HeavyTank),
                hp: 170.0,
                building: false,
            },
            AiMemory {
                entity_bits: Some(remembered.to_bits()),
                pos: Vec3::X * 20.0,
                age_ticks: 10,
                kind: Some(UnitKind::LightTank),
                hp: 100.0,
                building: false,
            },
        ]);

        let mix = foe_mix(&snap);
        assert_eq!(
            mix.iter().find(|(kind, _)| *kind == UnitKind::HeavyTank),
            Some(&(UnitKind::HeavyTank, 170.0))
        );
        assert_eq!(estimate_forces(&snap).1.len(), 2);
    }

    #[test]
    fn metal_without_free_spot_falls_through_to_solar() {
        // G1: unico deposito occupato dal nemico visibile → niente Metal,
        // la macro passa al Solare invece di intasarsi.
        let mut snap = empty_snapshot(1);
        idle_commander(&mut snap);
        snap.visible_enemy_buildings
            .push(super::super::snapshot::AiBuilding {
                entity: Entity::from_bits(800),
                kind: BuildingKind::Metal,
                pos: home_deposit().pos,
                under_construction: false,
                health: 100.0,
            });
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert_eq!(intents, vec![AiIntent::Build(BuildingKind::Solar)]);
    }

    #[test]
    fn metal_spot_picks_nearest_free_and_skips_occupied() {
        use crate::navigation::{CELL_SIZE, HALF_SIZE, NavGrid};
        use crate::structures::MetalDeposit;
        let grid = NavGrid::new(HALF_SIZE, CELL_SIZE, vec![]);
        let deps = vec![
            MetalDeposit {
                pos: Vec3::new(0.0, 0.0, 0.0),
                mult: 1.0,
            },
            MetalDeposit {
                pos: Vec3::new(100.0, 0.0, 0.0),
                mult: 2.0,
            },
        ];
        let builder = Vec3::new(10.0, 0.0, 0.0);
        // Libero più vicino.
        assert_eq!(
            find_metal_spot(&grid, &deps, &[], builder, &[]),
            Some(Vec3::new(0.0, 0.0, 0.0))
        );
        // Occupato → il lontano.
        assert_eq!(
            find_metal_spot(&grid, &deps, &[Vec3::new(1.0, 0.0, 0.0)], builder, &[]),
            Some(Vec3::new(100.0, 0.0, 0.0))
        );
        // Tutto occupato → niente.
        assert_eq!(
            find_metal_spot(
                &grid,
                &deps,
                &[Vec3::new(0.0, 0.0, 0.0), Vec3::new(100.0, 0.0, 0.0)],
                builder,
                &[]
            ),
            None
        );
    }

    #[test]
    fn labt2_without_income_stays_locked() {
        // 0.0.18 — tick ok ma eco a zero: niente LabT2 (l'eco T1 prima).
        let mut snap = armed_snapshot(1, &[]);
        snap.tick = LABT2_MIN_TICK;
        snap.income = [0.0, 0.0];
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::LabT2)))
        );
    }

    #[test]
    fn planner_picks_bigger_relative_deficit() {
        // Entrambi affamati: metallo 1.2×, energia 2.0× → vince il Solare
        // (la vecchia catena avrebbe preso il Metal per primo).
        let snap = eco_snapshot(1, 2, 1, 1, [10.0, 20.0], [12.0, 40.0]);
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert_eq!(intents, vec![AiIntent::Build(BuildingKind::Solar)]);
    }

    #[test]
    fn turret_spiral_uses_hotspot_anchor() {
        use crate::navigation::{CELL_SIZE, HALF_SIZE, NavGrid};
        // Campo aperto: primo anello a ovest dell'anchor (dentro la mappa),
        // non della base.
        let grid = NavGrid::new(HALF_SIZE, CELL_SIZE, vec![]);
        let base = Scenario::Playground.center(1);
        let anchor = base + Vec3::new(-100.0, 0.0, 0.0);
        let builders = [(crate::units::Team(1), base, 1.0)];
        let spot = find_build_spot(
            &grid,
            1,
            BuildingKind::Turret,
            Scenario::Playground,
            base,
            &[],
            &builders,
            &[],
            Some(anchor),
        );
        assert_eq!(spot, Some(anchor + Vec3::new(10.0, 0.0, 0.0)));
        // Senza anchor: stesso primo anello ma dalla base.
        let spot = find_build_spot(
            &grid,
            1,
            BuildingKind::Turret,
            Scenario::Playground,
            base,
            &[],
            &builders,
            &[],
            None,
        );
        assert_eq!(spot, Some(base + Vec3::new(10.0, 0.0, 0.0)));
    }

    #[test]
    fn wall_slots_form_ahead_screen() {
        // Torretta in (0,0), minaccia da +x: 3 slot a x=10, simmetrici in z.
        let slots = wall_slots(Vec3::ZERO, Vec3::X);
        assert_eq!(slots.len(), 3);
        for s in &slots {
            assert!((s.x - 10.0).abs() < 0.001, "{s:?}");
        }
        let mut zs: Vec<f32> = slots.iter().map(|s| s.z).collect();
        zs.sort_by(f32::total_cmp);
        assert_eq!(zs, vec![-2.0, 0.0, 2.0]);
        // Direzione nulla: fallback +x, mai NaN.
        let slots = wall_slots(Vec3::ZERO, Vec3::ZERO);
        assert!(slots.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn walls_need_a_turret_and_respect_cap() {
        use super::super::snapshot::AiBuilding;
        // Turtle + torretta completa: chiede il muro.
        let mut snap = armed_snapshot(1, &[]);
        snap.my_buildings.push(AiBuilding {
            entity: Entity::from_bits(700),
            kind: BuildingKind::Turret,
            pos: Vec3::ZERO,
            under_construction: false,
            health: 100.0,
        });
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::Wall)));
        // Al cap (3 muri): basta.
        for i in 0..3 {
            snap.my_buildings.push(AiBuilding {
                entity: Entity::from_bits(710 + i),
                kind: BuildingKind::Wall,
                pos: Vec3::new(20.0 + i as f32 * 4.0, 0.0, 0.0),
                under_construction: false,
                health: 100.0,
            });
        }
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Wall)))
        );
        // Rusher non mura mai.
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Wall)))
        );
    }

    #[test]
    fn wall_spot_needs_turret_leaves_factory_doors_open() {
        use crate::navigation::{CELL_SIZE, HALF_SIZE, NavGrid};
        use crate::units::Team;
        let grid = NavGrid::new(HALF_SIZE, CELL_SIZE, vec![]);
        let turret = Vec3::new(0.0, 0.0, 0.0);
        let mine = Team(1);
        let complete = false; // false = completa (no Construction)
        let buildings = vec![(mine, BuildingKind::Turret, turret, complete)];
        // Campo aperto: primo slot valido davanti (verso +x).
        let spot = find_wall_spot(
            &grid,
            1,
            &buildings,
            Vec3::new(0.0, 0.0, -10.0),
            &[],
            Vec3::new(100.0, 0.0, 0.0),
            Vec3::new(-260.0, 0.0, -260.0),
        );
        assert_eq!(spot, Some(Vec3::new(10.0, 0.0, -2.0)));
        // Senza torrette: niente muri mirati.
        assert_eq!(
            find_wall_spot(
                &grid,
                1,
                &[],
                Vec3::new(0.0, 0.0, -10.0),
                &[],
                Vec3::new(100.0, 0.0, 0.0),
                Vec3::new(-260.0, 0.0, -260.0),
            ),
            None
        );
        // Slot occupati da unità: niente (valid_ground li rifiuta tutti).
        let bodies = vec![
            (Vec3::new(10.0, 0.0, -2.0), 0.5),
            (Vec3::new(10.0, 0.0, 0.0), 0.5),
            (Vec3::new(10.0, 0.0, 2.0), 0.5),
        ];
        assert_eq!(
            find_wall_spot(
                &grid,
                1,
                &buildings,
                Vec3::new(0.0, 0.0, -10.0),
                &bodies,
                Vec3::new(100.0, 0.0, 0.0),
                Vec3::new(-260.0, 0.0, -260.0),
            ),
            None
        );
    }

    #[test]
    fn scouting_uses_frontier_and_memory() {
        use super::super::snapshot::AiMemory;
        // 0.0.19 — frontiera deterministica: a parità di esplorato la meta non
        // balla più col tick (il ciclo su 4 punti fissi mandava in roccia).
        let mut snap = armed_snapshot(1, &[(UnitKind::Scout, 60.0, UnitOrder::Idle)]);
        snap.tick = 0;
        let early = scout_destination(&snap, Scenario::Playground);
        snap.tick = 8;
        let later = scout_destination(&snap, Scenario::Playground);
        assert_eq!(early, later);
        // Frontiera diversa se l'esplorato cambia: cella vista esclusa.
        snap.explored_cells = vec![
            false;
            super::super::snapshot::EXPLORED_GRID_N
                * super::super::snapshot::EXPLORED_GRID_N
        ];
        let blind = scout_destination(&snap, Scenario::Playground);
        // Marca la cella della meta come vista → la meta si sposta.
        {
            use crate::navigation::HALF_SIZE;
            let n = super::super::snapshot::EXPLORED_GRID_N;
            let side = (HALF_SIZE * 2.0) / n as f32;
            let col = (((blind.x + HALF_SIZE) / side).floor() as usize).min(n - 1);
            let row = (((blind.z + HALF_SIZE) / side).floor() as usize).min(n - 1);
            snap.explored_cells[row * n + col] = true;
        }
        let moved = scout_destination(&snap, Scenario::Playground);
        assert_ne!(blind, moved);
        // Ricordo fresco: lo scout va a confermare lì (come 0.0.14).
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: Vec3::new(50.0, 0.0, -30.0),
            age_ticks: 10,
            kind: Some(UnitKind::HeavyTank),
            hp: 100.0,
            building: false,
        });
        assert_eq!(
            scout_destination(&snap, Scenario::Playground),
            Vec3::new(50.0, 0.0, -30.0)
        );
        // Attacco su ricordi-truppa quando la vista è vuota.
        assert_eq!(
            attack_destination(&snap, Scenario::Playground),
            Vec3::new(50.0, 0.0, -30.0)
        );
    }

    #[test]
    fn building_memory_redirects_attack() {
        use super::super::snapshot::AiMemory;
        // 0.0.19 — base nemica ricordata (edifici freschi, vista vuota) batte
        // il target statico: marcia su posizioni verificate.
        let mut snap = armed_snapshot(1, &[]);
        let static_target = Scenario::Playground.attack_target(1);
        // Senza memoria: target statico.
        assert_eq!(
            attack_destination(&snap, Scenario::Playground),
            static_target
        );
        // Con edificio ricordato fresco altrove: la meta si sposta lì.
        let base_seen = Vec3::new(200.0, 0.0, 180.0);
        assert!((base_seen - static_target).length() > 50.0);
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: base_seen,
            age_ticks: 10,
            kind: None,
            hp: 450.0,
            building: true,
        });
        assert_eq!(attack_destination(&snap, Scenario::Playground), base_seen);
        // Ricordo stantio oltre il fresh: si torna allo statico.
        snap.memory[0].age_ticks = MEMORY_FRESH_TICKS + 1;
        assert_eq!(
            attack_destination(&snap, Scenario::Playground),
            static_target
        );
    }

    #[test]
    fn scout_intent_is_free_from_army_count() {
        use crate::units::archetype;
        // 0.0.19 — fix ramo morto: lo scout esce anche con l'esercito vivo
        // (prima `army_count == 0` impossibile col Commander armato).
        let max = archetype(UnitKind::HeavyTank).max_health;
        let snap = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::Scout, 60.0, UnitOrder::Idle),
            ],
        );
        assert!(!has_fresh_eyes(&snap));
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            intents.iter().any(|i| matches!(i, AiIntent::Scout { .. })),
            "scout cieco con armata viva deve uscire: {intents:?}"
        );
    }

    #[test]
    fn scouts_patrol_distinct_frontiers_with_eyes_on() {
        use crate::units::archetype;
        // 0.0.19 — pattuglia continua: con occhi aperti ma scout liberi, gli
        // scout mappano (frontiere distinte) invece di marcire in base; non
        // si immolano nella battaglia live (meta ≠ nemico visibile).
        let max = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::Scout, 60.0, UnitOrder::Idle),
                (UnitKind::Scout, 60.0, UnitOrder::Idle),
            ],
        );
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max,
        });
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        let scouts: Vec<Vec3> = intents
            .iter()
            .filter_map(|i| match i {
                AiIntent::Scout { destination } => Some(*destination),
                _ => None,
            })
            .collect();
        assert_eq!(scouts.len(), 2, "due scout liberi = due mete: {intents:?}");
        assert!(
            scouts[0].distance(scouts[1]) > 9.0,
            "frontiere distinte, mai in pila: {scouts:?}"
        );
        for dest in &scouts {
            assert!(
                dest.distance(Vec3::new(10.0, 0.0, 0.0)) > 9.0,
                "niente immolazione sul live: {dest:?}"
            );
        }
    }

    #[test]
    fn scout_diverts_from_hot_route() {
        use crate::units::archetype;
        // 0.0.19 — scout in rotta verso una meta diventata calda (tank
        // avvistato lì dopo l'ordine): devia invece di immolarsi. Rotta
        // fredda: la finisce, nessun intento.
        let max = archetype(UnitKind::HeavyTank).max_health;
        let hot = Vec3::new(100.0, 0.0, 100.0);
        let mut snap = armed_snapshot(
            1,
            &[(UnitKind::Scout, 60.0, UnitOrder::Move { destination: hot })],
        );
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: hot,
            kind: UnitKind::HeavyTank,
            health: max,
        });
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            intents.iter().any(|i| matches!(i, AiIntent::Scout { .. })),
            "rotta calda: devia {intents:?}"
        );
        // Stesso nemico, rotta fredda altrove: nessun re-task.
        let mut cool = armed_snapshot(
            1,
            &[(
                UnitKind::Scout,
                60.0,
                UnitOrder::Move {
                    destination: Vec3::new(-100.0, 0.0, -100.0),
                },
            )],
        );
        cool.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: hot,
            kind: UnitKind::HeavyTank,
            health: max,
        });
        let intents = decide(&cool, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            !intents.iter().any(|i| matches!(i, AiIntent::Scout { .. })),
            "rotta fredda: la finisce {intents:?}"
        );
    }

    #[test]
    fn opponent_shift_moves_courage_and_mix() {
        use super::super::snapshot::{AiBuilding, AiMemory};
        use crate::units::archetype;
        // Vs rusher (massa precoce): courage effettivo scende di 0.1 e il mix
        // sposta verso Heavy/Arty (bias 1.1) contro Light (0.9).
        let max_h = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = armed_snapshot(1, &[(UnitKind::Scout, 60.0, UnitOrder::Idle)]);
        snap.tick = 200;
        for i in 0..3 {
            snap.memory.push(AiMemory {
                entity_bits: None,
                pos: Vec3::new(i as f32 * 5.0, 0.0, 0.0),
                age_ticks: 5,
                kind: Some(UnitKind::HeavyTank),
                hp: max_h,
                building: false,
            });
        }
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: Vec3::ZERO,
            age_ticks: 150,
            kind: Some(UnitKind::HeavyTank),
            hp: max_h,
            building: false,
        });
        assert_eq!(
            super::super::opponent::classify(&snap),
            super::super::opponent::OpponentKind::Rusher
        );
        assert!(
            (super::super::opponent::adjust_courage(
                0.55,
                super::super::opponent::OpponentKind::Rusher
            ) - 0.45)
                .abs()
                < 1e-6
        );
        // Vs turtle (torretta vista): courage sale e Arty bias 1.1.
        snap.visible_enemy_buildings.push(AiBuilding {
            entity: Entity::from_bits(700),
            kind: BuildingKind::Turret,
            pos: Vec3::new(200.0, 0.0, 200.0),
            under_construction: false,
            health: 450.0,
        });
        assert_eq!(
            super::super::opponent::classify(&snap),
            super::super::opponent::OpponentKind::Turtle
        );
        assert!(
            super::super::opponent::mix_bias(
                UnitKind::Artillery,
                super::super::opponent::OpponentKind::Turtle
            ) > super::super::opponent::mix_bias(
                UnitKind::LightTank,
                super::super::opponent::OpponentKind::Turtle
            )
        );
    }

    #[test]
    fn factory_builds_scout_when_blind() {
        // Factory completa, niente scout, niente occhi: tocca allo Scout.
        let snap = armed_snapshot(1, &[]);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[])],
            0,
        );
        assert_eq!(
            intents.iter().find_map(|i| match i {
                AiIntent::Enqueue { kind, .. } => Some(*kind),
                _ => None,
            }),
            Some(UnitKind::Scout)
        );
    }

    #[test]
    fn skips_blocked_factories() {
        // Coda libera ma porte ostruite (prodotto trattenuto): niente enqueue,
        // la factory si sblocca da sola quando le porte si liberano.
        let snap = armed_snapshot(1, &[]);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, true, 1, &[])],
            0,
        );
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Enqueue { .. }))
        );
    }

    fn idle_commander(snap: &mut AiSnapshot) {
        use crate::units::archetype;
        let max_c = archetype(UnitKind::Commander).max_health;
        snap.my_units.push(super::super::snapshot::AiUnit {
            entity: Entity::from_bits(99),
            pos: Vec3::ZERO,
            kind: UnitKind::Commander,
            order: UnitOrder::Idle,
            health: max_c,
            max_health: max_c,
        });
    }

    fn idle_engineer(snap: &mut AiSnapshot) {
        use crate::units::archetype;
        let max_e = archetype(UnitKind::Engineer).max_health;
        snap.my_units.push(super::super::snapshot::AiUnit {
            entity: Entity::from_bits(98),
            pos: Vec3::ZERO,
            kind: UnitKind::Engineer,
            order: UnitOrder::Idle,
            health: max_e,
            max_health: max_e,
        });
    }

    fn eco_snapshot(
        team: u8,
        metals: usize,
        solars: usize,
        factories: usize,
        income: [f64; 2],
        demand: [f64; 2],
    ) -> AiSnapshot {
        let mut snap = empty_snapshot(team);
        let mut bits = 500u64;
        for (kind, n) in [
            (BuildingKind::Metal, metals),
            (BuildingKind::Solar, solars),
            (BuildingKind::Factory, factories),
        ] {
            for _ in 0..n {
                snap.my_buildings.push(super::super::snapshot::AiBuilding {
                    entity: Entity::from_bits(bits),
                    kind,
                    pos: Vec3::ZERO,
                    under_construction: false,
                    health: 100.0,
                });
                bits += 1;
            }
        }
        snap.income = income;
        snap.demand = demand;
        // Un Commander libero: gli intenti Build scalano sui builder liberi.
        let max_c = crate::units::archetype(UnitKind::Commander).max_health;
        snap.my_units.push(super::super::snapshot::AiUnit {
            entity: Entity::from_bits(99),
            pos: Vec3::ZERO,
            kind: UnitKind::Commander,
            order: UnitOrder::Idle,
            health: max_c,
            max_health: max_c,
        });
        snap
    }

    fn give_fresh_eyes(snap: &mut AiSnapshot) {
        use super::super::snapshot::AiMemory;
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: Vec3::new(50.0, 0.0, -30.0),
            age_ticks: 5,
            kind: Some(UnitKind::HeavyTank),
            hp: 100.0,
            building: false,
        });
    }

    #[test]
    fn starved_metal_requests_metal_until_cap() {
        // Turtle affamata di metal (domanda > offerta): nuovo Metal.
        let snap = eco_snapshot(1, 1, 2, 1, [5.0, 24.0], [12.0, 10.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::Metal)));
        // Al cap (3): basta, anche se affamata.
        let snap = eco_snapshot(1, 3, 2, 1, [15.0, 24.0], [30.0, 10.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Metal)))
        );
        // Domanda soddisfatta: nessun nuovo Metal.
        let snap = eco_snapshot(1, 1, 2, 1, [5.0, 24.0], [4.0, 10.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Metal)))
        );
    }

    #[test]
    fn starved_energy_requests_solar_until_cap() {
        let snap = eco_snapshot(1, 2, 1, 1, [10.0, 12.0], [5.0, 30.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::Solar)));
        // Rusher al suo cap (2): basta.
        let snap = eco_snapshot(1, 2, 2, 1, [10.0, 24.0], [5.0, 60.0]);
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Solar)))
        );
    }

    #[test]
    fn planner_waits_when_broke_e2e() {
        // Debito Punto 5 — collo di bottiglia ma non abbordabile entro 60s
        // (income 0.5/s su costo 100: tta 200s = INF): niente Build, aspetta eco.
        // Stessa fame con income sano (5/s: tta 20s) costruisce.
        let broke = eco_snapshot(1, 1, 2, 1, [0.5, 24.0], [12.0, 10.0]);
        let intents = decide(&broke, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Metal))),
            "broke: aspetta eco {intents:?}"
        );
        let funded = eco_snapshot(1, 1, 2, 1, [5.0, 24.0], [12.0, 10.0]);
        let intents = decide(&funded, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::Metal)));
    }

    #[test]
    fn second_factory_needs_two_metals() {
        // Eco bilanciata ma un solo Metal: niente seconda lab.
        let snap = eco_snapshot(1, 1, 2, 1, [5.0, 24.0], [5.0, 10.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Factory)))
        );
        // Due Metal: via alla seconda (sotto il cap turtle di 2).
        let snap = eco_snapshot(1, 2, 2, 1, [10.0, 24.0], [9.0, 10.0]);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(intents.contains(&AiIntent::Build(BuildingKind::Factory)));
        // Rusher resta a una sola lab per scelta.
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::Build(BuildingKind::Factory)))
        );
    }

    #[test]
    fn engineer_and_scout_queues_count_toward_caps() {
        // Turtle: 1 engineer vivo + 1 accodato = cap raggiunto → tech reattivo
        // (ricordo fresco di Heavy: mortaio, non Heavy).
        let mut snap = armed_snapshot(1, &[(UnitKind::Engineer, 70.0, UnitOrder::Idle)]);
        give_fresh_eyes(&mut snap);
        let intents = decide(
            &snap,
            &Personality::TURTLE,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[UnitKind::Engineer])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::MortarTank));
        // Scout accodato ma non ancora uscito: niente doppione (stesso tech).
        let mut snap = armed_snapshot(1, &[]);
        give_fresh_eyes(&mut snap);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[UnitKind::Scout])],
            0,
        );
        assert_eq!(enqueue_kind(&intents), Some(UnitKind::MortarTank));
    }

    #[test]
    fn one_enqueue_per_free_factory() {
        // Due lab libere: due enqueue complementari (Heavy poi Light).
        let mut snap = armed_snapshot(1, &[(UnitKind::Scout, 60.0, UnitOrder::Idle)]);
        give_fresh_eyes(&mut snap);
        let intents = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[fac(1, 0, false, 1, &[]), fac(2, 0, false, 1, &[])],
            0,
        );
        let mut kinds: Vec<UnitKind> = intents
            .iter()
            .filter_map(|i| match i {
                AiIntent::Enqueue { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect();
        kinds.sort_by_key(|k| k.index());
        assert_eq!(kinds.len(), 2);
        let targets: Vec<Entity> = intents
            .iter()
            .filter_map(|i| match i {
                AiIntent::Enqueue { factory, .. } => Some(*factory),
                _ => None,
            })
            .collect();
        assert_ne!(targets[0], targets[1]);
    }

    #[test]
    fn eco_only_never_attacks_or_micros() {
        use crate::units::archetype;
        // Eco completa + esercito + nemico visibile: eco-only resta a casa.
        let mut snap = eco_snapshot(1, 3, 3, 2, [15.0, 36.0], [10.0, 10.0]);
        for i in 0..3 {
            snap.my_units.push(super::super::snapshot::AiUnit {
                entity: Entity::from_bits(100 + i),
                pos: Vec3::ZERO,
                kind: UnitKind::HeavyTank,
                order: UnitOrder::Idle,
                health: archetype(UnitKind::HeavyTank).max_health,
                max_health: archetype(UnitKind::HeavyTank).max_health,
            });
        }
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: archetype(UnitKind::HeavyTank).max_health,
        });
        let intents = decide(&snap, &Personality::ECO_ONLY, Scenario::Playground, &[], 0);
        assert!(!intents.iter().any(|i| matches!(
            i,
            AiIntent::AttackMoveGroup { .. } | AiIntent::FocusFire { .. }
        )));
    }

    #[test]
    fn rush_scripted_attacks_on_schedule_regardless_of_odds() {
        use crate::units::archetype;
        // Un solo heavy contro forza superiore: prima dello schedule, niente.
        let mut snap = eco_snapshot(1, 2, 1, 1, [10.0, 12.0], [9.0, 30.0]);
        // Builder inerme al posto del Commander: serve un builder libero per
        // gli intenti ma senza distorcere la stima forze (1200hp).
        snap.my_units.retain(|u| u.kind != UnitKind::Commander);
        let max_e = archetype(UnitKind::Engineer).max_health;
        snap.my_units.push(super::super::snapshot::AiUnit {
            entity: Entity::from_bits(98),
            pos: Vec3::ZERO,
            kind: UnitKind::Engineer,
            order: UnitOrder::Idle,
            health: max_e,
            max_health: max_e,
        });
        snap.my_units.push(super::super::snapshot::AiUnit {
            entity: Entity::from_bits(100),
            pos: Vec3::ZERO,
            kind: UnitKind::HeavyTank,
            order: UnitOrder::Idle,
            health: archetype(UnitKind::HeavyTank).max_health,
            max_health: archetype(UnitKind::HeavyTank).max_health,
        });
        for i in 0..4 {
            snap.visible_enemies.push(super::super::snapshot::AiEnemy {
                entity: Entity::from_bits(900 + i),
                pos: Vec3::new(10.0 + i as f32, 0.0, 0.0),
                kind: UnitKind::HeavyTank,
                health: archetype(UnitKind::HeavyTank).max_health,
            });
        }
        snap.tick = 0;
        let early = decide(
            &snap,
            &Personality::RUSH_SCRIPTED,
            Scenario::Playground,
            &[],
            0,
        );
        assert!(
            !early
                .iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. }))
        );
        // ...allo schedule: l'ondata parte comunque.
        snap.tick = Personality::RUSH_SCRIPTED.attack_at_tick;
        let late = decide(
            &snap,
            &Personality::RUSH_SCRIPTED,
            Scenario::Playground,
            &[],
            0,
        );
        assert!(
            late.iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. }))
        );
    }

    #[test]
    fn waves_tick_alignment() {
        // Il periodo è multiplo della cadenza strategia/snapshot (1Hz vede
        // ogni quarto tick a 4Hz): i lanci non si perdono mai.
        assert_eq!(WAVE_PERIOD_TICKS % 4, 0);
        assert!(is_wave_tick(0) && is_wave_tick(300) && is_wave_tick(600));
        assert!(!is_wave_tick(1) && !is_wave_tick(299) && !is_wave_tick(301));
        assert_eq!(wave_id(0), 0);
        assert_eq!(wave_id(299), 0);
        assert_eq!(wave_id(300), 1);
    }

    #[test]
    fn waves_monotonic() {
        use crate::units::archetype;
        // Stessa forza dominante: l'ondata parte ai tick d'ondata, mai fuori
        // (impulsi discreti ogni ~75s, rivalutati ogni volta).
        let max = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
            ],
        );
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max,
        });
        // Come `ai_tick`: last evolve solo quando l'ondata parte davvero.
        let mut last = 0u64;
        let mut has_group = |tick: u64| {
            let mut snap = snap.clone();
            snap.tick = tick;
            let fire = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], last)
                .iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. }));
            if fire {
                last = wave_id(tick);
            }
            fire
        };
        assert!(has_group(300));
        assert!(!has_group(301));
        assert!(!has_group(450));
        assert!(has_group(600));
        // Monotonia degli id d'ondata.
        assert!(wave_id(300) < wave_id(600));
    }

    #[test]
    fn wave_recalls_when_base_threatened() {
        use crate::units::archetype;
        // Nemico in casa (entro 120m da (260, 260)): a parità niente ondata,
        // il micro scherma a casa (richiamo difensivo).
        let max = archetype(UnitKind::HeavyTank).max_health;
        let raider = || super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(230.0, 0.0, 230.0),
            kind: UnitKind::HeavyTank,
            health: max,
        };
        let mut even = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
        even.visible_enemies.push(raider());
        assert!(base_under_threat(&even, Scenario::Playground));
        even.tick = 300;
        let intents = decide(&even, &Personality::RUSHER, Scenario::Playground, &[], 0);
        assert!(
            !intents
                .iter()
                .any(|i| matches!(i, AiIntent::AttackMoveGroup { .. })),
            "a parità minacciati l'ondata resta a casa: {intents:?}"
        );
        // Il micro invece scherma: linea a casa.
        let micro = decide_micro(&even, &Personality::RUSHER, Scenario::Playground);
        assert!(
            micro.iter().any(|i| matches!(i, AiIntent::Screen { .. })),
            "schermo a casa: {micro:?}"
        );
    }

    #[test]
    fn dominant_counter_pushes_onto_threat_at_home() {
        use crate::units::archetype;
        // Punto 2 — stessa minaccia in casa, ma dominanti 4v1: il richiamo non
        // è più un muro, si contrattacca SULLA minaccia (centroide visibile),
        // senza capitale (qui assente comunque).
        let max = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
                (UnitKind::HeavyTank, max, UnitOrder::Idle),
            ],
        );
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(230.0, 0.0, 230.0),
            kind: UnitKind::HeavyTank,
            health: max,
        });
        assert!(base_under_threat(&snap, Scenario::Playground));
        snap.tick = 300;
        let intents = decide(&snap, &Personality::RUSHER, Scenario::Playground, &[], 0);
        let dest = intents.iter().find_map(|i| match i {
            AiIntent::AttackMoveGroup { destination, .. } => Some(*destination),
            _ => None,
        });
        let dest = dest.expect("dominanti: contrattacco sulla minaccia");
        assert!(
            (dest.x - 230.0).abs() < 0.01 && (dest.z - 230.0).abs() < 0.01,
            "meta sul raider, non sulla base nemica: {dest:?}"
        );
    }

    #[test]
    fn commander_never_commits_early() {
        use crate::units::archetype;
        // Capitale + scorta con vittoria probabile ma non certa (0.6 < 0.9):
        // l'ondata parte senza Commander. A 0.99 il Commander si unisce, ma
        // mai da solo (scorta presente comunque).
        let max_h = archetype(UnitKind::HeavyTank).max_health;
        let max_c = archetype(UnitKind::Commander).max_health;
        let escort = |snap: &mut AiSnapshot| {
            snap.my_units.push(super::super::snapshot::AiUnit {
                entity: Entity::from_bits(101),
                pos: Vec3::ZERO,
                kind: UnitKind::HeavyTank,
                order: UnitOrder::Idle,
                health: max_h,
                max_health: max_h,
            });
            snap.my_units.push(super::super::snapshot::AiUnit {
                entity: Entity::from_bits(102),
                pos: Vec3::ZERO,
                kind: UnitKind::HeavyTank,
                order: UnitOrder::Idle,
                health: max_h,
                max_health: max_h,
            });
        };
        // wave_group diretto: soglia commit rusher 0.9, base libera.
        let mut snap = armed_snapshot(1, &[(UnitKind::Commander, max_c, UnitOrder::Idle)]);
        escort(&mut snap);
        let cautious = wave_group(&snap, &Personality::RUSHER, 0.6, false);
        assert_eq!(cautious.len(), 2);
        assert!(!cautious.contains(&Entity::from_bits(100)));
        let committed = wave_group(&snap, &Personality::RUSHER, 0.99, false);
        assert_eq!(committed.len(), 3);
        assert!(committed.contains(&Entity::from_bits(100)));
        // Base minacciata: il capitale non si gioca nelle mischie difensive,
        // anche oltre soglia (il torneo lo perdeva proprio lì).
        let defended = wave_group(&snap, &Personality::RUSHER, 1.0, true);
        assert_eq!(defended.len(), 2);
        assert!(!defended.contains(&Entity::from_bits(100)));
        // Solo capitale (niente scorta): mai ondata solitaria.
        let alone = armed_snapshot(1, &[(UnitKind::Commander, max_c, UnitOrder::Idle)]);
        assert!(wave_group(&alone, &Personality::RUSHER, 1.0, false).is_empty());
    }

    #[test]
    fn arty_holds_behind_screen() {
        use crate::units::archetype;
        // Linea + batteria ferme a contatto live: schermo davanti, arty in
        // gittata dietro (hold più lontano dalla minaccia dello screen).
        let max_h = archetype(UnitKind::HeavyTank).max_health;
        let max_a = archetype(UnitKind::Artillery).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max_h, UnitOrder::Idle),
                (UnitKind::Artillery, max_a, UnitOrder::Idle),
            ],
        );
        let foe = Vec3::new(100.0, 0.0, 0.0);
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: foe,
            kind: UnitKind::HeavyTank,
            health: max_h,
        });
        let micro = decide_micro(&snap, &Personality::RUSHER, Scenario::Playground);
        let screen_pos = micro.iter().find_map(|i| match i {
            AiIntent::Screen { position, .. } => Some(*position),
            _ => None,
        });
        let hold_pos = micro.iter().find_map(|i| match i {
            AiIntent::HoldAtMaxRange { position, .. } => Some(*position),
            _ => None,
        });
        let (screen_pos, hold_pos) = (screen_pos.expect("screen"), hold_pos.expect("hold"));
        // La batteria tiene la gittata con margine (~26m per 30m nominali),
        // lo schermo sta davanti alla batteria senza immolarsi sul nemico.
        assert!((hold_pos.distance(foe) - 26.0).abs() < 6.0);
        assert!(screen_pos.distance(foe) < hold_pos.distance(foe));
        assert!(screen_pos.distance(foe) > 5.0);
        // Arty ferita (<50%): niente hold, va in ritirata.
        let mut hurt = armed_snapshot(1, &[(UnitKind::Artillery, max_a * 0.4, UnitOrder::Idle)]);
        hurt.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(901),
            pos: foe,
            kind: UnitKind::HeavyTank,
            health: max_h,
        });
        let micro = decide_micro(&hurt, &Personality::RUSHER, Scenario::Playground);
        assert!(
            !micro
                .iter()
                .any(|i| matches!(i, AiIntent::HoldAtMaxRange { .. })),
            "arty ferita non tiene: {micro:?}"
        );
        assert!(
            micro.iter().any(|i| matches!(i, AiIntent::Retreat { .. })),
            "arty ferita ripiega: {micro:?}"
        );
    }

    #[test]
    fn kite_backoff_inside_line_hold_outside() {
        use crate::units::archetype;
        // Arty sola (30m) + heavy a 15m (< 22.5 linea): kita a ~26m dal
        // nemico lungo la direttrice, e niente Hold per lei.
        let max_a = archetype(UnitKind::Artillery).max_health;
        let max_h = archetype(UnitKind::HeavyTank).max_health;
        let foe = Vec3::new(15.0, 0.0, 0.0);
        let mut snap = armed_snapshot(1, &[(UnitKind::Artillery, max_a, UnitOrder::Idle)]);
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: foe,
            kind: UnitKind::HeavyTank,
            health: max_h,
        });
        let micro = decide_micro(&snap, &Personality::RUSHER, Scenario::Playground);
        let dest = micro.iter().find_map(|i| match i {
            AiIntent::Kite { moves } => Some(moves.clone()),
            _ => None,
        });
        let moves = dest.expect("kite quando pressata");
        assert_eq!(moves.len(), 1);
        let have = moves[0].1;
        // Meta di arretramento: ~26m dal nemico, più lontana di prima.
        assert!((have.distance(foe) - 26.0).abs() < 1.0, "{have:?}");
        assert!(have.distance(foe) > Vec3::ZERO.distance(foe));
        assert!(
            !micro
                .iter()
                .any(|i| matches!(i, AiIntent::HoldAtMaxRange { .. })),
            "chi kita non tiene: {micro:?}"
        );
        // Nemico a 28m (> linea): niente kite, Hold normale.
        let mut far = armed_snapshot(1, &[(UnitKind::Artillery, max_a, UnitOrder::Idle)]);
        far.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(901),
            pos: Vec3::new(28.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max_h,
        });
        let micro = decide_micro(&far, &Personality::RUSHER, Scenario::Playground);
        assert!(
            !micro.iter().any(|i| matches!(i, AiIntent::Kite { .. })),
            "fuori linea tiene: {micro:?}"
        );
        assert!(
            micro
                .iter()
                .any(|i| matches!(i, AiIntent::HoldAtMaxRange { .. })),
            "hold fuori linea: {micro:?}"
        );
    }

    #[test]
    fn snipe_designates_commander_without_suicide() {
        use crate::units::archetype;
        // Selezione pura: il Commander (hp minori) vince su chiunque.
        let max_h = archetype(UnitKind::HeavyTank).max_health;
        let mut snap = armed_snapshot(1, &[(UnitKind::HeavyTank, max_h, UnitOrder::Idle)]);
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(901),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max_h,
        });
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(902),
            pos: Vec3::new(12.0, 0.0, 0.0),
            kind: UnitKind::Commander,
            health: 150.0,
        });
        assert_eq!(snipe_target(&snap), Some(Entity::from_bits(902)));
        assert!(snipe_target(&armed_snapshot(1, &[])).is_none());
        // Dominanti 4v1 + Commander esposto: designato (snipe batte focus).
        let mut strong = armed_snapshot(
            1,
            &[
                (UnitKind::HeavyTank, max_h, UnitOrder::Idle),
                (UnitKind::HeavyTank, max_h, UnitOrder::Idle),
                (UnitKind::HeavyTank, max_h, UnitOrder::Idle),
                (UnitKind::HeavyTank, max_h, UnitOrder::Idle),
            ],
        );
        strong
            .visible_enemies
            .push(super::super::snapshot::AiEnemy {
                entity: Entity::from_bits(903),
                pos: Vec3::new(10.0, 0.0, 0.0),
                kind: UnitKind::Commander,
                health: 150.0,
            });
        let micro = decide_micro(&strong, &Personality::RUSHER, Scenario::Playground);
        assert_eq!(
            micro.iter().find_map(|i| match i {
                AiIntent::FocusFire { target } => Some(*target),
                _ => None,
            }),
            Some(Entity::from_bits(903))
        );
        // Debole 1v1+Commander full: silenzio, niente suicidi di prestigio.
        let mut weak = armed_snapshot(1, &[(UnitKind::HeavyTank, max_h, UnitOrder::Idle)]);
        weak.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(904),
            pos: Vec3::new(10.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max_h,
        });
        weak.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(905),
            pos: Vec3::new(12.0, 0.0, 0.0),
            kind: UnitKind::Commander,
            health: archetype(UnitKind::Commander).max_health,
        });
        let micro = decide_micro(&weak, &Personality::RUSHER, Scenario::Playground);
        assert!(
            !micro
                .iter()
                .any(|i| matches!(i, AiIntent::FocusFire { .. })),
            "debole non snipa: {micro:?}"
        );
    }

    #[test]
    fn retreat_to_turret() {
        // Ancora = torretta completa più vicina a casa; senza torrette, casa.
        let home = Vec3::new(-260.0, 0.0, -260.0);
        assert_eq!(retreat_anchor(&[], home), home);
        let near = Vec3::new(-240.0, 0.0, -240.0);
        let far = Vec3::new(0.0, 0.0, 0.0);
        assert_eq!(retreat_anchor(&[far, near], home), near);
        assert_eq!(retreat_anchor(&[near, far], home), near);
    }

    #[test]
    fn engagement_range_prefers_live_then_memory_then_melee() {
        use crate::units::archetype;
        let max = archetype(UnitKind::HeavyTank).max_health;
        // Armata a ZERO, nemico live a 30m: 30.
        let mut snap = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(30.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: max,
        });
        assert!((engagement_range(&snap) - 30.0).abs() < 0.01);
        // Solo ricordo fresco a 40m (niente live): 40.
        let mut ghost = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
        ghost.memory.push(super::super::snapshot::AiMemory {
            entity_bits: None,
            pos: Vec3::new(40.0, 0.0, 0.0),
            age_ticks: 5,
            kind: Some(UnitKind::HeavyTank),
            hp: max,
            building: false,
        });
        assert!((engagement_range(&ghost) - 40.0).abs() < 0.01);
        // Buio totale o nessuna armata: mischia (vecchia matematica).
        let dark = armed_snapshot(1, &[(UnitKind::HeavyTank, max, UnitOrder::Idle)]);
        assert_eq!(engagement_range(&dark), 0.0);
        let empty = armed_snapshot(1, &[]);
        assert_eq!(engagement_range(&empty), 0.0);
    }

    #[test]
    fn battery_excludes_scout_commander_explicitly() {
        use crate::units::archetype;
        // Anche se Scout/Commander cambiassero gittata in tabella, il filtro
        // batteria non deve mai arruolarli: sono occhi/capitale, mai batteria.
        let max_s = archetype(UnitKind::Scout).max_health;
        let max_c = archetype(UnitKind::Commander).max_health;
        let max_a = archetype(UnitKind::Artillery).max_health;
        let mut snap = armed_snapshot(
            1,
            &[
                (UnitKind::Scout, max_s, UnitOrder::Idle),
                (UnitKind::Commander, max_c, UnitOrder::Idle),
                (UnitKind::Artillery, max_a, UnitOrder::Idle),
            ],
        );
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(900),
            pos: Vec3::new(100.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: archetype(UnitKind::HeavyTank).max_health,
        });
        let micro = decide_micro(&snap, &Personality::RUSHER, Scenario::Playground);
        let hold_units: Vec<Entity> = micro
            .iter()
            .filter_map(|i| match i {
                AiIntent::HoldAtMaxRange { units, .. } => Some(units.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        // Solo l'arty in batteria (bit 102 se ordine snapshot stabile).
        assert_eq!(hold_units.len(), 1);
        let arty_entity = snap
            .my_units
            .iter()
            .find(|u| u.kind == UnitKind::Artillery)
            .unwrap()
            .entity;
        assert!(hold_units.contains(&arty_entity));
    }

    #[test]
    fn hold_holds_ground_when_already_in_range() {
        // Ancora già dentro (range-4): tiene il terreno, niente avanzate.
        let threat = Vec3::new(100.0, 0.0, 0.0);
        let anchor_close = Vec3::new(110.0, 0.0, 0.0); // dist 10 <= 26
        assert_eq!(
            hold_position(anchor_close, threat, 30.0),
            anchor_close.with_y(0.0)
        );
        // Fuori gittata: avanza a range-4 sul lato armata.
        let anchor_far = Vec3::ZERO; // dist 100 > 26
        let hold = hold_position(anchor_far, threat, 30.0);
        assert!((hold.distance(threat) - 26.0).abs() < 0.01);
    }

    #[test]
    fn from_name_knows_baselines() {
        assert_eq!(Personality::from_name("eco-only"), Personality::ECO_ONLY);
        assert_eq!(
            Personality::from_name("rush-scripted"),
            Personality::RUSH_SCRIPTED
        );
        assert_eq!(Personality::from_name("rusher"), Personality::RUSHER);
        assert_eq!(Personality::from_name("???"), Personality::TURTLE);
        // 0.0.21 — strict: ignoti sono errore, mai fallback silenzioso.
        assert!(Personality::try_from_name("gandalf").is_err());
        assert!(Personality::try_from_name("turtle").is_ok());
    }

    #[test]
    fn utility_weights_gate_build_and_defense() {
        // 0.0.22 — pesi a 0 sopprimono, a 1.0 emettono come prima (continuità).
        let mut snap = empty_snapshot(1);
        idle_commander(&mut snap);
        let intents = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        assert!(intents.iter().any(|i| matches!(i, AiIntent::Build(_))));
        let muted = Personality {
            w_build: 0.0,
            w_defense: 0.0,
            ..Personality::TURTLE
        };
        let intents = decide(&snap, &muted, Scenario::Playground, &[], 0);
        assert!(
            !intents.iter().any(|i| matches!(i, AiIntent::Build(_))),
            "w_build=0 deve sopprimere il bootstrap: {intents:?}"
        );
    }

    #[test]
    fn utility_weights_gate_enqueue_and_scout() {
        // Enqueue: factory libera + mix rusher = emette a peso 1, tace a 0.
        let snap = armed_snapshot(
            1,
            &[
                (UnitKind::Scout, 60.0, UnitOrder::Idle),
                (UnitKind::Scout, 60.0, UnitOrder::Idle),
            ],
        );
        let free = fac(1, 0, false, 1, &[]);
        let on = decide(
            &snap,
            &Personality::RUSHER,
            Scenario::Playground,
            std::slice::from_ref(&free),
            0,
        );
        assert!(on.iter().any(|i| matches!(i, AiIntent::Enqueue { .. })));
        let muted = Personality {
            w_enqueue: 0.0,
            ..Personality::RUSHER
        };
        let off = decide(&snap, &muted, Scenario::Playground, &[free], 0);
        assert!(
            !off.iter().any(|i| matches!(i, AiIntent::Enqueue { .. })),
            "w_enqueue=0 deve sopprimere: {off:?}"
        );
        // Scout: 1 scout libero, cieco = emette a peso 1, tace a 0.
        let mut scout_snap = empty_snapshot(1);
        scout_snap.my_units.push(super::super::snapshot::AiUnit {
            entity: Entity::from_bits(42),
            pos: Vec3::ZERO,
            kind: UnitKind::Scout,
            order: UnitOrder::Idle,
            health: 60.0,
            max_health: 60.0,
        });
        let on = decide(
            &scout_snap,
            &Personality::RUSHER,
            Scenario::Playground,
            &[],
            0,
        );
        assert!(on.iter().any(|i| matches!(i, AiIntent::Scout { .. })));
        let muted = Personality {
            w_scout: 0.0,
            ..Personality::RUSHER
        };
        let off = decide(&scout_snap, &muted, Scenario::Playground, &[], 0);
        assert!(
            !off.iter().any(|i| matches!(i, AiIntent::Scout { .. })),
            "w_scout=0 deve sopprimere: {off:?}"
        );
    }

    #[test]
    fn decide_with_threat_matches_decide() {
        // 0.0.23 — a mappa uguale (stesso snapshot), la via cached emette gli
        // stessi intenti della via con build interna: la cache non cambia la
        // decisione, solo il costo.
        let mut snap = empty_snapshot(1);
        idle_commander(&mut snap);
        idle_engineer(&mut snap);
        let threat = super::super::threat::build_threat(&snap);
        for pers in [Personality::TURTLE, Personality::RUSHER] {
            let a = decide(&snap, &pers, Scenario::Playground, &[], 0);
            let b = decide_with_threat(&snap, &pers, Scenario::Playground, &[], 0, &threat);
            assert_eq!(a, b);
        }
        // Anche con nemici visibili (hotspot/threat attivi nei rami).
        snap.visible_enemies.push(super::super::snapshot::AiEnemy {
            entity: Entity::from_bits(910),
            pos: Vec3::new(60.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: crate::units::archetype(UnitKind::HeavyTank).max_health,
        });
        let threat = super::super::threat::build_threat(&snap);
        let a = decide(&snap, &Personality::TURTLE, Scenario::Playground, &[], 0);
        let b = decide_with_threat(
            &snap,
            &Personality::TURTLE,
            Scenario::Playground,
            &[],
            0,
            &threat,
        );
        assert_eq!(a, b);
        assert!(
            base_under_threat(&snap, Scenario::Playground)
                == base_under_threat_with_map(&snap, Scenario::Playground, &threat)
        );
    }

    #[test]
    fn ron_roundtrip() {
        // 0.0.21 — 4 `.ron` bit-identici alle const (stesso cervello, file).
        for (name, want) in [
            ("turtle", Personality::TURTLE),
            ("rusher", Personality::RUSHER),
            ("eco-only", Personality::ECO_ONLY),
            ("rush-scripted", Personality::RUSH_SCRIPTED),
        ] {
            let path = format!("personalities/{name}.ron");
            let text = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("manca {path}"));
            let got = Personality::from_ron(&text).expect("ron deve parsare");
            assert_eq!(got, want, "mismatch {name}");
            // Roundtrip via def: ser -> de stabile.
            let def = PersonalityDef::from(&want);
            let ser = ron::ser::to_string(&def).expect("ron ser");
            let back = Personality::from_ron(&ser).expect("ron de");
            assert_eq!(back, want, "roundtrip {name}");
        }
        assert!(Personality::from_ron("(name: \"x\")").is_err());
    }
}
