//! Combat outcome prediction (0.0.17): Lanchester aimed-fire fast-forward.
//!
//! Stima `win_prob` 0..1 simulando 10s di fuoco mirato a step fissi, con DPS
//! da tabella pesati per counter. Puro e deterministico: input ordinati
//! internamente (indipendente dall'ordine di chiamata), loop a conteggio
//! fisso, `total_cmp` ovunque, niente HashMap/rand/time. Legge SOLO tabelle
//! (`effective_dps`, `counter_mult`): mai branch per-kind.

use crate::{economy::balance::counter_mult, units::UnitKind};

/// Orizzonte simulato: 20 step × 0.5s = 10s di ingaggio.
pub const SIM_STEPS: usize = 20;
pub const SIM_DT: f32 = 0.5;

/// DPS effettivo da tabella (danno/cooldown + secondaria ×0.7, stessa formula
/// di `strategy::combat_power` senza hp). Disarmati (Engineer) = 0.
pub fn effective_dps(kind: UnitKind) -> f32 {
    let stats = crate::units::archetype(kind);
    if !stats.armed {
        return 0.0;
    }
    let mut dps = stats.damage / stats.cooldown.max(0.05);
    if let Some(sec) = crate::units::archetype::secondary_stats(kind) {
        dps += (sec.damage / sec.cooldown.max(0.05)) * 0.7;
    }
    dps
}

/// Fase A step 1 — gittata massima d'ingaggio (primaria e secondaria):
/// oltre non si colpisce. Pura. (Nessun chiamante oltre i test: il cablaggio
/// nel predittore è lo step 2, con calibrazione sui dati torneo.)
#[allow(dead_code)]
pub fn max_range(kind: UnitKind) -> f32 {
    let stats = crate::units::archetype(kind);
    if !stats.armed {
        return 0.0;
    }
    let mut range = stats.range;
    if let Some(sec) = crate::units::archetype::secondary_stats(kind) {
        range = range.max(sec.range);
    }
    range
}

/// Fase A step 1 — DPS erogabile a distanza `dist` (gating fisico: fuori
/// gittata il cannone non colpisce, dentro rende pieno). Primaria e
/// secondaria valutate sulle rispettive gittate. Pura e deterministica.
/// (Statico: l'avvicinamento durante l'orizzonte è step 2.)
#[allow(dead_code)]
pub fn dps_at_range(kind: UnitKind, dist: f32) -> f32 {
    if !dist.is_finite() || dist < 0.0 {
        return 0.0;
    }
    let stats = crate::units::archetype(kind);
    if !stats.armed {
        return 0.0;
    }
    let mut dps = 0.0;
    if dist <= stats.range {
        dps += stats.damage / stats.cooldown.max(0.05);
    }
    if let Some(sec) = crate::units::archetype::secondary_stats(kind)
        && dist <= sec.range
    {
        dps += (sec.damage / sec.cooldown.max(0.05)) * 0.7;
    }
    dps
}

/// Priorità di focus per un bersaglio: minaccia (dps × counter contro il
/// nostro kind primario). A pari priorità decide lo hp (vedi strategia).
pub fn target_priority(kind: UnitKind, vs: UnitKind) -> f32 {
    effective_dps(kind) * counter_mult(kind, vs)
}

/// Probabilità di vittoria 0..1: `(kind, hp)` per lato (hp già scontati per
/// ricordi se necessario dal chiamante). Entrambi vuoti → 0.5, lato vuoto →
/// 0.0/1.0. Il danno è applicato ai più feriti prima (dottrina focus-fire,
/// uguale per entrambi i lati).
pub fn predict_outcome(my: &[(UnitKind, f32)], foe: &[(UnitKind, f32)]) -> f32 {
    let mut mine = sanitized(my);
    let mut theirs = sanitized(foe);
    let my_total: f32 = mine.iter().map(|(_, hp)| hp).sum();
    let foe_total: f32 = theirs.iter().map(|(_, hp)| hp).sum();
    if my_total <= 0.0 && foe_total <= 0.0 {
        return 0.5;
    }
    if my_total <= 0.0 {
        return 0.0;
    }
    if foe_total <= 0.0 {
        return 1.0;
    }
    for _ in 0..SIM_STEPS {
        if mine.is_empty() || theirs.is_empty() {
            break;
        }
        let my_dps = side_dps(&mine, &theirs);
        let foe_dps = side_dps(&theirs, &mine);
        deal_damage(&mut theirs, my_dps * SIM_DT);
        deal_damage(&mut mine, foe_dps * SIM_DT);
    }
    finish_score(&mine, &theirs, my_total, foe_total)
}

