//! Macro planner (0.0.18): collo di bottiglia eco su costi reali.
//!
//! Sostituisce l'euristica "domanda > offerta × 1.1" con un calcolo su
//! stock/income/demand + costi da tabella: quale edificio (Metal/Solar)
//! abbatte prima il deficit, e tra quanto è abbordabile. Puro e
//! deterministico (solo aritmetica, `total_cmp` nei confronti).

use crate::economy::balance::{BUILDINGS, BuildingKind};

/// Orizzonte: oltre questi secondi un edificio è "non abbordabile".
pub const PLAN_HORIZON_SECS: f64 = 60.0;

/// Isteresi domanda/offerta (come la vecchia euristica).
pub const DEMAND_HYSTERESIS: f64 = 1.1;

/// Esito planner: collo di bottiglia + secondi per poterselo permettere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EcoPlan {
    pub bottleneck: Option<BuildingKind>,
    pub time_to_afford_secs: f64,
}

/// Risorsa che frena la crescita (Metal/Solar) + tempo per il costo.
/// `stock/income/demand` sono gli account [metal, energia] dello snapshot.
/// Nessun branch per-kind fuori tabella: Metal↔risorsa 0, Solar↔risorsa 1.
pub fn plan_build(stock: [f64; 2], income: [f64; 2], demand: [f64; 2]) -> EcoPlan {
    // Candidati (risorsa, edificio che la produce).
    let cands = [(0usize, BuildingKind::Metal), (1usize, BuildingKind::Solar)];
    let mut best: Option<(BuildingKind, f64, f64)> = None; // (kind, deficit_ratio, tta)
    for (r, kind) in cands {
        if !(demand[r] > income[r] * DEMAND_HYSTERESIS && income[r] > 0.0) {
            continue;
        }
        let ratio = demand[r] / income[r].max(1e-9);
        let tta = time_to_afford(kind, stock, income);
        // Deficit relativo maggiore vince; a pari merito tiene il primo
        // (Metal: ordine tabella = deterministico).
        let better = match best {
            None => true,
            Some((_, br, _)) => ratio > br,
        };
        if better {
            best = Some((kind, ratio, tta));
        }
    }
    match best {
        None => EcoPlan {
            bottleneck: None,
            time_to_afford_secs: 0.0,
        },
        Some((kind, _, tta)) => EcoPlan {
            bottleneck: Some(kind),
            time_to_afford_secs: tta,
        },
    }
}

/// Secondi per accumulare stock sufficiente al costo (rate = income).
/// Oltre l'orizzonte → infinito pratico (non abbordabile: l'AI aspetta eco).
pub fn time_to_afford(kind: BuildingKind, stock: [f64; 2], income: [f64; 2]) -> f64 {
    let cost = BUILDINGS[kind as usize].cost.resources;
    let mut tta: f64 = 0.0;
    for r in 0..2 {
        let missing = (cost[r] - stock[r]).max(0.0);
        if missing <= 0.0 {
            continue;
        }
        if income[r] <= 0.0 {
            return f64::INFINITY;
        }
        tta = tta.max(missing / income[r]);
    }
    if tta > PLAN_HORIZON_SECS {
        f64::INFINITY
    } else {
        tta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_deficit_means_no_bottleneck() {
        let p = plan_build([100.0, 100.0], [5.0, 12.0], [4.0, 10.0]);
        assert_eq!(p.bottleneck, None);
        // Income a zero: niente domanda insoddisfatta misurabile.
        let p = plan_build([0.0, 0.0], [0.0, 0.0], [0.0, 0.0]);
        assert_eq!(p.bottleneck, None);
    }

    #[test]
    fn starved_metal_points_to_metal() {
        let p = plan_build([10.0, 50.0], [5.0, 24.0], [12.0, 10.0]);
        assert_eq!(p.bottleneck, Some(BuildingKind::Metal));
        assert!(p.time_to_afford_secs.is_finite());
        // Stock che copre il costo (100/80): tta = 0.
        let p = plan_build([200.0, 200.0], [5.0, 24.0], [12.0, 10.0]);
        assert_eq!(p.time_to_afford_secs, 0.0);
    }

    #[test]
    fn starved_energy_points_to_solar_and_bigger_ratio_wins() {
        let p = plan_build([50.0, 5.0], [10.0, 12.0], [5.0, 30.0]);
        assert_eq!(p.bottleneck, Some(BuildingKind::Solar));
        // Deficit relativo energia (3.0) > metallo (2.0): vince il Solare.
        let p = plan_build([0.0, 0.0], [5.0, 10.0], [10.0, 30.0]);
        assert_eq!(p.bottleneck, Some(BuildingKind::Solar));
    }

    #[test]
    fn time_to_afford_math() {
        // Metal costa 100/80: con 0 stock e 5/12 di income → max(20, 6.7).
        let tta = time_to_afford(BuildingKind::Metal, [0.0, 0.0], [5.0, 12.0]);
        assert!((tta - 20.0).abs() < 0.001, "{tta}");
        // Income zero su risorsa mancante → infinito (aspetta eco).
        assert_eq!(
            time_to_afford(BuildingKind::Metal, [0.0, 0.0], [0.0, 12.0]),
            f64::INFINITY
        );
        // Oltre l'orizzonte → infinito.
        assert_eq!(
            time_to_afford(BuildingKind::Metal, [0.0, 0.0], [0.5, 12.0]),
            f64::INFINITY
        );
    }

    #[test]
    fn prediction_is_repeatable() {
        let a = plan_build([10.0, 50.0], [5.0, 24.0], [12.0, 10.0]);
        let b = plan_build([10.0, 50.0], [5.0, 24.0], [12.0, 10.0]);
        assert_eq!(a, b);
    }
}
