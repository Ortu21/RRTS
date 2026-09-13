//! Utility strategy + tactics (pure, testable).
//!
//! Decisioni data-driven: ogni opzione ha scorer 0..1, la personalità pesa
//! le soglie. Aggiungere unità/edifici futuri = nuove righe in tabella +
//! pesi in [`Personality`], mai `if kind == X` nel core.

//! Decisione macro: scorer, courage Lanchester, counter-comp, ondate.

use super::*;
use crate::ai::{memory::MEMORY_FRESH_TICKS, snapshot::AiSnapshot};
use crate::{
    economy::balance::{BuildingKind, MAX_QUEUE},
    orders::UnitOrder,
    scenario::Scenario,
    units::UnitKind,
};
use bevy::prelude::*;

/// Potenza combattimento stile Lanchester: hp * dps. Engineer disarmato = 0.
/// 0.0.17: singola fonte = `combat::effective_dps` (stessa formula di prima).
pub fn combat_power(kind: UnitKind, health: f32) -> f32 {
    if health <= 0.0 {
        return 0.0;
    }
    health * crate::ai::combat::effective_dps(kind)
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

/// Ricordi freschi con potenza stimata, pesata per età (stesso decay della
/// threat map) e scontata (posizioni non verificate). 0.0.17: sostituisce lo
/// sconto fisso *0.5 — stessa formula di `threat::build_threat`, mai divergono.
/// 0.0.17: `decide()` usa `estimate_forces` (stessa matematica per-unità);
/// resta API per test di coerenza e telemetria.
#[allow(dead_code)]
pub fn remembered_enemy_power(snapshot: &AiSnapshot) -> f32 {
    use crate::ai::threat::{THREAT_MEMORY_DISCOUNT, age_decay};
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
    let threat = crate::ai::threat::build_threat(snapshot);
    // La personalità qui non è nota al chiamante legacy: usa il peso medio
    // dei main (0.85). `decide()` chiama `frontier_target` col peso vero.
    let fallback = Personality {
        scout_threat_weight: 0.85,
        ..Personality::RUSHER
    };
    crate::ai::scout::frontier_target(snapshot, scenario, &threat, &fallback)
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
pub(crate) fn count_kind(snapshot: &AiSnapshot, queued: &[UnitKind], kind: UnitKind) -> usize {
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
pub(crate) fn pick_deficit(
    cands: &[(UnitKind, f32)],
    count: impl Fn(UnitKind) -> usize,
) -> UnitKind {
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
pub(crate) fn foe_mix(snapshot: &AiSnapshot) -> Vec<(UnitKind, f32)> {
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
pub(crate) fn counter_edge(kind: UnitKind, foe: &[(UnitKind, f32)]) -> f32 {
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
    opponent: crate::ai::opponent::OpponentKind,
    tech_count: usize,
    max_tech: usize,
    confidence: f32,
) -> Option<UnitKind> {
    if foe.is_empty() || tech_count >= max_tech {
        return None;
    }
    let mut best: Option<(UnitKind, f32, u32)> = None;
    for (kind, w) in tech_mix.iter().filter(|(_, w)| *w > 0) {
        // 0.0.24 — bias opponent pesato per confidence (conf=1: identico).
        let edge = counter_edge(*kind, foe)
            * crate::ai::opponent::mix_bias_conf(*kind, opponent, confidence);
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
/// 0.0.24: bias pesato per confidence (conf=1: identico).
pub(crate) fn weighted_mix(
    mix: &[(UnitKind, u32)],
    foe: &[(UnitKind, f32)],
    opponent: crate::ai::opponent::OpponentKind,
    confidence: f32,
) -> Vec<(UnitKind, f32)> {
    mix.iter()
        .map(|(kind, w)| {
            (
                *kind,
                *w as f32
                    * counter_edge(*kind, foe)
                    * crate::ai::opponent::mix_bias_conf(*kind, opponent, confidence),
            )
        })
        .collect()
}

/// Coppia di forze (kind, hp) per il predittore: (mia, nemica).
pub(crate) type ForcePair = (Vec<(UnitKind, f32)>, Vec<(UnitKind, f32)>);

/// Stima forze per il predittore: armata viva (kind,hp) + nemici visibili +
/// ricordi freschi con hp scontati per età (come `remembered_enemy_power`).
pub(crate) fn estimate_forces(snapshot: &AiSnapshot) -> ForcePair {
    use crate::ai::threat::{THREAT_MEMORY_DISCOUNT, age_decay};
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
    crate::ai::combat::predict_outcome_at_range(&my_list, &foe_list, engagement_range(snapshot))
}

/// Kind armato proprio più numeroso (primario) per il focus: pareggi → indice
/// minore. Fallback HeavyTank se nessun armato (deterministico comunque).
pub(crate) fn my_primary_kind(snapshot: &AiSnapshot) -> UnitKind {
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
    let threat = crate::ai::threat::build_threat(snapshot);
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
    threat: &crate::ai::threat::ThreatMap,
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
        let plan = crate::ai::planner::plan_build(snapshot.stock, snapshot.income, snapshot.demand);
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
            let u = crate::ai::utility::build_urgency(
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
            if u * personality.w_build > crate::ai::utility::UTILITY_GATE {
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
        && crate::ai::utility::defense_urgency(true, threatened_early) * personality.w_defense
            > crate::ai::utility::UTILITY_GATE
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
        && crate::ai::utility::defense_urgency(true, threatened_early) * personality.w_defense
            > crate::ai::utility::UTILITY_GATE
    {
        intents.push(AiIntent::Build(BuildingKind::Lance));
    }
    // 0.0.18 — muri: schermo davanti alla prima torretta completa (l'executor
    // cerca gli slot verso la minaccia, senza murare le factory). Solo turtle,
    // contati con i siti: niente doppie richieste.
    if personality.max_walls > 0
        && snapshot.complete_building(BuildingKind::Turret) > 0
        && snapshot.count_building(BuildingKind::Wall) < personality.max_walls
        && crate::ai::utility::defense_urgency(true, threatened_early) * personality.w_defense
            > crate::ai::utility::UTILITY_GATE
    {
        intents.push(AiIntent::Build(BuildingKind::Wall));
    }

    // 0.0.17 — comp nemica stimata una volta per tick: guida i pesi mix
    // (counter) e il predittore Lanchester. Vuota = nemico ignoto.
    let foe = foe_mix(snapshot);
    // 0.0.24 — opponent modeling con isteresi: classe STABILE + confidence
    // dalla percezione (`refresh_snapshots`), mai classifica istantanea.
    // A credenza convergente (conf=1) pesi bit-identici ai vecchi.
    let opponent = snapshot.opponent;
    let opp_conf = snapshot.opp_confidence;
    let courage = crate::ai::opponent::adjust_courage_conf(personality.courage, opponent, opp_conf);

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
        } else if scouts < crate::ai::scout::MAX_SCOUTS
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
            pick_deficit(&weighted_mix(&T2_MIX, &foe, opponent, opp_conf), |k| {
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
                opp_conf,
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
                pick_deficit(&weighted_mix(&mix, &foe, opponent, opp_conf), |k| {
                    count_kind(snapshot, &queued_all, k)
                })
            }
        };
        // Accoda solo se producibile (mai Commander).
        // 0.0.22 — gate utility: capacità libera × peso (default passa sempre
        // quando il vecchio codice emetteva: code libere = urgenza > gate).
        if UnitKind::PRODUCIBLE.contains(&kind) {
            let u = crate::ai::utility::enqueue_urgency(view.queue_len, MAX_QUEUE, view.blocked);
            if u * personality.w_enqueue > crate::ai::utility::UTILITY_GATE {
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
    let win_prob = crate::ai::combat::predict_outcome_at_range(
        &my_list,
        &foe_list,
        engagement_range(snapshot),
    );
    let threatened = threatened_early;
    // Alla cieca (mai visto il nemico: `win_prob` è 1.0 a vuoto) serve la massa
    // critica di prima: soglia effettiva +2, in curva invece che in ramo morto.
    // Saturating: soglie "mai" (usize::MAX delle baseline) non vanno in overflow.
    let blind = snapshot.visible_enemies.is_empty() && !has_fresh_eyes(snapshot);
    let threshold_eff = personality
        .army_threshold
        .saturating_add(if blind { 2 } else { 0 });
    let opportunity =
        crate::ai::utility::attack_opportunity(win_prob, army_count, threshold_eff, threatened);
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
        && crate::ai::utility::attack_opportunity(
            crate::ai::combat::predict_outcome(&my_list, &foe_list),
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
                            > crate::ai::scout::SCOUT_THREAT_THRESHOLD))
            })
            .count();
        // 0.0.22 — gate utility (occhi + novelty): alla cieca = alto, con
        // occhi + mappa piena = basso ma > 0 (default passa sempre quando ci
        // sono scout liberi, come prima).
        let u = crate::ai::utility::scout_urgency(
            free_scouts,
            has_fresh_eyes(snapshot),
            snapshot.explored_pct,
        );
        if u * personality.w_scout <= crate::ai::utility::UTILITY_GATE {
            return intents;
        }
        let n = free_scouts.min(crate::ai::scout::MAX_SCOUTS);
        if n > 0 {
            for destination in
                crate::ai::scout::frontier_targets(snapshot, scenario, threat, personality, n)
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

/// Baricentro delle unità elencate (dalle posizioni snapshot). `None` se
/// vuote o sparite. Puro.
pub(crate) fn centroid_of(snapshot: &AiSnapshot, entities: &[Entity]) -> Option<Vec3> {
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
