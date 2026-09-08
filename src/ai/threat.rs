//! Threat/influence map pura in shadow (0.0.16).
//!
//! Legge SOLO [`AiSnapshot`](super::snapshot::AiSnapshot) — quindi è onesta
//! per costruzione (mai query nemiche dirette, mai fog bypassato).
//! In 0.0.16 nessuno la legge per decidere: serve solo come metrica
//! osservata (league/director) e base per 0.0.17+ (courage, torrette, scout).
//! Tutto puro e deterministico: ordine di accumulo stabile, `total_cmp` per
//! i confronti, niente `HashMap`/rand/time.

use crate::{
    economy::balance::{BuildingKind, turret_stats},
    navigation::HALF_SIZE,
    units::UnitKind,
};
use bevy::prelude::*;

use super::{memory::MEMORY_FRESH_TICKS, snapshot::AiSnapshot, strategy::combat_power};

/// Risoluzione griglia: 32×32 su tutta la mappa (cfr. fog 150×150 da 4m).
/// Grossolana di proposito: stima strategica, non targeting.
pub const THREAT_GRID_N: usize = 32;
/// Costante di decadimento ricordi: peso `1/(1+age/K)`.
pub const THREAT_DECAY_K: f32 = 60.0;
/// Sconto posizioni non verificate (coerente con `remembered_enemy_power`).
pub const THREAT_MEMORY_DISCOUNT: f32 = 0.5;

/// Mappa minaccia per-team, costruita dallo snapshot onesto.
#[derive(Clone, Debug)]
pub struct ThreatMap {
    pub n: usize,
    pub half: f32,
    pub cells: Vec<f32>,
}

impl ThreatMap {
    fn empty() -> Self {
        Self {
            n: THREAT_GRID_N,
            half: HALF_SIZE,
            cells: vec![0.0; THREAT_GRID_N * THREAT_GRID_N],
        }
    }

    fn cell_side(&self) -> f32 {
        (self.half * 2.0) / self.n as f32
    }

    fn index_for(&self, pos: Vec3) -> usize {
        let side = self.cell_side();
        let mut col = ((pos.x + self.half) / side).floor() as isize;
        let mut row = ((pos.z + self.half) / side).floor() as isize;
        col = col.clamp(0, self.n as isize - 1);
        row = row.clamp(0, self.n as isize - 1);
        row as usize * self.n + col as usize
    }

    /// Minaccia nella cella che contiene `pos` (0 fuori mappa → clamp al bordo).
    /// Uso diretto in 0.0.17+ (torrette/scout); in 0.0.16 solo test+metriche.
    #[allow(dead_code)]
    pub fn query(&self, pos: Vec3) -> f32 {
        self.cells[self.index_for(pos)]
    }

    /// Media su tutte le celle (0 se vuota).
    pub fn mean(&self) -> f32 {
        if self.cells.is_empty() {
            return 0.0;
        }
        let sum: f32 = self.cells.iter().sum();
        sum / self.cells.len() as f32
    }

    /// Massimo su tutte le celle.
    pub fn max(&self) -> f32 {
        self.cells.iter().fold(0.0f32, |a, b| a.max(*b))
    }

    /// Centro della cella più minacciosa. Pareggi → indice minore
    /// (deterministico). `None` se tutto zero.
    /// Uso diretto in 0.0.17+ (difesa/attacco); in 0.0.16 solo test.
    #[allow(dead_code)]
    pub fn hotspot(&self) -> Option<Vec3> {
        let mut best_idx: Option<usize> = None;
        let mut best_val = 0.0f32;
        for (i, v) in self.cells.iter().enumerate() {
            if *v > best_val {
                best_val = *v;
                best_idx = Some(i);
            }
        }
        let idx = best_idx?;
        let side = self.cell_side();
        let row = idx / self.n;
        let col = idx % self.n;
        Some(Vec3::new(
            -self.half + (col as f32 + 0.5) * side,
            0.0,
            -self.half + (row as f32 + 0.5) * side,
        ))
    }
}

/// Peso per età: 1 a vista live, decade per ricordi. Puro.
pub fn age_decay(age_ticks: u64) -> f32 {
    1.0 / (1.0 + age_ticks as f32 / THREAT_DECAY_K)
}

/// Minaccia di un edificio nemico visibile: solo torrette (da tabella
/// `turret_stats`, mai branch per-kind qui fuori). Muri/eco = 0.
pub fn building_threat(kind: BuildingKind, hp: f32) -> f32 {
    if hp <= 0.0 {
        return 0.0;
    }
    match turret_stats(kind) {
        Some(gun) => hp * (gun.damage / gun.cooldown.max(0.05)) * 0.8,
        None => 0.0,
    }
}

/// Costruisce la mappa dallo snapshot onesto: nemici visibili (peso 1) +
/// ricordi freschi di unità (peso `decay*discount`) + edifici-torrette
/// visibili. Edifici ricordati senza kind: ignorati (nessuna info).
/// Ordine di accumulo = ordine snapshot (già ordinato per determinismo).
pub fn build_threat(snapshot: &AiSnapshot) -> ThreatMap {
    let mut map = ThreatMap::empty();
    for e in &snapshot.visible_enemies {
        let w = threat_unit_weight(e.kind, e.health, 0);
        if w > 0.0 {
            let idx = map.index_for(e.pos);
            map.cells[idx] += w;
        }
    }
    for b in &snapshot.visible_enemy_buildings {
        let w = building_threat(b.kind, b.health);
        if w > 0.0 {
            let idx = map.index_for(b.pos);
            map.cells[idx] += w;
        }
    }
    for m in snapshot.remembered_troop_memory(MEMORY_FRESH_TICKS) {
        let Some(kind) = m.kind else { continue };
        let w = threat_unit_weight(kind, m.hp, m.age_ticks) * THREAT_MEMORY_DISCOUNT;
        if w > 0.0 {
            let idx = map.index_for(m.pos);
            map.cells[idx] += w;
        }
    }
    map
}