/// Fase A step 2 — fast-forward range-aware: come `predict_outcome`, ma da
/// distanza iniziale `dist` che si chiude a `closing_speed` (difensore fermo)
/// e con ogni cannone gated sulla sua gittata. La dinamica che decide gli
/// assedi (lunghi che picchiano da fuori, corti che incassano avvicinandosi)
/// emerge dal sim, non da costanti.
/// `dist` non-finita/≤0 (sconosciuta/cieca) = mischia immediata, bit-identica
/// a `predict_outcome` (test di continuità). Pura e deterministica.
pub fn predict_outcome_at_range(my: &[(UnitKind, f32)], foe: &[(UnitKind, f32)], dist: f32) -> f32 {
    let mut mine = sanitized(my);
    let mut theirs = sanitized(foe);
    let my_total: f32 = mine.iter().map(|(_, hp)| hp).sum();
    let foe_total: f32 = theirs.iter().map(|(_, hp)| hp).sum();
    if my_total <= 0.0 && foe_total <= 0.0 {
        return 0.5;
    }
    if my_total <= 0.0 {
        return 0.0;
    }
    if foe_total <= 0.0 {
        return 1.0;
    }
    let mut d = if dist.is_finite() { dist.max(0.0) } else { 0.0 };
    let close = closing_speed(&mine);
    for _ in 0..SIM_STEPS {
        if mine.is_empty() || theirs.is_empty() {
            break;
        }
        let my_dps = side_dps_at_range(&mine, &theirs, d);
        let foe_dps = side_dps_at_range(&theirs, &mine, d);
        deal_damage(&mut theirs, my_dps * SIM_DT);
        deal_damage(&mut mine, foe_dps * SIM_DT);
        d = (d - close * SIM_DT).max(0.0);
    }
    finish_score(&mine, &theirs, my_total, foe_total)
}

/// Filtra hp non-finiti/≤0 e ordina per (kind, hp): risultato indipendente
/// dall'ordine di input (bit-identico tra chiamanti diversi).
fn sanitized(units: &[(UnitKind, f32)]) -> Vec<(UnitKind, f32)> {
    let mut out: Vec<(UnitKind, f32)> = units
        .iter()
        .copied()
        .filter(|(_, hp)| hp.is_finite() && *hp > 0.0)
        .collect();
    out.sort_by(|a, b| {
        a.0.index()
            .cmp(&b.0.index())
            .then_with(|| a.1.total_cmp(&b.1))
    });
    out
}

/// DPS del lato con counter medi pesati per hp-share nemica. L'attrito
/// emerge dalle morti (unità rimosse), mai scalando il DPS per gli hp.
fn side_dps(units: &[(UnitKind, f32)], foe: &[(UnitKind, f32)]) -> f32 {
    let foe_total: f32 = foe.iter().map(|(_, hp)| hp).sum();
    units
        .iter()
        .map(|(kind, _)| {
            let edge = if foe_total > 0.0 {
                foe.iter()
                    .map(|(fk, fhp)| counter_mult(*kind, *fk) * fhp)
                    .sum::<f32>()
                    / foe_total
            } else {
                1.0
            };
            effective_dps(*kind) * edge
        })
        .sum()
}

