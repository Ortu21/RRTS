//! Utility strategy + tactics (pure, testable).
//!
//! Decisioni data-driven: ogni opzione ha scorer 0..1, la personalità pesa
//! le soglie. Aggiungere unità/edifici futuri = nuove righe in tabella +
//! pesi in [`Personality`], mai `if kind == X` nel core.

//! Onde multi-ondata + capitale protetto + minaccia base.

use super::*;
use crate::ai::snapshot::AiSnapshot;
use crate::{orders::UnitOrder, scenario::Scenario, units::UnitKind};
use bevy::prelude::*;

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
    let threat = crate::ai::threat::build_threat(snapshot);
    base_under_threat_with_map(snapshot, scenario, &threat)
}

/// 0.0.23 — come `base_under_threat`, con mappa già pronta dalla `ThreatCache`.
/// Stessa matematica: a mappa uguale, stesso booleano. Puro.
pub fn base_under_threat_with_map(
    snapshot: &AiSnapshot,
    scenario: Scenario,
    threat: &crate::ai::threat::ThreatMap,
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
