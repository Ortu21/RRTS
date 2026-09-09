//! Utility scoring full per la macro parallela (0.0.22).
//!
//! Graham Pro1 Ch9 (teoria: opzioni valutate 0..1, non if/else), Lewis Pro3
//! Ch13 (considerations con response-curve), Hanlon/Watts Pro3 Ch31 (DA:I:
//! scorer piccoli e composti, pesi in data).
//!
//! Attacco (pre-esistente): tre considerazioni pure (`win_prob`, prontezza
//! armata, sicurezza base) composte in prodotto (fuzzy-AND, come DA:I) in
//! un'unica `opportunity` 0..1 che `decide()` confronta con `courage`.
//!
//! 0.0.22 — ogni altra macro-azione parallela ha il suo scorer 0..1 puro:
//! `build_urgency`, `enqueue_urgency`, `scout_urgency`, `defense_urgency`.
//! `decide()` li confronta con `peso × soglia` (pesi in `Personality`, default
//! 1.0 = comportamento bit-identico). Niente `HashMap`/rand/time, tutto
//! deterministico.
//!
//! Continuità col vecchio cancello booleano (stessi casi di riferimento):
//! - armata alla soglia + `win_prob == courage` → `opportunity == courage`
//!   (stessa decisione di prima);
//! - sotto soglia la curva smussa invece di tagliare (niente knife-edge);
//! - alla cieca la soglia si alza di +2 come prima (stessa massa critica).
//! - build/enqueue/scout/defense a default emettono come prima (gate 0.05
//!   permissivo: salta solo lo zero vero, mai un caso che prima emetteva).

/// Soglia di sicurezza quando la base è minacciata e la vittoria è tutt'altro
/// che certa: il richiamo difensivo resta, ma non è più un muro (vedi
/// `safety`). Dato di tuning come gli altri (pesi veri in `Personality` =
/// follow-up, oggi solo questa costante + `courage` in data).
pub const SAFETY_FLOOR: f32 = 0.35;

/// Clamp 0..1 (NaN → 0.0: mai NaN in cascata negli scorer).
pub fn clamp01(x: f32) -> f32 {
    if !x.is_finite() {
        return 0.0;
    }
    x.clamp(0.0, 1.0)
}

/// Smoothstep deterministico 0..1 tra i bordi. Bordi degeneri (`lo >= hi`) =
/// gradino su `lo` (mai divisione per zero).
pub fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    if !(lo.is_finite() && hi.is_finite() && x.is_finite()) {
        return 0.0;
    }
    if lo >= hi {
        return if x >= lo { 1.0 } else { 0.0 };
    }
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    clamp01(s)
}

/// Prontezza armata 0..1: rampa `threshold-2 → threshold` (piena alla soglia,
/// come il vecchio cancello `army >= threshold`; sotto soglia credito
/// parziale invece del taglio secco). Alla cieca il chiamante alza la soglia
/// di +2 (stessa massa critica di prima). Soglie "mai" (`usize::MAX` delle
/// baseline) = sempre 0 (aritmetica saturata, mai overflow).
pub fn readiness(army_count: usize, threshold: usize) -> f32 {
    let hi = threshold;
    let lo = threshold.saturating_sub(2);
    // Soglia 0/1 (mai usata oggi, ma pura): banda degenere = gradino.
    smoothstep(lo as f32, hi as f32, army_count as f32)
}

/// Sicurezza 0..1: base libera = 1.0; base minacciata = rampa da
/// `SAFETY_FLOOR` (vittoria improbabile: resta a casa) a 1.0 (dominanza:
/// contrattacco consentito, il richiamo non è più un muro). Pura.
pub fn safety(base_threatened: bool, win_prob: f32) -> f32 {
    if !base_threatened {
        return 1.0;
    }
    SAFETY_FLOOR + (1.0 - SAFETY_FLOOR) * clamp01(win_prob)
}