/// Fase A step 2 — DPS del lato a distanza `dist`: come `side_dps`, ma ogni
/// cannone spara solo dentro la sua gittata (`dps_at_range`). Oltre gittata
/// il lato tace (assedi e kiting emergono dal fast-forward, non da costanti).
/// Pura e deterministica.
fn side_dps_at_range(units: &[(UnitKind, f32)], foe: &[(UnitKind, f32)], dist: f32) -> f32 {
    let foe_total: f32 = foe.iter().map(|(_, hp)| hp).sum();
    units
        .iter()
        .map(|(kind, _)| {
            let edge = if foe_total > 0.0 {
                foe.iter()
                    .map(|(fk, fhp)| counter_mult(*kind, *fk) * fhp)
                    .sum::<f32>()
                    / foe_total
            } else {
                1.0
            };
            dps_at_range(*kind, dist) * edge
        })
        .sum()
}

/// Fase A step 2 — velocità di chiusura (coesione: avanza il più lento degli
/// armati; il difensore tiene la posizione). Ipotesi conservativa per chi
/// attacca: sovrastima il tempo esposto fuori gittata, quindi sottostima la
/// win_prob contro difese lunghe — meno suicidi, mai più. 0 senza armati.
/// Pura.
pub fn closing_speed(my: &[(UnitKind, f32)]) -> f32 {
    my.iter()
        .map(|(kind, _)| *kind)
        .filter(|k| crate::units::archetype(*k).armed)
        .map(|k| crate::units::archetype(k).speed)
        .filter(|s| s.is_finite() && *s > 0.0)
        .fold(None, |acc: Option<f32>, s| {
            Some(acc.map_or(s, |a: f32| a.min(s)))
        })
        .unwrap_or(0.0)
}

/// Punteggio finale 0..1 da hp residui (frazioni sul totale iniziale).
/// Singola matematica per entrambi i predittori: mai divergono.
fn finish_score(
    mine: &[(UnitKind, f32)],
    theirs: &[(UnitKind, f32)],
    my_total: f32,
    foe_total: f32,
) -> f32 {
    let my_left: f32 = mine.iter().map(|(_, hp)| hp).sum();
    let foe_left: f32 = theirs.iter().map(|(_, hp)| hp).sum();
    let my_frac = my_left / my_total;
    let foe_frac = foe_left / foe_total;
    let denom = my_frac + foe_frac;
    // NaN esplicito: `<=` su NaN è falso e propagherebbe NaN nel clamp.
    if !denom.is_finite() || denom <= 0.0 {
        0.5
    } else {
        (my_frac / denom).clamp(0.0, 1.0)
    }
}

