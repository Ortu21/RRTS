//! Utility scoring per la decisione d'attacco (Punto 2).
//!
//! Graham Pro1 Ch9 (teoria: opzioni valutate 0..1, non if/else), Lewis Pro3
//! Ch13 (considerations con response-curve), Hanlon/Watts Pro3 Ch31 (DA:I:
//! scorer piccoli e composti, pesi in data). Qui: tre considerazioni pure
//! (`win_prob`, prontezza armata, sicurezza base) composte in prodotto
//! (fuzzy-AND, come DA:I) in un'unica `opportunity` 0..1 che `decide()`
//! confronta con `courage`. Niente `HashMap`/rand/time, tutto deterministico.
//!
//! Continuità col vecchio cancello booleano (stessi casi di riferimento):
//! - armata alla soglia + `win_prob == courage` → `opportunity == courage`
//!   (stessa decisione di prima);
//! - sotto soglia la curva smussa invece di tagliare (niente knife-edge);
//! - alla cieca la soglia si alza di +2 come prima (stessa massa critica).

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
}