fn threat_unit_weight(kind: UnitKind, hp: f32, age_ticks: u64) -> f32 {
    if hp <= 0.0 {
        return 0.0;
    }
    combat_power(kind, hp) * age_decay(age_ticks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_map_is_zero_without_hotspot() {
        let snap = AiSnapshot {
            team: 1,
            ..Default::default()
        };
        let map = build_threat(&snap);
        assert_eq!(map.mean(), 0.0);
        assert_eq!(map.max(), 0.0);
        assert_eq!(map.hotspot(), None);
        assert_eq!(map.query(Vec3::ZERO), 0.0);
        // Fuori mappa: clamp al bordo, comunque 0.
        assert_eq!(map.query(Vec3::new(9999.0, 0.0, -9999.0)), 0.0);
    }

    #[test]
    fn confidence_matches_threat_decay() {
        // 0.0.19 — `memory::confidence` e `age_decay` stessa formula, stesso K:
        // percezione e threat non divergono mai.
        assert!((super::super::memory::CONFIDENCE_K - THREAT_DECAY_K).abs() < 1e-6);
        for age in [0u64, 1, 10, 60, 120, 240] {
            assert!(
                (super::super::memory::confidence(age) - age_decay(age)).abs() < 1e-6,
                "age {age}"
            );
        }
    }

    #[test]
    fn threat_decays_with_age() {
        assert!((age_decay(0) - 1.0).abs() < 1e-6);
        assert!(age_decay(60) < 1.0 && age_decay(60) > 0.0);
        assert!(age_decay(120) < age_decay(60));
        // Ricordo fresco pesa meno del visibile a pari kind/hp.
        let hp = crate::units::archetype(UnitKind::HeavyTank).max_health;
        let live = threat_unit_weight(UnitKind::HeavyTank, hp, 0);
        let remembered = threat_unit_weight(UnitKind::HeavyTank, hp, 60) * THREAT_MEMORY_DISCOUNT;
        assert!(remembered < live);
        assert!(remembered > 0.0);
    }

    #[test]
    fn threat_counts_turret_but_not_walls() {
        use crate::economy::balance::BuildingKind;
        assert!(building_threat(BuildingKind::Turret, 450.0) > 0.0);
        assert_eq!(building_threat(BuildingKind::Wall, 700.0), 0.0);
        assert_eq!(building_threat(BuildingKind::Metal, 350.0), 0.0);
        assert_eq!(building_threat(BuildingKind::Turret, 0.0), 0.0);
        // Engineer disarmato = 0 anche live.
        assert_eq!(threat_unit_weight(UnitKind::Engineer, 70.0, 0), 0.0);
        let tank = threat_unit_weight(UnitKind::HeavyTank, 170.0, 0);
        let scout = threat_unit_weight(UnitKind::Scout, 60.0, 0);
        assert!(tank > scout);
    }

    #[test]
    fn build_threat_spots_memory_and_hotspot_is_deterministic() {
        use super::super::snapshot::{AiEnemy, AiMemory};
        let mut snap = AiSnapshot {
            team: 1,
            tick: 10,
            ..Default::default()
        };
        snap.visible_enemies.push(AiEnemy {
            entity: Entity::from_bits(1),
            pos: Vec3::new(50.0, 0.0, 0.0),
            kind: UnitKind::HeavyTank,
            health: 170.0,
        });
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: Vec3::new(-80.0, 0.0, 20.0),
            age_ticks: 10,
            kind: Some(UnitKind::HeavyTank),
            hp: 170.0,
            building: false,
        });
        // Ricordo vecchio oltre TTL fresca: ignorato.
        snap.memory.push(AiMemory {
            entity_bits: None,
            pos: Vec3::new(0.0, 0.0, 100.0),
            age_ticks: MEMORY_FRESH_TICKS + 1,
            kind: Some(UnitKind::HeavyTank),
            hp: 170.0,
            building: false,
        });
        let a = build_threat(&snap);
        let b = build_threat(&snap);
        assert_eq!(a.cells, b.cells);
        assert!(a.max() > 0.0);
        // Hotspot sulla cella del visibile (peso maggiore del ricordo).
        let hot = a.hotspot().expect("deve esserci un hotspot");
        assert!((hot.x - 50.0).abs() < a.cell_side());
        // Il ricordo vecchio non ha lasciato traccia.
        assert_eq!(a.query(Vec3::new(0.0, 0.0, 100.0)), 0.0);
        // Ma il ricordo fresco sì.
        assert!(a.query(Vec3::new(-80.0, 0.0, 20.0)) > 0.0);
    }

    #[test]
    fn live_contact_is_not_counted_again_from_memory() {
        use super::super::snapshot::{AiEnemy, AiMemory};
        let entity = Entity::from_bits(77);
        let mut snap = AiSnapshot::default();
        snap.visible_enemies.push(AiEnemy {
            entity,
            pos: Vec3::ZERO,
            kind: UnitKind::HeavyTank,
            health: 170.0,
        });
        snap.memory.push(AiMemory {
            entity_bits: Some(entity.to_bits()),
            pos: Vec3::ZERO,
            age_ticks: 0,
            kind: Some(UnitKind::HeavyTank),
            hp: 170.0,
            building: false,
        });
        let expected = threat_unit_weight(UnitKind::HeavyTank, 170.0, 0);
        assert!((build_threat(&snap).max() - expected).abs() < 0.001);
    }
}