/// Applica danno ai più feriti prima (ordinamento stabile deterministico).
fn deal_damage(units: &mut Vec<(UnitKind, f32)>, mut dmg: f32) {
    // NaN esplicito: danno non-finito = nessun danno (mai propagato).
    if !dmg.is_finite() || dmg <= 0.0 {
        return;
    }
    units.sort_by(|a, b| a.1.total_cmp(&b.1));
    for (_, hp) in units.iter_mut() {
        if dmg <= 0.0 {
            break;
        }
        let take = hp.min(dmg);
        *hp -= take;
        dmg -= take;
    }
    units.retain(|(_, hp)| *hp > 0.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economy::balance::COUNTER_TABLE;
    use crate::units::UnitKind;

    fn heavies(n: usize) -> Vec<(UnitKind, f32)> {
        let hp = crate::units::archetype(UnitKind::HeavyTank).max_health;
        vec![(UnitKind::HeavyTank, hp); n]
    }

    fn one(kind: UnitKind) -> Vec<(UnitKind, f32)> {
        vec![(kind, crate::units::archetype(kind).max_health)]
    }

    #[test]
    fn range_blind_equals_melee_bit_identical() {
        // Continuità: distanza sconosciuta = vecchia matematica esatta.
        let a = vec![
            (UnitKind::HeavyTank, 170.0),
            (UnitKind::LightTank, 70.0),
            (UnitKind::Artillery, 80.0),
        ];
        let foe = heavies(2);
        for dist in [0.0, -5.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                predict_outcome_at_range(&a, &foe, dist).to_bits(),
                predict_outcome(&a, &foe).to_bits(),
                "dist {dist}"
            );
        }
        // Anche simmetrico resta 0.5 a qualsiasi distanza.
        assert_eq!(
            predict_outcome_at_range(&heavies(3), &heavies(3), 25.0).to_bits(),
            0.5f32.to_bits()
        );
    }

    #[test]
    fn range_rewards_long_guns_monotonically() {
        // 2 arty (30m) vs 1 heavy (19m): più distanza iniziale = più danno
        // gratis prima del trade = win_prob monotona in distanza. A 0m è
        // quasi parità vinta di misura, da 30m è dominanza.
        let foe = one(UnitKind::HeavyTank);
        let mine = vec![
            (
                UnitKind::Artillery,
                crate::units::archetype(UnitKind::Artillery).max_health,
            ),
            (
                UnitKind::Artillery,
                crate::units::archetype(UnitKind::Artillery).max_health,
            ),
        ];
        let p0 = predict_outcome_at_range(&mine, &foe, 0.0);
        let p25 = predict_outcome_at_range(&mine, &foe, 25.0);
        let p30 = predict_outcome_at_range(&mine, &foe, 30.0);
        assert!(p0 > 0.5, "2v1 in mischia parte sopra: {p0}");
        assert!(p25 > p0, "distanza aiuta i lunghi: {p25} > {p0}");
        assert!(p30 > p25, "ancora distanza, ancora meglio: {p30} > {p25}");
        assert!(p30 > 0.8, "da fuori gittata nemica è dominanza: {p30}");
    }

    #[test]
    fn closing_speed_is_slowest_armed_or_zero() {
        let hp = 100.0;
        let mix = vec![
            (UnitKind::HeavyTank, hp),
            (UnitKind::Artillery, hp),
            (UnitKind::Scout, hp),
        ];
        let min = [
            crate::units::archetype(UnitKind::HeavyTank).speed,
            crate::units::archetype(UnitKind::Artillery).speed,
            crate::units::archetype(UnitKind::Scout).speed,
        ]
        .into_iter()
        .fold(f32::INFINITY, f32::min);
        assert!((closing_speed(&mix) - min).abs() < 1e-6);
        assert_eq!(closing_speed(&[]), 0.0);
        assert_eq!(closing_speed(&[(UnitKind::Engineer, hp)]), 0.0);
    }

    #[test]
    fn dps_weights_tank_over_scout_and_ignores_engineer() {
        let tank = effective_dps(UnitKind::HeavyTank);
        let scout = effective_dps(UnitKind::Scout);
        assert!(tank > scout && scout > 0.0);
        assert_eq!(effective_dps(UnitKind::Engineer), 0.0);
    }

    #[test]
    fn range_gates_dps_arty_outranges_heavy() {
        // Heavy 19m, Arty 30m: a 25m l'arty picchia piena, l'heavy zero.
        assert!(dps_at_range(UnitKind::Artillery, 25.0) > 0.0);
        assert_eq!(dps_at_range(UnitKind::HeavyTank, 25.0), 0.0);
        // Dentro le gittate: pieno come `effective_dps`.
        assert_eq!(
            dps_at_range(UnitKind::HeavyTank, 10.0),
            effective_dps(UnitKind::HeavyTank)
        );
        assert_eq!(
            dps_at_range(UnitKind::Artillery, 10.0),
            effective_dps(UnitKind::Artillery)
        );
        // Secondaria Commander (missili 34m): oltre i 20m del cannone resta
        // solo il contributo missili, oltre i 34m zero.
        let cmd_full = effective_dps(UnitKind::Commander);
        assert!(dps_at_range(UnitKind::Commander, 25.0) > 0.0);
        assert!(dps_at_range(UnitKind::Commander, 25.0) < cmd_full);
        assert_eq!(dps_at_range(UnitKind::Commander, 40.0), 0.0);
        // Gittate massime e casi sporchi.
        assert_eq!(max_range(UnitKind::Commander), 34.0);
        assert_eq!(max_range(UnitKind::Artillery), 30.0);
        assert_eq!(max_range(UnitKind::Engineer), 0.0);
        assert_eq!(dps_at_range(UnitKind::Engineer, 5.0), 0.0);
        assert_eq!(dps_at_range(UnitKind::HeavyTank, f32::NAN), 0.0);
        assert_eq!(dps_at_range(UnitKind::HeavyTank, -3.0), 0.0);
    }

    #[test]
    fn counter_table_is_shaped_and_motivated_only() {
        assert_eq!(COUNTER_TABLE.len(), UnitKind::ALL.len());
        for row in &COUNTER_TABLE {
            assert_eq!(row.len(), UnitKind::ALL.len());
            for v in row {
                assert!(v.is_finite() && *v > 0.0);
            }
        }
        // Diagonale neutra.
        for (i, kind) in UnitKind::ALL.into_iter().enumerate() {
            assert_eq!(COUNTER_TABLE[i][i], 1.0, "{kind:?}");
            let _ = kind;
        }
        assert_eq!(counter_mult(UnitKind::HeavyTank, UnitKind::LightTank), 1.15);
        assert_eq!(counter_mult(UnitKind::LightTank, UnitKind::HeavyTank), 0.9);
        assert_eq!(counter_mult(UnitKind::LightTank, UnitKind::Artillery), 1.25);
        assert_eq!(counter_mult(UnitKind::Artillery, UnitKind::LightTank), 0.85);
        assert_eq!(counter_mult(UnitKind::Scout, UnitKind::HeavyTank), 1.0);
        assert_eq!(counter_mult(UnitKind::Commander, UnitKind::HeavyTank), 1.0);
    }

    #[test]
    fn lanchester_square_rewards_numbers_and_is_calibrated() {
        // Superiorità numerica 4v2: vittoria quasi certa (legge quadratica).
        assert!(predict_outcome(&heavies(4), &heavies(2)) > 0.8);
        // Parità perfetta: 0.5 esatto per simmetria.
        assert!((predict_outcome(&heavies(3), &heavies(3)) - 0.5).abs() < 1e-6);
        // Inferiorità 2v4: sconfitta quasi certa.
        assert!(predict_outcome(&heavies(2), &heavies(4)) < 0.2);
        // Bordi: vuoti e lati mancanti.
        assert_eq!(predict_outcome(&[], &[]), 0.5);
        assert_eq!(predict_outcome(&heavies(1), &[]), 1.0);
        assert_eq!(predict_outcome(&[], &heavies(1)), 0.0);
        // Input sporchi (hp≤0, NaN): ignorati come i filtri visibilità.
        assert_eq!(
            predict_outcome(
                &[(UnitKind::HeavyTank, 0.0), (UnitKind::HeavyTank, f32::NAN)],
                &[]
            ),
            0.5
        );
    }

    #[test]
    fn prediction_is_order_independent_and_repeatable() {
        let hp_h = crate::units::archetype(UnitKind::HeavyTank).max_health;
        let hp_l = crate::units::archetype(UnitKind::LightTank).max_health;
        let a = vec![
            (UnitKind::HeavyTank, hp_h),
            (UnitKind::LightTank, hp_l),
            (UnitKind::Artillery, 80.0),
        ];
        let mut b = a.clone();
        b.reverse();
        let foe = heavies(2);
        assert_eq!(
            predict_outcome(&a, &foe).to_bits(),
            predict_outcome(&b, &foe).to_bits()
        );
        assert_eq!(
            predict_outcome(&a, &foe).to_bits(),
            predict_outcome(&a, &foe).to_bits()
        );
    }

    #[test]
    fn focus_priority_prefers_threat_over_chaff() {
        // Primario proprio Heavy: Arty (8.3) batte Light screenato (8.9*0.9).
        let arty = target_priority(UnitKind::Artillery, UnitKind::HeavyTank);
        let light = target_priority(UnitKind::LightTank, UnitKind::HeavyTank);
        let heavy = target_priority(UnitKind::HeavyTank, UnitKind::HeavyTank);
        assert!(heavy > arty && arty > light);
        assert_eq!(
            target_priority(UnitKind::Engineer, UnitKind::HeavyTank),
            0.0
        );
    }
}
