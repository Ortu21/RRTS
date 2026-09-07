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

    #[test]
    fn dps_weights_tank_over_scout_and_ignores_engineer() {
        let tank = effective_dps(UnitKind::HeavyTank);
        let scout = effective_dps(UnitKind::Scout);
        assert!(tank > scout && scout > 0.0);
        assert_eq!(effective_dps(UnitKind::Engineer), 0.0);
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
