//! Difficoltà a handicap puri (0.0.21): stesso cervello, solo freno.
//!
//! Borovikov Online Ch10 (autoplay per handicap): niente personalità separate,
//! niente bonus nascosti all'AI. Puro e deterministico (solo numeri, confronti
//! `total_cmp` dove serve, niente HashMap/rand/time).

use serde::{Deserialize, Serialize};

/// Freno sullo stesso cervello: APM (ordini/tick), malus al courage
/// (soglia win_prob più alta = attacca meno), periodo strategia più lento
/// (salta tick macro). `income_mult` resta 1.0 di default (niente cheat eco).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Handicap {
    pub max_orders_per_tick: usize,
    pub courage_malus: f32,
    pub strategy_period_mult: f32,
    pub income_mult: f64,
}

impl Default for Handicap {
    fn default() -> Self {
        Self::HARD
    }
}

impl Handicap {
    /// Hard = nessun freno (cervello pieno).
    pub const HARD: Self = Self {
        max_orders_per_tick: 8,
        courage_malus: 0.0,
        strategy_period_mult: 1.0,
        income_mult: 1.0,
    };
    /// Medium = metà APM, un po' più prudente, macro 2× lenta.
    pub const MEDIUM: Self = Self {
        max_orders_per_tick: 4,
        courage_malus: 0.1,
        strategy_period_mult: 2.0,
        income_mult: 1.0,
    };
    /// Easy = APM minima, molto prudente, macro 3× lenta. Deve perdere da hard.
    pub const EASY: Self = Self {
        max_orders_per_tick: 2,
        courage_malus: 0.2,
        strategy_period_mult: 3.0,
        income_mult: 1.0,
    };

    /// Strict come `resolve_brain`: ignoti sono errore.
    pub fn try_from_name(name: &str) -> Result<Self, String> {
        match name {
            "hard" => Ok(Self::HARD),
            "medium" => Ok(Self::MEDIUM),
            "easy" => Ok(Self::EASY),
            other => Err(format!(
                "Unknown handicap '{other}'; use one of: hard, medium, easy"
            )),
        }
    }

    /// Ordinamento freno: easy >= medium >= hard su ogni asse (a parte APM
    /// invertito: hard ha più ordini). Puro, per test `easy_capped`.
    pub fn capped_by(&self, cap: &Self) -> bool {
        self.max_orders_per_tick <= cap.max_orders_per_tick
            && self.courage_malus >= cap.courage_malus - f32::EPSILON
            && self.strategy_period_mult >= cap.strategy_period_mult - f32::EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_ordered_hard_beats_easy() {
        // Hard nessun freno, easy il più frenato. Stesso cervello, solo freno.
        assert_eq!(Handicap::default(), Handicap::HARD);
        assert!(Handicap::EASY.capped_by(&Handicap::MEDIUM));
        assert!(Handicap::MEDIUM.capped_by(&Handicap::HARD));
        assert!(Handicap::EASY.capped_by(&Handicap::HARD));
        assert!(!Handicap::HARD.capped_by(&Handicap::EASY));
        // Income mai toccato (niente cheat eco).
        assert_eq!(Handicap::EASY.income_mult, 1.0);
        assert_eq!(Handicap::HARD.income_mult, 1.0);
        assert!(Handicap::try_from_name("gandalf").is_err());
    }

    #[test]
    fn easy_capped() {
        // 0.0.21 — easy = APM 2, malus 0.2, periodo 3×: hard batte easy per
        // costruzione (stesso cervello, freno strictly maggiore).
        assert_eq!(Handicap::EASY.max_orders_per_tick, 2);
        assert!((Handicap::EASY.courage_malus - 0.2).abs() < 1e-6);
        assert!((Handicap::EASY.strategy_period_mult - 3.0).abs() < 1e-6);
    }
}