/// Opportunità d'attacco 0..1 = `win_prob × readiness × safety` (fuzzy-AND:
/// basta una considerazione a 0 per restare a casa). `decide()` la confronta
/// con `courage` (stessa soglia di prima, ora sulla composita). Pura.
///
/// Nota onesta dai dati (torneo full 192 match + probe quick): la `win_prob`
/// al lancio è ~1.0 anche nelle sconfitte — il predittore non vede riserve
/// nascoste/range/posizione, quindi NESSUN gate sulla sua magnitudine può
/// discriminare (abbassare `courage` non serve: si spara già a 1.0). Qui NON
/// c'è un quarto fattore "fiducia informativa": provato (`confidence` da
/// `explored_pct`), scartato perché frenava proprio l'unico path che chiude
/// (marce alla cieca su nemico idle) senza toccare la patologia vera
/// (sconfitte CON occhi a p≈1.0). Quella richiede predizione range-aware
/// (lavoro futuro): quando `win_prob` sarà informativa, avrà senso pesarla
/// per l'incertezza.
pub fn attack_opportunity(
    win_prob: f32,
    army_count: usize,
    threshold: usize,
    base_threatened: bool,
) -> f32 {
    clamp01(win_prob) * readiness(army_count, threshold) * safety(base_threatened, win_prob)
}

/// 0.0.22 — soglia permissiva dei gate utility (pesi default = continuità):
/// sotto resta a casa, sopra emette come prima. Solo lo zero vero salta.
pub const UTILITY_GATE: f32 = 0.05;

/// 0.0.22 — urgenza build 0..1: bootstrap (Metal/Solar/Factory a 0) = 1.0
/// incondizionato; altrimenti bottleneck × abbordabilità × capacità.
/// `tta_secs` INF (non abbordabile entro 60s) = 0. Pura.
pub fn build_urgency(
    bootstrap_needed: bool,
    bottleneck: bool,
    affordable: bool,
    has_capacity: bool,
    tta_secs: f64,
) -> f32 {
    if !has_capacity {
        return 0.0;
    }
    if bootstrap_needed {
        return 1.0;
    }
    if !bottleneck || !affordable {
        return 0.0;
    }
    if !tta_secs.is_finite() {
        return 0.0;
    }
    // Abbordabilità: 0s = 1.0, 60s = 0.0 (stesso orizzonte del planner).
    let t = (tta_secs.max(0.0) / 60.0) as f32;
    clamp01(1.0 - t).max(0.35)
}

/// 0.0.22 — urgenza enqueue 0..1 per factory: bloccata/piena = 0,
/// altrimenti capacità libera in curva (mezza coda = 0.5). Pura.
pub fn enqueue_urgency(queue_len: usize, max_queue: usize, blocked: bool) -> f32 {
    if blocked || max_queue == 0 {
        return 0.0;
    }
    if queue_len >= max_queue {
        return 0.0;
    }
    clamp01(1.0 - queue_len as f32 / max_queue.max(1) as f32).max(0.15)
}

/// 0.0.22 — urgenza scout 0..1: senza scout liberi = 0; alla cieca = alta,
/// poi `1 - explored` con pavimento (occhi servono sempre per il courage).
/// `explored_pct` NaN/fuori range = trattato come 0 (novelty massima). Pura.
pub fn scout_urgency(free_scouts: usize, has_fresh_eyes: bool, explored_pct: f32) -> f32 {
    if free_scouts == 0 {
        return 0.0;
    }
    let explored = if explored_pct.is_finite() {
        explored_pct.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let novelty = 1.0 - explored;
    if !has_fresh_eyes {
        (0.65 + 0.35 * novelty).clamp(0.0, 1.0)
    } else {
        (0.25 + 0.55 * novelty).clamp(0.0, 1.0)
    }
}

/// 0.0.22 — urgenza difesa statica 0..1: senza deficit = 0; minacciati = 1.0,
/// altrimenti 0.55 (presidio programmato, come il vecchio ramo turtle).
/// Pura.
pub fn defense_urgency(has_deficit: bool, base_threatened: bool) -> f32 {
    if !has_deficit {
        return 0.0;
    }
    if base_threatened { 1.0 } else { 0.55 }
}

/// 0.0.22 — snapshot degli score macro per telemetria/debug (tutti 0..1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UtilityScores {
    pub attack: f32,
    pub build: f32,
    pub enqueue: f32,
    pub scout: f32,
    pub defense: f32,
}

impl UtilityScores {
    /// Tutti finiti e dentro 0..1 (gate `utility-sane` del director).
    pub fn sane(&self) -> bool {
        [
            self.attack,
            self.build,
            self.enqueue,
            self.scout,
            self.defense,
        ]
        .iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp01_kills_nan_and_clamps() {
        assert_eq!(clamp01(f32::NAN), 0.0);
        assert_eq!(clamp01(-2.0), 0.0);
        assert_eq!(clamp01(2.0), 1.0);
        assert_eq!(clamp01(0.4), 0.4);
    }

    #[test]
    fn smoothstep_edges_and_monotonic() {
        assert_eq!(smoothstep(2.0, 6.0, 0.0), 0.0);
        assert_eq!(smoothstep(2.0, 6.0, 9.0), 1.0);
        assert!((smoothstep(2.0, 6.0, 4.0) - 0.5).abs() < 1e-6);
        // Monotona e dentro 0..1 sul tratto.
        let mut prev = 0.0;
        for i in 0..=40 {
            let v = smoothstep(2.0, 6.0, i as f32 * 0.2);
            assert!((0.0..=1.0).contains(&v) && v >= prev);
            prev = v;
        }
        // Degeneri e sporchi: mai NaN, mai panic.
        assert_eq!(smoothstep(3.0, 3.0, 2.9), 0.0);
        assert_eq!(smoothstep(3.0, 3.0, 3.0), 1.0);
        assert_eq!(smoothstep(2.0, 6.0, f32::NAN), 0.0);
    }

    #[test]
    fn readiness_ramps_and_never_fires() {
        // Piena alla soglia (come il vecchio `army >= threshold`), rampa -2.
        assert_eq!(readiness(0, 4), 0.0);
        assert_eq!(readiness(2, 4), 0.0);
        assert!((readiness(3, 4) - 0.5).abs() < 1e-6);
        assert_eq!(readiness(4, 4), 1.0);
        assert_eq!(readiness(99, 4), 1.0);
        // Soglia 0: gradino (pura anche se mai usata).
        assert_eq!(readiness(0, 0), 1.0);
        // Soglie "mai" (eco-only): sempre 0, mai overflow.
        assert_eq!(readiness(0, usize::MAX), 0.0);
        assert_eq!(readiness(10_000, usize::MAX), 0.0);
    }

    #[test]
    fn safety_recalls_but_yields_to_dominance() {
        assert_eq!(safety(false, 0.0), 1.0);
        assert_eq!(safety(false, 0.9), 1.0);
        assert!((safety(true, 0.0) - SAFETY_FLOOR).abs() < 1e-6);
        assert_eq!(safety(true, 1.0), 1.0);
        // Monotona in win_prob quando minacciati.
        assert!(safety(true, 0.8) > safety(true, 0.3));
    }

    #[test]
    fn opportunity_matches_old_gate_at_reference() {
        // Continuità: armata alla soglia + win_prob == courage + occhi →
        // == courage (stessa decisione di prima a meno dello stretto `>`).
        let opp = attack_opportunity(0.65, 4, 4, false);
        assert!((opp - 0.65).abs() < 1e-6, "{opp}");
        // Sotto prontezza dimezza (niente knife-edge: 0.65*0.5 < 0.65).
        let opp = attack_opportunity(0.65, 3, 4, false);
        assert!(opp < 0.65 && opp > 0.0, "{opp}");
        // Minacciati a parità: richiamo (0.65*1.0*safety(0.65)=~0.50).
        let opp = attack_opportunity(0.65, 4, 4, true);
        assert!(opp < 0.65, "{opp}");
        // Minacciati ma dominanti: contrattacco (1.0*1.0*1.0).
        assert_eq!(attack_opportunity(1.0, 4, 4, true), 1.0);
        // Mai NaN dentro.
        assert_eq!(attack_opportunity(f32::NAN, 4, 4, false), 0.0);
    }

    #[test]
    fn build_bootstrap_is_one() {
        assert_eq!(build_urgency(true, false, true, true, 0.0), 1.0);
        // Senza capacità mai, nemmeno in bootstrap.
        assert_eq!(build_urgency(true, true, true, false, 0.0), 0.0);
    }

    #[test]
    fn build_no_deficit_is_zero() {
        assert_eq!(build_urgency(false, false, true, true, 0.0), 0.0);
        // Bottleneck ma non abbordabile (TTA INF) = aspetta eco.
        assert_eq!(build_urgency(false, true, false, true, f64::INFINITY), 0.0);
        assert_eq!(build_urgency(false, true, true, true, f64::INFINITY), 0.0);
        // Bottleneck abbordabile subito = alto (continuità: emette come prima).
        let u = build_urgency(false, true, true, true, 0.0);
        assert!(u > UTILITY_GATE, "{u}");
        // Monotona in TTA: prima = più urgente.
        assert!(
            build_urgency(false, true, true, true, 0.0)
                >= build_urgency(false, true, true, true, 30.0)
        );
    }

    #[test]
    fn enqueue_full_is_zero() {
        assert_eq!(enqueue_urgency(12, 12, false), 0.0);
        assert_eq!(enqueue_urgency(0, 12, true), 0.0);
        assert_eq!(enqueue_urgency(0, 0, false), 0.0);
        // Libera = sopra gate (continuità), mezza = metà.
        assert!(enqueue_urgency(0, 12, false) > UTILITY_GATE);
        assert!(enqueue_urgency(0, 12, false) > enqueue_urgency(6, 12, false));
        assert!(enqueue_urgency(6, 12, false) > 0.0);
    }

    #[test]
    fn scout_blind_is_high() {
        assert_eq!(scout_urgency(0, false, 0.0), 0.0);
        let blind = scout_urgency(1, false, 0.0);
        let eyes = scout_urgency(1, true, 0.0);
        assert!(blind > eyes, "{blind} vs {eyes}");
        assert!(blind > UTILITY_GATE && eyes > UTILITY_GATE);
        // Esplorato tutto + occhi = pavimento basso ma > 0 (occhi servono).
        let done = scout_urgency(1, true, 1.0);
        assert!(done > 0.0 && done < eyes);
        // NaN = novelty massima, mai NaN fuori.
        assert!(scout_urgency(1, false, f32::NAN) > UTILITY_GATE);
    }

    #[test]
    fn defense_threatened_is_one() {
        assert_eq!(defense_urgency(false, false), 0.0);
        assert_eq!(defense_urgency(false, true), 0.0);
        assert_eq!(defense_urgency(true, true), 1.0);
        let calm = defense_urgency(true, false);
        assert!(calm > UTILITY_GATE && calm < 1.0, "{calm}");
    }

    #[test]
    fn utility_scores_bounded() {
        let s = UtilityScores {
            attack: 0.65,
            build: 1.0,
            enqueue: 0.5,
            scout: 0.8,
            defense: 0.55,
        };
        assert!(s.sane());
        assert!(
            !UtilityScores {
                attack: f32::NAN,
                ..s
            }
            .sane()
        );
        assert!(!UtilityScores { attack: 2.0, ..s }.sane());
        assert!(UtilityScores::default().sane());
    }
}
